mod app;
mod file_searcher;
mod ui;
mod tui;
mod event;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use app::App;
use event::{Event, EventHandler};
use tui::{Tui, TuiTrait};
use ratatui::backend::CrosstermBackend;
use std::io::stdout;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = ".")]
    directory: PathBuf,
    #[arg(short, long)]
    pattern: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize TUI backend and terminal
    let backend = CrosstermBackend::new(stdout());
    let mut tui = Tui::new(backend)?;
    tui.init()?;

    // Initialize application state
    let mut app = App::new(args.directory, args.pattern).await?;

    // Event handler
    let mut events = EventHandler::new(250);

    loop {
        tui.draw(|f| ui::render(f, &mut app))?;

        match events.next().await? {
            Event::Tick => app.tick().await?,
            Event::Key(key_event) => {
                if app.handle_key_event(key_event).await? {
                    break;
                }
            }
            Event::Mouse(_) => {}
            Event::Resize(_, _) => {}
        }
    }

    tui.exit()?;
    Ok(())
}
