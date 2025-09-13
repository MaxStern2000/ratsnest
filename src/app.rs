use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use tokio::sync::mpsc;

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
    
    // Async search state
    search_tx: Option<mpsc::UnboundedSender<()>>,
    search_rx: mpsc::UnboundedReceiver<Vec<SearchResult>>,
    pub is_searching: bool,
}

impl App {
    pub async fn new(directory: PathBuf, initial_pattern: Option<String>) -> Result<Self> {
        let file_searcher = FileSearcher::new(directory.clone())?;
        let (_, search_rx) = mpsc::unbounded_channel();
        
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
            search_tx: None,
            search_rx,
            is_searching: false,
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
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                    self.adjust_scroll();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max_index = match self.mode {
                    AppMode::FileBrowser => self.file_results.len(),
                    AppMode::ContentSearch => self.content_results.len(),
                    _ => 0,
                };
                if self.selected_index + 1 < max_index {
                    self.selected_index += 1;
                    self.adjust_scroll();
                }
            }
            KeyCode::PageUp => {
                self.selected_index = self.selected_index.saturating_sub(10);
                self.adjust_scroll();
            }
            KeyCode::PageDown => {
                let max_index = match self.mode {
                    AppMode::FileBrowser => self.file_results.len(),
                    AppMode::ContentSearch => self.content_results.len(),
                    _ => 0,
                };
                self.selected_index = (self.selected_index + 10).min(max_index.saturating_sub(1));
                self.adjust_scroll();
            }
            KeyCode::Home => {
                self.selected_index = 0;
                self.scroll_offset = 0;
            }
            KeyCode::End => {
                let max_index = match self.mode {
                    AppMode::FileBrowser => self.file_results.len(),
                    AppMode::ContentSearch => self.content_results.len(),
                    _ => 0,
                };
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
                    // Live search for files
                    if self.mode == AppMode::FileBrowser {
                        self.search_files().await?;
                    }
                }
            }
            KeyCode::Backspace => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                    self.search_query.remove(self.cursor_position);
                    // Live search for files
                    if self.mode == AppMode::FileBrowser {
                        self.search_files().await?;
                    }
                }
            }
            KeyCode::Left => {
                self.cursor_position = self.cursor_position.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor_position = self.cursor_position.min(self.search_query.len());
            }
            _ => {}
        }
        Ok(false)
    }
    
    async fn refresh_files(&mut self) -> Result<()> {
        self.file_results = self.file_searcher.list_files().await?;
        self.selected_index = 0;
        self.scroll_offset = 0;
        Ok(())
    }
    
    async fn search_files(&mut self) -> Result<()> {
        if self.search_query.is_empty() {
            self.refresh_files().await?;
        } else {
            self.file_results = self.file_searcher.fuzzy_search_files(&self.search_query).await?;
            self.selected_index = 0;
            self.scroll_offset = 0;
        }
        Ok(())
    }
    
    async fn search_content(&mut self) -> Result<()> {
        if !self.search_query.is_empty() {
            self.is_searching = true;
            self.content_results = self.file_searcher.search_content(&self.search_query).await?;
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.is_searching = false;
        }
        Ok(())
    }
    
    fn adjust_scroll(&mut self) {
        const VISIBLE_ITEMS: usize = 20; // Adjust based on terminal height
        
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + VISIBLE_ITEMS {
            self.scroll_offset = self.selected_index - VISIBLE_ITEMS + 1;
        }
    }
    
    pub async fn tick(&mut self) -> Result<()> {
        // Handle any pending search results
        while let Ok(results) = self.search_rx.try_recv() {
            self.content_results = results;
            self.is_searching = false;
        }
        Ok(())
    }
    
    pub fn get_visible_items(&self) -> (usize, usize) {
        let total_items = match self.mode {
            AppMode::FileBrowser => self.file_results.len(),
            AppMode::ContentSearch => self.content_results.len(),
            _ => 0,
        };
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
}
