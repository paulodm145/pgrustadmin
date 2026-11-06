use std::{io, time::Duration};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures_util::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

const MENU_ITEMS: [&str; 3] = ["Overview", "Connections", "SQL workspace"];

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("TUI stopped: {error}");
        std::process::exit(1);
    }
}

async fn run() -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = run_app(&mut terminal).await;
    ratatui::try_restore()?;
    result
}

#[derive(Default)]
struct App {
    selected: usize,
    should_quit: bool,
}

impl App {
    fn handle_event(&mut self, event: Event) {
        let Event::Key(key) = event else {
            return;
        };
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1) % MENU_ITEMS.len();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = (self.selected + MENU_ITEMS.len() - 1) % MENU_ITEMS.len();
            }
            _ => {}
        }
    }
}

async fn run_app(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::default();
    let mut events = EventStream::new();
    let mut redraw = tokio::time::interval(Duration::from_millis(250));

    terminal.draw(|frame| render(frame, &app))?;
    while !app.should_quit {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(event)) => app.handle_event(event),
                    Some(Err(error)) => return Err(io::Error::other(error)),
                    None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream ended")),
                }
                terminal.draw(|frame| render(frame, &app))?;
            }
            _ = redraw.tick() => {
                terminal.draw(|frame| render(frame, &app))?;
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);

    let title = Paragraph::new("PostgreSQL workspace").block(
        Block::default()
            .borders(Borders::ALL)
            .title(" PGRustAdmin "),
    );
    frame.render_widget(title, vertical[0]);

    let menu = List::new(
        MENU_ITEMS
            .iter()
            .map(|item| ListItem::new(Line::from(format!(" {item}")))),
    )
    .block(Block::default().borders(Borders::ALL).title(" Sections "))
    .highlight_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
    )
    .highlight_symbol("› ");
    let mut menu_state = ListState::default().with_selected(Some(app.selected));

    let content = match app.selected {
        1 => ("Connections", "No connection selected."),
        2 => ("SQL workspace", "No SQL workspace open."),
        _ => ("Overview", "No active session."),
    };
    let details = Paragraph::new(Text::from(vec![
        Line::from(content.0).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(content.1),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" Workspace "));

    if area.width < 60 {
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(3)])
            .split(vertical[1]);
        frame.render_stateful_widget(menu, body[0], &mut menu_state);
        frame.render_widget(details, body[1]);
    } else {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(24), Constraint::Min(0)])
            .split(vertical[1]);
        frame.render_stateful_widget(menu, body[0], &mut menu_state);
        frame.render_widget(details, body[1]);
    }

    let footer = Paragraph::new("↑/↓ or j/k: navigate   q/Esc: quit")
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, vertical[2]);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use super::{App, MENU_ITEMS, render};

    #[test]
    fn keyboard_navigation_wraps_and_quit_keys_exit() {
        let mut app = App::default();
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        assert_eq!(app.selected, MENU_ITEMS.len() - 1);
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        assert_eq!(app.selected, 0);
        app.handle_event(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )));
        assert!(!app.should_quit);
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(app.should_quit);
    }

    #[test]
    fn layout_renders_on_wide_and_narrow_terminals() {
        for (width, height) in [(100, 30), (40, 14)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("create test terminal");
            terminal
                .draw(|frame| render(frame, &App::default()))
                .expect("render layout");
            let buffer = terminal.backend().buffer();
            let rendered = buffer
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(rendered.contains("PGRustAdmin"));
            assert!(rendered.contains("Connections"));
        }
    }
}
