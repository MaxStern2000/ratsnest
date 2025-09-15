use anyhow::Result;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_path: PathBuf,
    pub line_number: usize,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Clone)]
struct CachedFileList {
    files: Vec<PathBuf>,
    last_updated: Instant,
}

pub struct FileSearcher {
    root_directory: PathBuf,
    matcher: SkimMatcherV2,
    max_concurrent_reads: Arc<Semaphore>,
    // Cache for file listings
    cached_files: Arc<RwLock<Option<CachedFileList>>>,
    // Cache for fuzzy search results
    fuzzy_cache: Arc<Mutex<HashMap<String, Vec<PathBuf>>>>,
    cache_duration: Duration,
}

impl FileSearcher {
    pub fn new(root_directory: PathBuf) -> Result<Self> {
        Ok(Self {
            root_directory,
            matcher: SkimMatcherV2::default(),
            max_concurrent_reads: Arc::new(Semaphore::new(100)), // Increased for better parallelism
            cached_files: Arc::new(RwLock::new(None)),
            fuzzy_cache: Arc::new(Mutex::new(HashMap::new())),
            cache_duration: Duration::from_secs(30), // Cache for 30 seconds
        })
    }

    // Async file listing with caching
    pub async fn list_files(&self) -> Result<Vec<PathBuf>> {
        // Check if we have a valid cache
        {
            let cache = self.cached_files.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.last_updated.elapsed() < self.cache_duration {
                    return Ok(cached.files.clone());
                }
            }
        }

        // Cache is invalid or doesn't exist, rebuild it
        let files = self.build_file_list().await?;
        
        // Update cache
        {
            let mut cache = self.cached_files.write().await;
            *cache = Some(CachedFileList {
                files: files.clone(),
                last_updated: Instant::now(),
            });
        }

        Ok(files)
    }

    async fn build_file_list(&self) -> Result<Vec<PathBuf>> {
        // Use tokio::task::spawn_blocking for CPU-intensive file traversal
        let root_dir = self.root_directory.clone();
        
        Ok(tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();

            let walker = WalkBuilder::new(&root_dir)
                .hidden(false)
                .ignore(true)
                .git_ignore(true)
                .max_depth(Some(10)) // Limit depth to avoid deep recursion
                .build();

            for entry in walker.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(relative_path) = path.strip_prefix(&root_dir) {
                        files.push(relative_path.to_path_buf());
                    } else {
                        files.push(path.to_path_buf());
                    }
                }
            }

            files.sort_unstable(); // Slightly faster than sort()
            files
        }).await?)
    }

    // Optimized fuzzy search with caching and early termination
    pub async fn fuzzy_search_files(&self, query: &str) -> Result<Vec<PathBuf>> {
        if query.is_empty() {
            return self.list_files().await;
        }

        // Check cache first
        {
            let cache = self.fuzzy_cache.lock().await;
            if let Some(cached_results) = cache.get(query) {
                return Ok(cached_results.clone());
            }
        }

        let all_files = self.list_files().await?;
        
        // Use spawn_blocking for CPU-intensive fuzzy matching
        let query_str = query.to_string();
        let matcher = SkimMatcherV2::default(); // Create new matcher since it doesn't implement Clone
        
        let results = tokio::task::spawn_blocking(move || {
            let mut scored_files: Vec<(PathBuf, i64)> = Vec::new();
            
            // Process files in chunks to avoid blocking too long
            const CHUNK_SIZE: usize = 1000;
            
            for chunk in all_files.chunks(CHUNK_SIZE) {
                for file_path in chunk {
                    let file_str = file_path.to_string_lossy();
                    if let Some(score) = matcher.fuzzy_match(&file_str, &query_str) {
                        scored_files.push((file_path.clone(), score));
                        
                        // Early termination for very large result sets
                        if scored_files.len() > 5000 {
                            break;
                        }
                    }
                }
                
                // Yield control periodically
                if scored_files.len() > CHUNK_SIZE {
                    std::thread::yield_now();
                }
            }

            // Sort by score (higher is better) and take top results
            scored_files.sort_unstable_by(|a, b| b.1.cmp(&a.1));
            scored_files.truncate(1000); // Limit results to top 1000
            
            scored_files.into_iter().map(|(path, _)| path).collect::<Vec<_>>()
        }).await?;

        // Cache the results
        {
            let mut cache = self.fuzzy_cache.lock().await;
            // Limit cache size to prevent memory bloat
            if cache.len() > 100 {
                cache.clear();
            }
            cache.insert(query.to_string(), results.clone());
        }

        Ok(results)
    }

    // Optimized content search with better concurrency
    pub async fn search_content(&self, query: &str) -> Result<Vec<SearchResult>> {
        let files = self.list_files().await?;
        let mut results = Vec::new();

        // Process files in smaller chunks for better responsiveness
        const CHUNK_SIZE: usize = 50;
        
        for chunk in files.chunks(CHUNK_SIZE) {
            let chunk_results = self.search_content_in_files_parallel(chunk, query).await?;
            results.extend(chunk_results);
            
            // Yield control to allow UI updates
            tokio::task::yield_now().await;
            
            // Early termination if we have too many results
            if results.len() > 10000 {
                break;
            }
        }

        results.sort_unstable_by(|a, b| {
            a.file_path.cmp(&b.file_path).then(a.line_number.cmp(&b.line_number))
        });

        Ok(results)
    }

    async fn search_content_in_files_parallel(
        &self,
        files: &[PathBuf],
        query: &str,
    ) -> Result<Vec<SearchResult>> {
        let semaphore = Arc::clone(&self.max_concurrent_reads);
        let mut tasks = Vec::new();

        for file_path in files {
            let full_path = self.root_directory.join(file_path);
            let file_path = file_path.clone();
            let query = query.to_string();
            let semaphore = Arc::clone(&semaphore);

            let task = tokio::spawn(async move {
                let _permit = semaphore.acquire().await.map_err(|_| {
                    anyhow::anyhow!("Failed to acquire semaphore permit")
                })?;
                search_in_file_optimized(&full_path, &file_path, &query).await
            });

            tasks.push(task);
        }

        let mut results = Vec::new();
        for task in tasks {
            match task.await {
                Ok(Ok(file_results)) => results.extend(file_results),
                Ok(Err(_)) | Err(_) => {
                    // Skip files that can't be read or cause errors
                    continue;
                }
            }
        }

        Ok(results)
    }

    // Method to invalidate caches when needed
    pub async fn invalidate_caches(&self) {
        {
            let mut file_cache = self.cached_files.write().await;
            *file_cache = None;
        }
        {
            let mut fuzzy_cache = self.fuzzy_cache.lock().await;
            fuzzy_cache.clear();
        }
    }
}

