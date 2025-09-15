use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use tokio::time::{Duration, Instant};

use crate::file_searcher::{FileSearcher, SearchResult};

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    FileBrowser,
    ContentSearch,
    Help,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    Editing,
}

pub struct App {
    pub should_quit: bool,
    pub mode: AppMode,
    pub input_mode: InputMode,
    pub current_directory: PathBuf,
    pub search_query: String,
    pub cursor_position: usize,
    pub selected_index: usize,
    pub scroll_offset: usize,
    
    // Search results
    pub file_results: Vec<PathBuf>,
    pub content_results: Vec<SearchResult>,
    
    // File searcher
    file_searcher: FileSearcher,
    
    // Debouncing for live search
    last_search_time: Instant,
    search_debounce_duration: Duration,
    
    // Async search state
    pub is_searching: bool,
    pub search_progress: String,
    
    // Pagination for large result sets
    pub max_visible_items: usize,
}

impl App {
    pub async fn new(directory: PathBuf, initial_pattern: Option<String>) -> Result<Self> {
        let file_searcher = FileSearcher::new(directory.clone())?;
        
        let mut app = Self {
            should_quit: false,
            mode: AppMode::FileBrowser,
            input_mode: InputMode::Normal,
            current_directory: directory,
            search_query: initial_pattern.unwrap_or_default(),
            cursor_position: 0,
            selected_index: 0,
            scroll_offset: 0,
            file_results: Vec::new(),
            content_results: Vec::new(),
            file_searcher,
            last_search_time: Instant::now(),
            search_debounce_duration: Duration::from_millis(150), // 150ms debounce
            is_searching: false,
            search_progress: String::new(),
            max_visible_items: 1000, // Limit UI rendering
        };
        
        // Initial file listing (async)
        app.refresh_files().await?;
        
        Ok(app)
    }
    
    pub async fn handle_key_event(&mut self, key_event: KeyEvent) -> Result<bool> {
        match self.input_mode {
            InputMode::Normal => self.handle_normal_mode(key_event).await,
            InputMode::Editing => self.handle_editing_mode(key_event).await,
        }
    }
    
