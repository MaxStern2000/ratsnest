use anyhow::Result;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_path: PathBuf,
    pub line_number: usize,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
}

pub struct FileSearcher {
    root_directory: PathBuf,
    matcher: SkimMatcherV2,
    max_concurrent_reads: Arc<Semaphore>,
}

impl FileSearcher {
    pub fn new(root_directory: PathBuf) -> Result<Self> {
        Ok(Self {
            root_directory,
            matcher: SkimMatcherV2::default(),
            max_concurrent_reads: Arc::new(Semaphore::new(50)),
        })
    }

    pub async fn list_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        let walker = WalkBuilder::new(&self.root_directory)
            .hidden(false)
            .ignore(true)
            .git_ignore(true)
            .build();

        for entry in walker {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                if let Ok(relative_path) = path.strip_prefix(&self.root_directory) {
                    files.push(relative_path.to_path_buf());
                } else {
                    files.push(path.to_path_buf());
                }
            }
        }

        files.sort();
        Ok(files)
    }

    pub async fn fuzzy_search_files(&self, query: &str) -> Result<Vec<PathBuf>> {
        let all_files = self.list_files().await?;
        let mut scored_files: Vec<(PathBuf, i64)> = Vec::new();

        for file_path in all_files {
            let file_str = file_path.to_string_lossy();
            if let Some(score) = self.matcher.fuzzy_match(&file_str, query) {
                scored_files.push((file_path, score));
            }
        }

        scored_files.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(scored_files.into_iter().map(|(path, _)| path).collect())
    }

    pub async fn search_content(&self, query: &str) -> Result<Vec<SearchResult>> {
        let files = self.list_files().await?;
        let mut results = Vec::new();

        // Process files in chunks
        for chunk in files.chunks(20) {
            let chunk_results = self.search_content_in_files(chunk, query).await?;
            results.extend(chunk_results);
        }

        results.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.line_number.cmp(&b.line_number)));

        Ok(results)
    }

    async fn search_content_in_files(
        &self,
        files: &[PathBuf],
        query: &str,
    ) -> Result<Vec<SearchResult>> {
        let mut tasks: Vec<tokio::task::JoinHandle<Result<Vec<SearchResult>>>> = Vec::new();

        for file_path in files {
            let full_path = self.root_directory.join(file_path);
            let file_path = file_path.clone();
            let query = query.to_string();
            let semaphore = Arc::clone(&self.max_concurrent_reads);

            let task = tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                search_in_file(&full_path, &file_path, &query).await
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
}

async fn search_in_file(
    full_path: &Path,
    relative_path: &Path,
    query: &str,
) -> Result<Vec<SearchResult>> {
    if let Ok(metadata) = fs::metadata(full_path).await {
        if metadata.len() > 10_000_000 {
            return Ok(Vec::new());
        }
    }

    if let Some(ext) = full_path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        match ext_str.as_str() {
            "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "a" | "lib" | "obj"
            | "jpg" | "jpeg" | "png" | "gif" | "bmp" | "ico" | "svg"
            | "mp3" | "mp4" | "avi" | "mkv" | "wav" | "flac"
            | "zip" | "tar" | "gz" | "7z" | "rar" | "pdf" => return Ok(Vec::new()),
            _ => {}
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

    let mut results = Vec::new();
    let query_lower = query.to_lowercase();

    for (line_number, line) in content.lines().enumerate() {
        let line_lower = line.to_lowercase();
        if let Some(start) = line_lower.find(&query_lower) {
            results.push(SearchResult {
                file_path: relative_path.to_path_buf(),
                line_number: line_number + 1,
                line_content: line.to_string(),
                match_start: start,
                match_end: start + query.len(),
            });
        }
    }

    Ok(results)
}
