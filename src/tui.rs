use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, Terminal};
use std::io::{stdout, Stdout};

use crate::{app::App, ui};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub trait TuiTrait {
    fn new() -> Result<Tui>;
    fn init(&mut self) -> Result<()>;
    fn draw(&mut self, app: &mut App) -> Result<()>;
    fn exit(&mut self) -> Result<()>;
}

impl TuiTrait for Tui {
    fn new() -> Result<Tui> {
        let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        Ok(terminal)
    }
    
    fn init(&mut self) -> Result<()> {
        execute!(stdout(), EnterAlternateScreen)?;
        enable_raw_mode()?;
        self.clear()?;
        Ok(())
    }
    
    fn draw(&mut self, app: &mut App) -> Result<()> {
        self.draw(|frame| ui::render(frame, app))?;
        Ok(())
    }
    
    fn exit(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(stdout(), LeaveAlternateScreen)?;
        Ok(())
    }
}