// Optimized file search function
async fn search_in_file_optimized(
    full_path: &Path,
    relative_path: &Path,
    query: &str,
) -> Result<Vec<SearchResult>> {
    // Quick metadata check
    let metadata = match fs::metadata(full_path).await {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Vec::new()),
    };

    // Skip very large files
    if metadata.len() > 10_000_000 {
        return Ok(Vec::new());
    }

    // Skip binary files based on extension
    if let Some(ext) = full_path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        if matches!(ext_str.as_str(),
            "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "a" | "lib" | "obj"
            | "jpg" | "jpeg" | "png" | "gif" | "bmp" | "ico" | "svg" | "webp"
            | "mp3" | "mp4" | "avi" | "mkv" | "wav" | "flac" | "ogg"
            | "zip" | "tar" | "gz" | "7z" | "rar" | "pdf" | "class" | "jar"
        ) {
            return Ok(Vec::new());
        }
    }

    // Read file content
    let mut file = match fs::File::open(full_path).await {
        Ok(f) => f,
        Err(_) => return Ok(Vec::new()),
    };

    let mut content = String::new();
    if file.read_to_string(&mut content).await.is_err() {
        return Ok(Vec::new());
    }

    // Early return if file is too large after reading
    if content.len() > 5_000_000 {
        return Ok(Vec::new());
    }

    // Optimized search
    let mut results = Vec::new();
    let query_lower = query.to_lowercase();
    
    // Use lines iterator which is more efficient
    for (line_number, line) in content.lines().enumerate() {
        // Skip very long lines that are likely binary or generated
        if line.len() > 1000 {
            continue;
        }
        
        let line_lower = line.to_lowercase();
        if let Some(start) = line_lower.find(&query_lower) {
            results.push(SearchResult {
                file_path: relative_path.to_path_buf(),
                line_number: line_number + 1,
                line_content: line.to_string(),
                match_start: start,
                match_end: start + query.len(),
            });
            
            // Limit results per file to prevent memory bloat
            if results.len() > 100 {
                break;
            }
        }
    }

    Ok(results)
}
