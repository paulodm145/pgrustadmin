use std::{io, time::Duration};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use pgrustadmin_api_client::{ApiClient, ApiError, AuthSession};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

const MENU_ITEMS: [&str; 3] = ["Overview", "Connections", "SQL workspace"];

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("TUI stopped: {error}");
        std::process::exit(1);
    }
}

async fn run() -> io::Result<()> {
    let client = ApiClient::from_env().map_err(io::Error::other)?;
    let mut terminal = ratatui::try_init()?;
    let result = run_app(&mut terminal, client).await;
    ratatui::try_restore()?;
    result
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoginField {
    Email,
    Password,
}

enum ScreenState {
    Editing {
        focus: LoginField,
        error: Option<String>,
    },
    Authenticating,
    Authenticated(AuthSession),
}

struct App {
    client: Option<ApiClient>,
    login_task: Option<JoinHandle<(ApiClient, Result<AuthSession, ApiError>)>>,
    email: String,
    password: Zeroizing<String>,
    state: ScreenState,
    selected: usize,
    should_quit: bool,
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(task) = &self.login_task {
            task.abort();
        }
    }
}

impl App {
    fn new(client: ApiClient) -> Self {
        Self {
            client: Some(client),
            login_task: None,
            email: String::new(),
            password: Zeroizing::new(String::new()),
            state: ScreenState::Editing {
                focus: LoginField::Email,
                error: None,
            },
            selected: 0,
            should_quit: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return false;
        }
        match &mut self.state {
            ScreenState::Editing { focus, error } => match key.code {
                KeyCode::Esc => self.should_quit = true,
                KeyCode::Tab | KeyCode::Down => {
                    *focus = match focus {
                        LoginField::Email => LoginField::Password,
                        LoginField::Password => LoginField::Email,
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *focus = match focus {
                        LoginField::Email => LoginField::Password,
                        LoginField::Password => LoginField::Email,
                    }
                }
                KeyCode::Enter if *focus == LoginField::Email => *focus = LoginField::Password,
                KeyCode::Enter => return true,
                KeyCode::Backspace if *focus == LoginField::Email => {
                    self.email.pop();
                }
                KeyCode::Backspace => {
                    self.password.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    error.take();
                    if *focus == LoginField::Email {
                        self.email.push(character);
                    } else {
                        self.password.push(character);
                    }
                }
                _ => {}
            },
            ScreenState::Authenticating => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.should_quit = true;
                }
            }
            ScreenState::Authenticated(_) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1) % MENU_ITEMS.len();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = (self.selected + MENU_ITEMS.len() - 1) % MENU_ITEMS.len();
                }
                _ => {}
            },
        }
        false
    }

    fn begin_login(&mut self) {
        if self.email.trim().is_empty() || self.password.is_empty() {
            self.state = ScreenState::Editing {
                focus: LoginField::Email,
                error: Some("Email and password are required.".to_owned()),
            };
            return;
        }
        let Some(mut client) = self.client.take() else {
            self.state = ScreenState::Editing {
                focus: LoginField::Email,
                error: Some("Unable to start sign-in.".to_owned()),
            };
            return;
        };
        let email = self.email.clone();
        let password = std::mem::replace(&mut self.password, Zeroizing::new(String::new()));
        self.state = ScreenState::Authenticating;
        self.login_task = Some(tokio::spawn(async move {
            let result = async {
                let login = client.login(&email, &password).await?;
                let session = client.current_session().await?;
                if login.user_id != session.user_id {
                    return Err(ApiError::InvalidResponse);
                }
                Ok(login)
            }
            .await;
            (client, result)
        }));
    }

    async fn complete_login_if_ready(&mut self) {
        let is_finished = self
            .login_task
            .as_ref()
            .is_some_and(JoinHandle::is_finished);
        if !is_finished {
            return;
        }
        let Some(task) = self.login_task.take() else {
            return;
        };
        match task.await {
            Ok((client, Ok(session))) => {
                self.client = Some(client);
                self.state = ScreenState::Authenticated(session);
            }
            Ok((client, Err(error))) => {
                self.client = Some(client);
                self.state = ScreenState::Editing {
                    focus: LoginField::Password,
                    error: Some(login_error_message(error).to_owned()),
                };
            }
            Err(_) => {
                self.client = ApiClient::from_env().ok();
                self.state = ScreenState::Editing {
                    focus: LoginField::Email,
                    error: Some("Sign-in could not be completed.".to_owned()),
                };
            }
        }
    }
}

fn login_error_message(error: ApiError) -> &'static str {
    match error {
        ApiError::Http {
            code: Some("invalid_credentials"),
            ..
        }
        | ApiError::Http {
            code: Some("unauthorized"),
            ..
        } => "Email or password is incorrect.",
        ApiError::Transport => "Could not contact the API.",
        _ => "Sign-in failed. Check the API and try again.",
    }
}

