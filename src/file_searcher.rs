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
            max_concurrent_reads: Arc::new(Semaphore::new(100)),
            cached_files: Arc::new(RwLock::new(None)),
            fuzzy_cache: Arc::new(Mutex::new(HashMap::new())),
            cache_duration: Duration::from_secs(30),
        })
    }

    /// Async file listing with caching
    pub async fn list_files(&self) -> Result<Vec<PathBuf>> {
        {
            let cache = self.cached_files.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.last_updated.elapsed() < self.cache_duration {
                    return Ok(cached.files.clone());
                }
            }
        }

        let files = self.build_file_list().await?;

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
        let root_dir = self.root_directory.clone();

        Ok(tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
            let mut files = Vec::new();

            let walker = WalkBuilder::new(&root_dir)
                .hidden(false)
                .ignore(true)
                .git_ignore(true)
                .max_depth(Some(10))
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

            files.sort_unstable();
            Ok(files)
        })
        .await??)
    }

    /// Fuzzy search with caching
    pub async fn fuzzy_search_files(&self, query: &str) -> Result<Vec<PathBuf>> {
        if query.is_empty() {
            return self.list_files().await;
        }

        {
            let cache = self.fuzzy_cache.lock().await;
            if let Some(cached_results) = cache.get(query) {
                return Ok(cached_results.clone());
            }
        }

        let all_files = self.list_files().await?;
        let query_str = query.to_string();
        let matcher = SkimMatcherV2::default();

        let results = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
            let mut scored_files: Vec<(PathBuf, i64)> = Vec::new();

            for file_path in all_files {
                let file_str = file_path.to_string_lossy();
                if let Some(score) = matcher.fuzzy_match(&file_str, &query_str) {
                    scored_files.push((file_path.clone(), score));
                }
            }

            scored_files.sort_unstable_by(|a, b| b.1.cmp(&a.1));
            Ok(scored_files.into_iter().map(|(path, _)| path).collect())
        })
        .await??;

        {
            let mut cache = self.fuzzy_cache.lock().await;
            if cache.len() > 100 {
                cache.clear();
            }
            cache.insert(query.to_string(), results.clone());
        }

        Ok(results)
    }

    /// Content search
    pub async fn search_content(&self, query: &str) -> Result<Vec<SearchResult>> {
        let files = self.list_files().await?;
        let mut results = Vec::new();

        const CHUNK_SIZE: usize = 50;

        for chunk in files.chunks(CHUNK_SIZE) {
            let chunk_results = self.search_content_in_files_parallel(chunk, query).await?;
            results.extend(chunk_results);
            tokio::task::yield_now().await;
        }

        results.sort_unstable_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then(a.line_number.cmp(&b.line_number))
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
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to acquire semaphore permit"))?;
                search_in_file_optimized(&full_path, &file_path, &query).await
            });

            tasks.push(task);
        }

        let mut results = Vec::new();
        for task in tasks {
            if let Ok(Ok(file_results)) = task.await {
                results.extend(file_results);
            }
        }

        Ok(results)
    }

    /// Invalidate caches
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

/// Optimized file content search
async fn search_in_file_optimized(
    full_path: &Path,
    relative_path: &Path,
    query: &str,
) -> Result<Vec<SearchResult>> {
    let metadata = match fs::metadata(full_path).await {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Vec::new()),
    };

    if metadata.len() > 10_000_000 {
        return Ok(Vec::new());
    }

    if let Some(ext) = full_path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        if matches!(
            ext_str.as_str(),
            "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "a" | "lib" | "obj"
                | "jpg" | "jpeg" | "png" | "gif" | "bmp" | "ico" | "svg" | "webp"
                | "mp3" | "mp4" | "avi" | "mkv" | "wav" | "flac" | "ogg"
                | "zip" | "tar" | "gz" | "7z" | "rar" | "pdf" | "class" | "jar"
        ) {
            return Ok(Vec::new());
        }
    }

    let mut file = match fs::File::open(full_path).await {
        Ok(f) => f,
        Err(_) => return Ok(Vec::new()),
    };

    let mut content = String::new();
    if file.read_to_string(&mut content).await.is_err() {
        return Ok(Vec::new());
    }

    if content.len() > 5_000_000 {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    let query_lower = query.to_lowercase();

    for (line_number, line) in content.lines().enumerate() {
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

            if results.len() > 100 {
                break;
            }
        }
    }

    Ok(results)
}
