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
    
    // Search results with pagination
    pub file_results: Vec<PathBuf>,
    pub content_results: Vec<SearchResult>,
    
    // All results (unpaginated for searching through)
    all_file_results: Vec<PathBuf>,
    all_content_results: Vec<SearchResult>,
    
    // Pagination
    pub current_page: usize,
    pub page_size: usize,
    pub total_pages: usize,
    
    // File searcher
    file_searcher: FileSearcher,
    
    // Debouncing for live search
    last_search_time: Instant,
    search_debounce_duration: Duration,
    
    // Async search state
    pub is_searching: bool,
    pub search_progress: String,
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
            all_file_results: Vec::new(),
            all_content_results: Vec::new(),
            current_page: 0,
            page_size: 1000, // Items per page
            total_pages: 0,
            file_searcher,
            last_search_time: Instant::now(),
            search_debounce_duration: Duration::from_millis(150),
            is_searching: false,
            search_progress: String::new(),
        };
        
        // Initial file listing
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
            // Pagination controls
            KeyCode::Char('n') | KeyCode::Char(']') => {
                self.next_page();
            }
            KeyCode::Char('p') | KeyCode::Char('[') => {
                self.prev_page();
            }
            KeyCode::Char('g') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.first_page();
            }
            KeyCode::Char('G') => {
                self.last_page();
            }
            // Navigation with improved bounds checking
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                    self.adjust_scroll();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max_index = self.get_current_page_items_len();
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
                let max_index = self.get_current_page_items_len();
                self.selected_index = (self.selected_index + 20).min(max_index.saturating_sub(1));
                self.adjust_scroll();
            }
            KeyCode::Home => {
                self.selected_index = 0;
                self.scroll_offset = 0;
            }
            KeyCode::End => {
                let max_index = self.get_current_page_items_len();
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
        self.all_file_results = files;
        
        self.update_pagination();
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
        self.all_file_results = results;
        
        self.update_pagination();
        self.reset_selection();
        self.is_searching = false;
        self.search_progress.clear();
        Ok(())
    }
    
    async fn search_content(&mut self) -> Result<()> {
        if self.search_query.is_empty() {
            self.all_content_results.clear();
            self.update_pagination();
            self.reset_selection();
            return Ok(());
        }
        
        self.is_searching = true;
        self.search_progress = format!("Searching content for '{}'...", self.search_query);
        
        let results = self.file_searcher.search_content(&self.search_query).await?;
        self.all_content_results = results;
        
        self.update_pagination();
        self.reset_selection();
        self.is_searching = false;
        self.search_progress.clear();
        Ok(())
    }
    
    fn update_pagination(&mut self) {
        let total_items = match self.mode {
            AppMode::FileBrowser => self.all_file_results.len(),
            AppMode::ContentSearch => self.all_content_results.len(),
            _ => 0,
        };
        
        self.total_pages = if total_items == 0 {
            0
        } else {
            (total_items + self.page_size - 1) / self.page_size
        };
        
        // Ensure current page is valid
        if self.current_page >= self.total_pages && self.total_pages > 0 {
            self.current_page = self.total_pages - 1;
        }
        
        self.update_current_page_results();
    }
    
    fn update_current_page_results(&mut self) {
        let start_idx = self.current_page * self.page_size;
        
        match self.mode {
            AppMode::FileBrowser => {
                let end_idx = ((start_idx + self.page_size).min(self.all_file_results.len())).max(start_idx);
                self.file_results = self.all_file_results[start_idx..end_idx].to_vec();
            }
            AppMode::ContentSearch => {
                let end_idx = ((start_idx + self.page_size).min(self.all_content_results.len())).max(start_idx);
                self.content_results = self.all_content_results[start_idx..end_idx].to_vec();
            }
            _ => {}
        }
    }
    
    fn next_page(&mut self) {
        if self.current_page + 1 < self.total_pages {
            self.current_page += 1;
            self.update_current_page_results();
            self.reset_selection();
        }
    }
    
    fn prev_page(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.update_current_page_results();
            self.reset_selection();
        }
    }
    
    fn first_page(&mut self) {
        if self.total_pages > 0 {
            self.current_page = 0;
            self.update_current_page_results();
            self.reset_selection();
        }
    }
    
    fn last_page(&mut self) {
        if self.total_pages > 0 {
            self.current_page = self.total_pages - 1;
            self.update_current_page_results();
            self.reset_selection();
        }
    }
    
    fn reset_selection(&mut self) {
        self.selected_index = 0;
        self.scroll_offset = 0;
    }
    
    fn get_current_page_items_len(&self) -> usize {
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
        let total_items = self.get_current_page_items_len();
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
        
        let total_items = match self.mode {
            AppMode::FileBrowser => self.all_file_results.len(),
            AppMode::ContentSearch => self.all_content_results.len(),
            _ => 0,
        };
        
        if self.total_pages <= 1 {
            match self.mode {
                AppMode::FileBrowser => format!("Files: {}", total_items),
                AppMode::ContentSearch => format!("Results: {}", total_items),
                AppMode::Help => "Help".to_string(),
            }
        } else {
            let start_item = self.current_page * self.page_size + 1;
            let end_item = ((self.current_page + 1) * self.page_size).min(total_items);
            
            match self.mode {
                AppMode::FileBrowser => format!(
                    "Files: {}-{} of {} (Page {}/{})", 
                    start_item, end_item, total_items, 
                    self.current_page + 1, self.total_pages
                ),
                AppMode::ContentSearch => format!(
                    "Results: {}-{} of {} (Page {}/{})", 
                    start_item, end_item, total_items, 
                    self.current_page + 1, self.total_pages
                ),
                AppMode::Help => "Help".to_string(),
            }
        }
    }
    
    pub fn get_pagination_info(&self) -> String {
        if self.total_pages <= 1 {
            String::new()
        } else {
            format!("Page {}/{} | n/]: Next | p/[: Prev | Ctrl+g: First | G: Last", 
                    self.current_page + 1, self.total_pages)
        }
    }
}