async fn run_app(terminal: &mut DefaultTerminal, client: ApiClient) -> io::Result<()> {
    let mut app = App::new(client);
    let mut events = EventStream::new();
    let mut redraw = tokio::time::interval(Duration::from_millis(100));

    terminal.draw(|frame| render(frame, &app))?;
    while !app.should_quit {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) => {
                        if app.handle_key(key) {
                            app.begin_login();
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(io::Error::other(error)),
                    None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream ended")),
                }
                terminal.draw(|frame| render(frame, &app))?;
            }
            _ = redraw.tick() => {
                app.complete_login_if_ready().await;
                terminal.draw(|frame| render(frame, &app))?;
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new("PostgreSQL workspace").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" PGRustAdmin "),
        ),
        sections[0],
    );

    match &app.state {
        ScreenState::Authenticated(session) => render_workspace(frame, sections[1], app, session),
        state => render_login(frame, sections[1], app, state),
    }

    let footer_text = if matches!(app.state, ScreenState::Authenticated(_)) {
        "↑/↓ or j/k: navigate   q/Esc: quit"
    } else {
        "Tab: next field   Enter: continue/sign in   Esc: quit"
    };
    frame.render_widget(
        Paragraph::new(footer_text).block(Block::default().borders(Borders::ALL)),
        sections[2],
    );
}

fn render_login(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    app: &App,
    state: &ScreenState,
) {
    let form = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .margin(1)
        .split(area);
    let focus = match state {
        ScreenState::Editing { focus, .. } => Some(*focus),
        _ => None,
    };
    frame.render_widget(
        Paragraph::new(app.email.as_str())
            .block(field_block("Email", focus == Some(LoginField::Email))),
        form[0],
    );
    let masked_password = "•".repeat(app.password.chars().count());
    frame.render_widget(
        Paragraph::new(masked_password)
            .block(field_block("Password", focus == Some(LoginField::Password))),
        form[1],
    );

    let status = match state {
        ScreenState::Editing {
            error: Some(error), ..
        } => error.as_str(),
        ScreenState::Editing { error: None, .. } => "Sign in with your PGRustAdmin account.",
        ScreenState::Authenticating => "Signing in and checking session…",
        ScreenState::Authenticated(_) => "",
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Yellow)),
        form[2],
    );
}

fn field_block(title: &'static str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(style)
}

fn render_workspace(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    app: &App,
    session: &AuthSession,
) {
    let body_direction = if area.width < 60 {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let constraints = if body_direction == Direction::Vertical {
        vec![Constraint::Length(5), Constraint::Min(3)]
    } else {
        vec![Constraint::Length(24), Constraint::Min(0)]
    };
    let body = Layout::default()
        .direction(body_direction)
        .constraints(constraints)
        .split(area);
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
    frame.render_stateful_widget(menu, body[0], &mut menu_state);

    let detail = match app.selected {
        1 => ("Connections", "No connection selected."),
        2 => ("SQL workspace", "No SQL workspace open."),
        _ => ("Overview", "Authenticated session is active."),
    };
    let detail_text = Text::from(vec![
        Line::from(detail.0).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(detail.1),
        Line::from(format!("User: {}", session.user_id)),
        Line::from(format!("Session lifetime: {} seconds", session.expires_in)),
    ]);
    frame.render_widget(
        Paragraph::new(detail_text)
            .block(Block::default().borders(Borders::ALL).title(" Workspace ")),
        body[1],
    );
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pgrustadmin_api_client::ApiClient;
    use ratatui::{Terminal, backend::TestBackend};
    use uuid::Uuid;

    use super::{App, LoginField, ScreenState, render};

    fn client() -> ApiClient {
        ApiClient::new("http://127.0.0.1:3000").expect("valid API URL")
    }

    #[test]
    fn login_form_tracks_focus_and_hides_password_text() {
        let mut app = App::new(client());
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(
            app.state,
            ScreenState::Editing {
                focus: LoginField::Password,
                ..
            }
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render login");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains('•'));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn submit_requires_credentials_and_control_c_quits() {
        let mut app = App::new(client());
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        app.begin_login();
        assert!(matches!(
            app.state,
            ScreenState::Editing { error: Some(_), .. }
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn authenticated_session_state_is_renderable() {
        let mut app = App::new(client());
        app.state = ScreenState::Authenticated(pgrustadmin_api_client::AuthSession {
            user_id: Uuid::new_v4(),
            expires_in: 43200,
        });
        let backend = TestBackend::new(40, 14);
        let mut terminal = Terminal::new(backend).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render session");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Authenticated session is active."));
    }
}