    async fn handle_normal_mode(&mut self, key_event: KeyEvent) -> Result<bool> {
        match key_event.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('h') | KeyCode::F(1) => {
                self.mode = if self.mode == AppMode::Help {
                    AppMode::FileBrowser
                } else {
                    AppMode::Help
                };
            }
            KeyCode::Tab => {
                self.mode = match self.mode {
                    AppMode::FileBrowser => AppMode::ContentSearch,
                    AppMode::ContentSearch => AppMode::FileBrowser,
                    AppMode::Help => AppMode::FileBrowser,
                };
                self.reset_selection();
            }
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Editing;
                self.search_query.clear();
                self.cursor_position = 0;
            }
            KeyCode::Enter => {
                if !self.search_query.is_empty() {
                    match self.mode {
                        AppMode::FileBrowser => self.search_files().await?,
                        AppMode::ContentSearch => self.search_content().await?,
                        _ => {}
                    }
                }
            }
            KeyCode::Char('r') => {
                // Refresh/reload files
                self.file_searcher.invalidate_caches().await;
                self.refresh_files().await?;
            }
            // Navigation with improved bounds checking
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                    self.adjust_scroll();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max_index = self.get_max_index();
                if self.selected_index + 1 < max_index {
                    self.selected_index += 1;
                    self.adjust_scroll();
                }
            }
            KeyCode::PageUp => {
                self.selected_index = self.selected_index.saturating_sub(20);
                self.adjust_scroll();
            }
            KeyCode::PageDown => {
                let max_index = self.get_max_index();
                self.selected_index = (self.selected_index + 20).min(max_index.saturating_sub(1));
                self.adjust_scroll();
            }
            KeyCode::Home => {
                self.selected_index = 0;
                self.scroll_offset = 0;
            }
            KeyCode::End => {
                let max_index = self.get_max_index();
                self.selected_index = max_index.saturating_sub(1);
                self.adjust_scroll();
            }
            _ => {}
        }
        Ok(false)
    }
    
    async fn handle_editing_mode(&mut self, key_event: KeyEvent) -> Result<bool> {
        match key_event.code {
            KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                match self.mode {
                    AppMode::FileBrowser => self.search_files().await?,
                    AppMode::ContentSearch => self.search_content().await?,
                    _ => {}
                }
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char(c) => {
                if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                    match c {
                        'c' => self.input_mode = InputMode::Normal,
                        _ => {}
                    }
                } else {
                    self.search_query.insert(self.cursor_position, c);
                    self.cursor_position += 1;
                    self.last_search_time = Instant::now();
                    
                    // Only do live search for file browser and if query is not too short
                    if self.mode == AppMode::FileBrowser && self.search_query.len() >= 1 {
                        self.schedule_debounced_search().await?;
                    }
                }
            }
            KeyCode::Backspace => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                    self.search_query.remove(self.cursor_position);
                    self.last_search_time = Instant::now();
                    
                    if self.mode == AppMode::FileBrowser {
                        self.schedule_debounced_search().await?;
                    }
                }
            }
            KeyCode::Left => {
                self.cursor_position = self.cursor_position.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor_position = (self.cursor_position + 1).min(self.search_query.len());
            }
            _ => {}
        }
        Ok(false)
    }
    
    async fn schedule_debounced_search(&mut self) -> Result<()> {
        // Simple debouncing - we'll check in tick() if enough time has passed
        Ok(())
    }
    
    async fn refresh_files(&mut self) -> Result<()> {
        self.is_searching = true;
        self.search_progress = "Loading files...".to_string();
        
        let files = self.file_searcher.list_files().await?;
        self.file_results = files.into_iter().take(self.max_visible_items).collect();
        
        self.reset_selection();
        self.is_searching = false;
        self.search_progress.clear();
        Ok(())
    }
    
    async fn search_files(&mut self) -> Result<()> {
        if self.search_query.is_empty() {
            self.refresh_files().await?;
            return Ok(());
        }
        
        self.is_searching = true;
        self.search_progress = format!("Searching files for '{}'...", self.search_query);
        
        let results = self.file_searcher.fuzzy_search_files(&self.search_query).await?;
        self.file_results = results.into_iter().take(self.max_visible_items).collect();
        
        self.reset_selection();
        self.is_searching = false;
        self.search_progress.clear();
        Ok(())
    }
    
    async fn search_content(&mut self) -> Result<()> {
        if self.search_query.is_empty() {
            self.content_results.clear();
            self.reset_selection();
            return Ok(());
        }
        
        self.is_searching = true;
        self.search_progress = format!("Searching content for '{}'...", self.search_query);
        
        let results = self.file_searcher.search_content(&self.search_query).await?;
        self.content_results = results.into_iter().take(self.max_visible_items).collect();
        
        self.reset_selection();
        self.is_searching = false;
        self.search_progress.clear();
        Ok(())
    }
    
    fn reset_selection(&mut self) {
        self.selected_index = 0;
        self.scroll_offset = 0;
    }
    
    fn get_max_index(&self) -> usize {
        match self.mode {
            AppMode::FileBrowser => self.file_results.len(),
            AppMode::ContentSearch => self.content_results.len(),
            _ => 0,
        }
    }
    
    fn adjust_scroll(&mut self) {
        const VISIBLE_ITEMS: usize = 25; // Adjust based on terminal height
        
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + VISIBLE_ITEMS {
            self.scroll_offset = self.selected_index.saturating_sub(VISIBLE_ITEMS - 1);
        }
    }
    
    pub async fn tick(&mut self) -> Result<()> {
        // Handle debounced search for file browser
        if self.input_mode == InputMode::Editing 
            && self.mode == AppMode::FileBrowser 
            && !self.is_searching
            && self.last_search_time.elapsed() >= self.search_debounce_duration 
        {
            // Check if we need to trigger a search
            let should_search = self.last_search_time.elapsed() >= self.search_debounce_duration;
            
            if should_search {
                // Reset the timer to prevent repeated searches
                self.last_search_time = Instant::now() + Duration::from_secs(3600);
                self.search_files().await?;
            }
        }
        
        Ok(())
    }
    
    pub fn get_visible_items(&self) -> (usize, usize) {
        let total_items = self.get_max_index();
        (self.scroll_offset, total_items)
    }
    
    pub fn get_current_file(&self) -> Option<&PathBuf> {
        if self.mode == AppMode::FileBrowser && self.selected_index < self.file_results.len() {
            Some(&self.file_results[self.selected_index])
        } else {
            None
        }
    }
    
    pub fn get_current_content_result(&self) -> Option<&SearchResult> {
        if self.mode == AppMode::ContentSearch && self.selected_index < self.content_results.len() {
            Some(&self.content_results[self.selected_index])
        } else {
            None
        }
    }
    
    pub fn get_status_info(&self) -> String {
        if self.is_searching && !self.search_progress.is_empty() {
            return self.search_progress.clone();
        }
        
        match self.mode {
            AppMode::FileBrowser => {
                let total = self.file_results.len();
                if total >= self.max_visible_items {
                    format!("Files: {}+ (showing first {})", total, self.max_visible_items)
                } else {
                    format!("Files: {}", total)
                }
            }
            AppMode::ContentSearch => {
                let total = self.content_results.len();
                if total >= self.max_visible_items {
                    format!("Results: {}+ (showing first {})", total, self.max_visible_items)
                } else {
                    format!("Results: {}", total)
                }
            }
            AppMode::Help => "Help".to_string(),
        }
    }
}
