use crate::app::{App, AppMode, InputMode};

use ratatui::{
    prelude::{Frame, Rect, Color, Style, Alignment, Constraint, Direction},
    widgets::{Block, Paragraph, List, ListItem, Wrap, Borders},
    layout::Layout,
    style::Modifier,
    text::{Line, Span},
};

pub fn render(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(size);

    render_header(frame, chunks[0], app);

    match app.mode {
        AppMode::FileBrowser => render_file_browser(frame, chunks[1], app),
        AppMode::ContentSearch => render_content_search(frame, chunks[1], app),
        AppMode::Help => render_help(frame, chunks[1]),
    }

    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.mode {
        AppMode::FileBrowser => "File Browser",
        AppMode::ContentSearch => "Content Search",
        AppMode::Help => "Help",
    };

    let header_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(match app.mode {
            AppMode::FileBrowser => Style::default().fg(Color::Green),
            AppMode::ContentSearch => Style::default().fg(Color::Blue),
            AppMode::Help => Style::default().fg(Color::Yellow),
        });

    let path_info = Paragraph::new(format!("Directory: {}", app.current_directory.display()))
        .block(header_block)
        .wrap(Wrap { trim: true });

    frame.render_widget(path_info, area);
}

fn render_search_input(frame: &mut Frame, area: Rect, app: &App) {
    let input_style = match app.input_mode {
        InputMode::Normal => Style::default(),
        InputMode::Editing => Style::default().fg(Color::Yellow),
    };

    let input_block = Block::default()
        .title(" Search (Press '/' to edit, Enter to search) ")
        .borders(Borders::ALL)
        .border_style(input_style);

    let input = Paragraph::new(app.search_query.as_str())
        .block(input_block)
        .style(input_style);

    frame.render_widget(input, area);

    if app.input_mode == InputMode::Editing {
        frame.set_cursor_position((area.x + app.cursor_position as u16 + 1, area.y + 1));
    }
}

fn render_file_browser(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    render_search_input(frame, chunks[0], app);

    let (start_index, _total_items) = app.get_visible_items();
    let visible_items = app.file_results
        .iter()
        .skip(start_index)
        .take(chunks[1].height as usize)
        .enumerate()
        .map(|(i, path)| {
            let style = if start_index + i == app.selected_index {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            ListItem::new(path.display().to_string()).style(style)
        })
        .collect::<Vec<_>>();

    let list_title = format!(" {} ", app.get_status_info());

    let list = List::new(visible_items)
        .block(
            Block::default()
                .title(list_title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(list, chunks[1]);
}

fn render_content_search(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    render_search_input(frame, chunks[0], app);

    let (start_index, _total_items) = app.get_visible_items();
    let visible_items = app.content_results
        .iter()
        .skip(start_index)
        .take(chunks[1].height as usize)
        .enumerate()
        .map(|(i, result)| {
            let style = if start_index + i == app.selected_index {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            let mut spans = Vec::new();
            let line = &result.line_content;
            if result.match_start > 0 {
                spans.push(Span::raw(&line[..result.match_start]));
            }
            spans.push(Span::styled(
                &line[result.match_start..result.match_end],
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
            if result.match_end < line.len() {
                spans.push(Span::raw(&line[result.match_end..]));
            }

            let content = format!("{}:{} ", result.file_path.display(), result.line_number);
            let mut full_spans = vec![Span::styled(content, Style::default().fg(Color::Cyan))];
            full_spans.extend(spans);

            ListItem::new(Line::from(full_spans)).style(style)
        })
        .collect::<Vec<_>>();

    let list_title = format!(" {} ", app.get_status_info());

    let list = List::new(visible_items)
        .block(
            Block::default()
                .title(list_title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(list, chunks[1]);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(vec![Span::styled("Navigation:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from("  ↑/k, ↓/j    Navigate up/down"),
        Line::from("  Page Up/Down  Navigate by pages"),
        Line::from("  Home/End     Go to start/end"),
        Line::from(""),
        Line::from(vec![Span::styled("Search:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from("  /            Start search (live for files)"),
        Line::from("  Enter        Execute content search"),
        Line::from("  Esc          Cancel search"),
        Line::from(""),
        Line::from(vec![Span::styled("Modes:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from("  Tab          Switch between File/Content modes"),
        Line::from("  h/F1         Toggle help"),
        Line::from("  r            Refresh/reload files"),
        Line::from(""),
        Line::from(vec![Span::styled("General:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from("  q            Quit application"),
        Line::from(""),
        Line::from(vec![Span::styled("Features:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
        Line::from("• Fuzzy search for filenames (live)"),
        Line::from("• Content search across all files"),
        Line::from("• Respects .gitignore files"),
        Line::from("• Async/concurrent file processing"),
        Line::from("• Highlight matches in search"),
        Line::from("• Caching for improved performance"),
    ];

    let help = Paragraph::new(help_text)
        .block(Block::default()
            .title(" Help - Press 'h' or F1 to close ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)))
        .wrap(Wrap { trim: true });

    frame.render_widget(help, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let mode_indicator = match app.mode {
        AppMode::FileBrowser => "[FILES]",
        AppMode::ContentSearch => "[SEARCH]",
        AppMode::Help => "[HELP]",
    };

    let input_indicator = match app.input_mode {
        InputMode::Normal => "NORMAL",
        InputMode::Editing => "EDITING",
    };

    let footer_text = format!(
        " {} | {} | Tab: Switch Mode | /: Search | r: Refresh | q: Quit | h: Help ",
        mode_indicator, input_indicator
    );

    let footer = Paragraph::new(footer_text)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray)))
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center);

    frame.render_widget(footer, area);
}
