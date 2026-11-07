use std::{io, time::Duration};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use pgrustadmin_api_client::{
    ApiClient, ApiError, AuthSession, Connection, ConnectionInput, ConnectionTest, Database,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

const MENU_ITEMS: [&str; 2] = ["Overview", "Connections"];
const CONNECTION_FIELDS: [&str; 7] = [
    "Name", "Host", "Port", "Database", "Username", "Password", "TLS",
];

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
    EditingLogin {
        focus: LoginField,
        error: Option<String>,
    },
    Authenticating,
    Authenticated(AuthSession),
}

#[derive(Clone, Copy)]
enum Operation {
    Login,
    LoadConnections { page: u64 },
    LoadDatabases { connection_id: Uuid },
    SaveConnection { id: Option<Uuid> },
    TestConnection { id: Uuid },
    DeleteConnection { id: Uuid },
}

enum OperationResult {
    Login(Result<AuthSession, ApiError>),
    Connections(Result<Vec<Connection>, ApiError>),
    Databases(Result<Vec<Database>, ApiError>),
    Saved(Result<Connection, ApiError>),
    Tested(Result<ConnectionTest, ApiError>),
    Deleted(Result<(), ApiError>),
}

#[derive(Clone, Copy)]
enum UiAction {
    None,
    Login,
    LoadConnections(u64),
    LoadDatabases(Uuid),
    SaveConnection,
    TestConnection(Uuid),
    DeleteConnection(Uuid),
}

struct ConnectionForm {
    id: Option<Uuid>,
    name: String,
    host: String,
    port: String,
    database: String,
    username: String,
    password: Zeroizing<String>,
    tls_enabled: bool,
    focus: usize,
}

impl ConnectionForm {
    fn create() -> Self {
        Self {
            id: None,
            name: String::new(),
            host: String::new(),
            port: "5432".to_owned(),
            database: String::new(),
            username: String::new(),
            password: Zeroizing::new(String::new()),
            tls_enabled: true,
            focus: 0,
        }
    }

    fn edit(connection: &Connection) -> Self {
        Self {
            id: Some(connection.id),
            name: connection.name.clone(),
            host: connection.host.clone(),
            port: connection.port.to_string(),
            database: connection.database.clone(),
            username: connection.username.clone(),
            password: Zeroizing::new(String::new()),
            tls_enabled: connection.tls_enabled,
            focus: 0,
        }
    }

    fn input(&self) -> Result<ConnectionInput, &'static str> {
        let port = self
            .port
            .parse::<u16>()
            .map_err(|_| "Port must be between 1 and 65535.")?;
        if self.name.trim().is_empty()
            || self.host.trim().is_empty()
            || self.database.trim().is_empty()
            || self.username.trim().is_empty()
            || port == 0
        {
            return Err("Name, host, port, database and username are required.");
        }
        if self.id.is_none() && self.password.is_empty() {
            return Err("A password is required for a new connection.");
        }
        Ok(ConnectionInput {
            name: self.name.clone(),
            host: self.host.clone(),
            port,
            database: self.database.clone(),
            username: self.username.clone(),
            tls_enabled: self.tls_enabled,
            password: (!self.password.is_empty()).then(|| self.password.to_string()),
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        match key.code {
            KeyCode::Esc => return false,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % CONNECTION_FIELDS.len(),
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + CONNECTION_FIELDS.len() - 1) % CONNECTION_FIELDS.len();
            }
            KeyCode::Left | KeyCode::Right if self.focus == 6 => {
                self.tls_enabled = !self.tls_enabled;
            }
            KeyCode::Char(' ') if self.focus == 6 => self.tls_enabled = !self.tls_enabled,
            KeyCode::Enter if self.focus == 6 => return true,
            KeyCode::Enter => self.focus = (self.focus + 1) % CONNECTION_FIELDS.len(),
            KeyCode::Backspace => match self.focus {
                0 => {
                    self.name.pop();
                }
                1 => {
                    self.host.pop();
                }
                2 => {
                    self.port.pop();
                }
                3 => {
                    self.database.pop();
                }
                4 => {
                    self.username.pop();
                }
                5 => {
                    self.password.pop();
                }
                _ => {}
            },
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focus {
                    0 => self.name.push(character),
                    1 => self.host.push(character),
                    2 if character.is_ascii_digit() => self.port.push(character),
                    3 => self.database.push(character),
                    4 => self.username.push(character),
                    5 => self.password.push(character),
                    _ => {}
                }
            }
            _ => {}
        }
        false
    }
}

struct App {
    client: Option<ApiClient>,
    pending: Option<JoinHandle<(ApiClient, OperationResult)>>,
    login_email: String,
    login_password: Zeroizing<String>,
    state: ScreenState,
    view: usize,
    connections: Vec<Connection>,
    selected_connection: usize,
    connection_page: u64,
    catalog_connection: Option<Uuid>,
    catalog_connection_name: String,
    databases: Vec<Database>,
    selected_database: usize,
    form: Option<ConnectionForm>,
    confirm_delete: Option<Uuid>,
    message: String,
    should_quit: bool,
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(task) = &self.pending {
            task.abort();
        }
    }
}

impl App {
    fn new(client: ApiClient) -> Self {
        Self {
            client: Some(client),
            pending: None,
            login_email: String::new(),
            login_password: Zeroizing::new(String::new()),
            state: ScreenState::EditingLogin {
                focus: LoginField::Email,
                error: None,
            },
            view: 0,
            connections: Vec::new(),
            selected_connection: 0,
            connection_page: 1,
            catalog_connection: None,
            catalog_connection_name: String::new(),
            databases: Vec::new(),
            selected_database: 0,
            form: None,
            confirm_delete: None,
            message: String::new(),
            should_quit: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> UiAction {
        if key.kind != KeyEventKind::Press {
            return UiAction::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return UiAction::None;
        }
        if key.code == KeyCode::Esc
            && self.view == 2
            && self.form.is_none()
            && matches!(self.state, ScreenState::Authenticated(_))
        {
            self.view = 1;
            return UiAction::None;
        }
        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
            && !matches!(self.state, ScreenState::EditingLogin { .. })
            && self.form.is_none()
        {
            if self.form.take().is_some() || self.confirm_delete.take().is_some() {
                return UiAction::None;
            }
            self.should_quit = true;
            return UiAction::None;
        }
        if self.pending.is_some() {
            return UiAction::None;
        }
        if let Some(form) = &mut self.form {
            if key.code == KeyCode::Esc {
                self.form = None;
                self.message.clear();
                return UiAction::None;
            }
            if form.handle_key(key) {
                return UiAction::SaveConnection;
            }
            return UiAction::None;
        }
        if let Some(connection_id) = self.confirm_delete {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_delete = None;
                    return UiAction::DeleteConnection(connection_id);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_delete = None;
                    self.message = "Deletion cancelled.".to_owned();
                }
                _ => {}
            }
            return UiAction::None;
        }

        match &mut self.state {
            ScreenState::EditingLogin { focus, error } => match key.code {
                KeyCode::Esc => self.should_quit = true,
                KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => {
                    *focus = match focus {
                        LoginField::Email => LoginField::Password,
                        LoginField::Password => LoginField::Email,
                    }
                }
                KeyCode::Enter if *focus == LoginField::Email => *focus = LoginField::Password,
                KeyCode::Enter => return UiAction::Login,
                KeyCode::Backspace if *focus == LoginField::Email => {
                    self.login_email.pop();
                }
                KeyCode::Backspace => {
                    self.login_password.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    error.take();
                    if *focus == LoginField::Email {
                        self.login_email.push(character);
                    } else {
                        self.login_password.push(character);
                    }
                }
                _ => {}
            },
            ScreenState::Authenticating => {}
            ScreenState::Authenticated(_) => {
                if self.view == 2 {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') if !self.databases.is_empty() => {
                            self.selected_database =
                                (self.selected_database + self.databases.len() - 1)
                                    % self.databases.len();
                        }
                        KeyCode::Down | KeyCode::Char('j') if !self.databases.is_empty() => {
                            self.selected_database =
                                (self.selected_database + 1) % self.databases.len();
                        }
                        KeyCode::Char('r') => {
                            if let Some(connection_id) = self.catalog_connection {
                                return UiAction::LoadDatabases(connection_id);
                            }
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 1,
                        _ => {}
                    }
                } else if self.view == 1 {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') if !self.connections.is_empty() => {
                            self.selected_connection =
                                (self.selected_connection + self.connections.len() - 1)
                                    % self.connections.len();
                        }
                        KeyCode::Down | KeyCode::Char('j') if !self.connections.is_empty() => {
                            self.selected_connection =
                                (self.selected_connection + 1) % self.connections.len();
                        }
                        KeyCode::Char('r') => {
                            return UiAction::LoadConnections(self.connection_page);
                        }
                        KeyCode::Char('[') if self.connection_page > 1 => {
                            self.connection_page -= 1;
                            self.selected_connection = 0;
                            return UiAction::LoadConnections(self.connection_page);
                        }
                        KeyCode::Char(']') if self.connections.len() == 100 => {
                            self.connection_page += 1;
                            self.selected_connection = 0;
                            return UiAction::LoadConnections(self.connection_page);
                        }
                        KeyCode::Char('n') => {
                            self.form = Some(ConnectionForm::create());
                            self.message.clear();
                        }
                        KeyCode::Char('e') => {
                            if let Some(connection) = self.connections.get(self.selected_connection)
                            {
                                self.form = Some(ConnectionForm::edit(connection));
                                self.message.clear();
                            }
                        }
                        KeyCode::Char('t') => {
                            if let Some(connection) = self.connections.get(self.selected_connection)
                            {
                                return UiAction::TestConnection(connection.id);
                            }
                        }
                        KeyCode::Char('d') => {
                            self.confirm_delete = self
                                .connections
                                .get(self.selected_connection)
                                .map(|item| item.id);
                        }
                        KeyCode::Char('c') => {
                            if let Some(connection) = self.connections.get(self.selected_connection)
                            {
                                self.catalog_connection = Some(connection.id);
                                self.catalog_connection_name = connection.name.clone();
                                self.databases.clear();
                                self.selected_database = 0;
                                self.view = 2;
                                return UiAction::LoadDatabases(connection.id);
                            }
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 0,
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                            self.view = 1
                        }
                        KeyCode::Tab | KeyCode::Enter => {
                            self.view = (self.view + 1) % MENU_ITEMS.len();
                            if self.view == 1 {
                                return UiAction::LoadConnections(self.connection_page);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        UiAction::None
    }

    fn start_operation(&mut self, operation: Operation, action: UiAction) {
        let Some(mut client) = self.client.take() else {
            self.message = "API client is unavailable.".to_owned();
            return;
        };
        match action {
            UiAction::Login => self.state = ScreenState::Authenticating,
            UiAction::LoadConnections(_) => self.message = "Loading connections…".to_owned(),
            UiAction::LoadDatabases(_) => self.message = "Loading databases…".to_owned(),
            UiAction::SaveConnection => self.message = "Saving connection…".to_owned(),
            UiAction::TestConnection(_) => self.message = "Testing connection…".to_owned(),
            UiAction::DeleteConnection(_) => self.message = "Deleting connection…".to_owned(),
            UiAction::None => return,
        }

        let email = (matches!(operation, Operation::Login)).then(|| self.login_email.clone());
        let password = if matches!(operation, Operation::Login) {
            std::mem::replace(&mut self.login_password, Zeroizing::new(String::new()))
        } else {
            Zeroizing::new(String::new())
        };
        let input = if let Operation::SaveConnection { .. } = operation {
            match self.form.as_ref().map(ConnectionForm::input) {
                Some(Ok(input)) => Some(input),
                Some(Err(error)) => {
                    self.client = Some(client);
                    self.message = error.to_owned();
                    return;
                }
                None => {
                    self.client = Some(client);
                    self.message = "Connection form is unavailable.".to_owned();
                    return;
                }
            }
        } else {
            None
        };
        if matches!(operation, Operation::SaveConnection { .. }) && input.is_none() {
            self.client = Some(client);
            return;
        }

        self.pending = Some(tokio::spawn(async move {
            let result = run_operation(
                &mut client,
                operation,
                email.as_deref().unwrap_or_default(),
                &password,
                input.as_ref(),
            )
            .await;
            (client, result)
        }));
    }

    async fn complete_operation_if_ready(&mut self) {
        if !self.pending.as_ref().is_some_and(JoinHandle::is_finished) {
            return;
        }
        let Some(task) = self.pending.take() else {
            return;
        };
        match task.await {
            Ok((client, result)) => {
                self.client = Some(client);
                match result {
                    OperationResult::Login(Ok(session)) => {
                        self.state = ScreenState::Authenticated(session);
                        self.start_operation(
                            Operation::LoadConnections {
                                page: self.connection_page,
                            },
                            UiAction::LoadConnections(self.connection_page),
                        );
                    }
                    OperationResult::Login(Err(error)) => {
                        self.state = ScreenState::EditingLogin {
                            focus: LoginField::Password,
                            error: Some(api_error_message(error).to_owned()),
                        };
                    }
                    OperationResult::Connections(Ok(connections)) => {
                        self.connections = connections;
                        self.selected_connection = self
                            .selected_connection
                            .min(self.connections.len().saturating_sub(1));
                        self.message = if self.connections.is_empty() {
                            "No connections yet. Press n to add one.".to_owned()
                        } else {
                            format!("Loaded {} connection(s).", self.connections.len())
                        };
                    }
                    OperationResult::Connections(Err(error)) => {
                        self.message = api_error_message(error).to_owned();
                    }
                    OperationResult::Databases(Ok(databases)) => {
                        self.databases = databases;
                        self.selected_database = self
                            .selected_database
                            .min(self.databases.len().saturating_sub(1));
                        self.message = format!("Loaded {} database(s).", self.databases.len());
                    }
                    OperationResult::Databases(Err(error)) => {
                        self.databases.clear();
                        self.selected_database = 0;
                        self.message = api_error_message(error).to_owned();
                    }
                    OperationResult::Saved(Ok(_)) => {
                        self.form = None;
                        self.connection_page = 1;
                        self.message = "Connection saved.".to_owned();
                        self.start_operation(
                            Operation::LoadConnections { page: 1 },
                            UiAction::LoadConnections(1),
                        );
                    }
                    OperationResult::Saved(Err(error)) => {
                        self.message = api_error_message(error).to_owned();
                    }
                    OperationResult::Tested(Ok(result)) => {
                        self.message = if result.success {
                            "Connection test succeeded.".to_owned()
                        } else {
                            "Connection test failed.".to_owned()
                        };
                    }
                    OperationResult::Tested(Err(error)) => {
                        self.message = api_error_message(error).to_owned();
                    }
                    OperationResult::Deleted(Ok(())) => {
                        self.connection_page = 1;
                        self.message = "Connection deleted.".to_owned();
                        self.start_operation(
                            Operation::LoadConnections { page: 1 },
                            UiAction::LoadConnections(1),
                        );
                    }
                    OperationResult::Deleted(Err(error)) => {
                        self.message = api_error_message(error).to_owned();
                    }
                }
            }
            Err(_) => {
                self.client = ApiClient::from_env().ok();
                self.message = "The API operation did not complete.".to_owned();
            }
        }
    }
}

fn api_error_message(error: ApiError) -> &'static str {
    match error {
        ApiError::Http {
            code: Some("invalid_credentials"),
            ..
        }
        | ApiError::Http {
            code: Some("unauthorized"),
            ..
        } => "Email or password is incorrect.",
        ApiError::Http {
            code: Some("not_found"),
            ..
        } => "Connection is no longer available.",
        ApiError::Http {
            code: Some("connection_name_conflict"),
            ..
        } => "A connection with this name already exists.",
        ApiError::Http {
            code: Some("catalog_unavailable"),
            ..
        } => "Could not retrieve databases from this connection.",
        ApiError::Transport => "Could not contact the API.",
        _ => "The request failed. Check the API and try again.",
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
                        let action = app.handle_key(key);
                        start_action(&mut app, action);
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(io::Error::other(error)),
                    None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream ended")),
                }
                terminal.draw(|frame| render(frame, &app))?;
            }
            _ = redraw.tick() => {
                app.complete_operation_if_ready().await;
                terminal.draw(|frame| render(frame, &app))?;
            }
        }
    }
    Ok(())
}

fn start_action(app: &mut App, action: UiAction) {
    let operation = match action {
        UiAction::None => return,
        UiAction::Login => Operation::Login,
        UiAction::LoadConnections(page) => Operation::LoadConnections { page },
        UiAction::LoadDatabases(connection_id) => Operation::LoadDatabases { connection_id },
        UiAction::SaveConnection => Operation::SaveConnection {
            id: app.form.as_ref().and_then(|form| form.id),
        },
        UiAction::TestConnection(id) => Operation::TestConnection { id },
        UiAction::DeleteConnection(id) => Operation::DeleteConnection { id },
    };
    app.start_operation(operation, action);
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
        ScreenState::Authenticated(session) => {
            if let Some(form) = &app.form {
                render_connection_form(frame, sections[1], form, &app.message);
            } else if app.view == 1 {
                render_connections(frame, sections[1], app);
            } else if app.view == 2 {
                render_databases(frame, sections[1], app);
            } else {
                render_workspace(frame, sections[1], app, session);
            }
        }
        state => render_login(frame, sections[1], app, state),
    }

    let footer_text = if app.pending.is_some() {
        "Request in progress…   q/Esc: quit"
    } else if app.form.is_some() {
        "Tab: next field   Enter: save   Esc: cancel"
    } else if app.confirm_delete.is_some() {
        "Delete this connection? y: confirm   n/Esc: cancel"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 1 {
        "n: new   e: edit   t: test   c: databases   d: delete   [ / ]: pages   r: reload   Tab: overview   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 2 {
        "↑/↓: navigate   r: reload   Tab/Esc: connections   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) {
        "Tab/Enter: open Connections   ↑/↓: select   q/Esc: quit"
    } else {
        "Tab: next field   Enter: continue/sign in   Esc: quit"
    };
    frame.render_widget(
        Paragraph::new(footer_text).block(Block::default().borders(Borders::ALL)),
        sections[2],
    );
}

fn render_login(frame: &mut Frame<'_>, area: Rect, app: &App, state: &ScreenState) {
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
        ScreenState::EditingLogin { focus, .. } => Some(*focus),
        _ => None,
    };
    frame.render_widget(
        Paragraph::new(app.login_email.as_str())
            .block(field_block("Email", focus == Some(LoginField::Email))),
        form[0],
    );
    let masked_password = "•".repeat(app.login_password.chars().count());
    frame.render_widget(
        Paragraph::new(masked_password)
            .block(field_block("Password", focus == Some(LoginField::Password))),
        form[1],
    );
    let status = match state {
        ScreenState::EditingLogin {
            error: Some(error), ..
        } => error.as_str(),
        ScreenState::EditingLogin { error: None, .. } => "Sign in with your PGRustAdmin account.",
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

fn render_workspace(frame: &mut Frame<'_>, area: Rect, app: &App, session: &AuthSession) {
    let layout = responsive_layout(area);
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
    let mut menu_state = ListState::default().with_selected(Some(app.view));
    frame.render_stateful_widget(menu, layout[0], &mut menu_state);
    let content = Text::from(vec![
        Line::from("Overview").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from("Authenticated session is active."),
        Line::from(format!("User: {}", session.user_id)),
        Line::from(format!("Session lifetime: {} seconds", session.expires_in)),
        Line::from("Select Connections to manage saved PostgreSQL servers."),
    ]);
    frame.render_widget(
        Paragraph::new(content).block(Block::default().borders(Borders::ALL).title(" Workspace ")),
        layout[1],
    );
}

fn responsive_layout(area: Rect) -> Vec<Rect> {
    let direction = if area.width < 60 {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let constraints = if direction == Direction::Vertical {
        vec![Constraint::Length(4), Constraint::Min(3)]
    } else {
        vec![Constraint::Length(24), Constraint::Min(0)]
    };
    Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area)
        .to_vec()
}

fn render_connections(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let layout = responsive_layout(area);
    let items = app.connections.iter().map(|connection| {
        ListItem::new(vec![
            Line::from(connection.name.as_str()),
            Line::from(format!("  {}:{}", connection.host, connection.port)),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Connections "),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default()
        .with_selected((!app.connections.is_empty()).then_some(app.selected_connection));
    frame.render_stateful_widget(list, layout[0], &mut list_state);

    let details = if let Some(connection) = app.connections.get(app.selected_connection) {
        Text::from(vec![
            Line::from(connection.name.as_str()).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(format!("Host: {}:{}", connection.host, connection.port)),
            Line::from(format!("Database: {}", connection.database)),
            Line::from(format!("Username: {}", connection.username)),
            Line::from(format!(
                "TLS: {}",
                if connection.tls_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )),
            Line::from("Password: stored securely"),
        ])
    } else {
        Text::from("No saved connections. Press n to add one.")
    };
    let status = if app.confirm_delete.is_some() {
        "Confirm deletion? This cannot be undone.".to_owned()
    } else {
        format!("Page {} · {}", app.connection_page, app.message)
    };
    let panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(2)])
        .split(layout[1]);
    frame.render_widget(
        Paragraph::new(details).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Connection details "),
        ),
        panel[0],
    );
    frame.render_widget(Paragraph::new(status), panel[1]);
}

fn render_databases(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = app
        .databases
        .iter()
        .map(|database| ListItem::new(database.name.as_str()));
    let title = format!(" Databases — {} ", app.catalog_connection_name);
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default()
        .with_selected((!app.databases.is_empty()).then_some(app.selected_database));
    frame.render_stateful_widget(list, area, &mut list_state);
    if app.databases.is_empty() {
        frame.render_widget(
            Paragraph::new(app.message.as_str()).style(Style::default().fg(Color::Yellow)),
            area,
        );
    }
}

fn render_connection_form(frame: &mut Frame<'_>, area: Rect, form: &ConnectionForm, message: &str) {
    let values = [
        form.name.as_str().to_owned(),
        form.host.as_str().to_owned(),
        form.port.as_str().to_owned(),
        form.database.as_str().to_owned(),
        form.username.as_str().to_owned(),
        "•".repeat(form.password.chars().count()),
        if form.tls_enabled {
            "Enabled".to_owned()
        } else {
            "Disabled".to_owned()
        },
    ];
    let lines = CONNECTION_FIELDS
        .iter()
        .zip(values)
        .enumerate()
        .map(|(index, (label, value))| {
            let marker = if index == form.focus { "›" } else { " " };
            let style = if index == form.focus {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(format!("{marker} {label}: {value}")).style(style)
        })
        .collect::<Vec<_>>();
    let title = if form.id.is_some() {
        " Edit connection "
    } else {
        " New connection "
    };
    let panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)])
        .split(area);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title(title)),
        panel[0],
    );
    frame.render_widget(Paragraph::new(message), panel[1]);
}

async fn run_operation(
    client: &mut ApiClient,
    operation: Operation,
    email: &str,
    password: &str,
    input: Option<&ConnectionInput>,
) -> OperationResult {
    match operation {
        Operation::Login => {
            let result = async {
                let login = client.login(email, password).await?;
                let session = client.current_session().await?;
                if login.user_id != session.user_id {
                    return Err(ApiError::InvalidResponse);
                }
                Ok(login)
            }
            .await;
            OperationResult::Login(result)
        }
        Operation::LoadConnections { page } => OperationResult::Connections(
            client
                .list_connections(page, 100)
                .await
                .map(|page| page.items),
        ),
        Operation::LoadDatabases { connection_id } => OperationResult::Databases(
            client
                .list_databases(connection_id)
                .await
                .map(|databases| databases.items),
        ),
        Operation::SaveConnection { id: Some(id) } => {
            let Some(input) = input else {
                return OperationResult::Saved(Err(ApiError::InvalidResponse));
            };
            OperationResult::Saved(client.update_connection(id, input).await)
        }
        Operation::SaveConnection { id: None } => {
            let Some(input) = input else {
                return OperationResult::Saved(Err(ApiError::InvalidResponse));
            };
            OperationResult::Saved(client.create_connection(input).await)
        }
        Operation::TestConnection { id } => {
            OperationResult::Tested(client.test_connection(id).await)
        }
        Operation::DeleteConnection { id } => {
            OperationResult::Deleted(client.delete_connection(id).await)
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pgrustadmin_api_client::{ApiClient, AuthSession, Connection, Database};
    use ratatui::{Terminal, backend::TestBackend};
    use uuid::Uuid;

    use super::{App, ConnectionForm, LoginField, ScreenState, UiAction, render};

    fn client() -> ApiClient {
        ApiClient::new("http://127.0.0.1:3000").expect("valid API URL")
    }

    fn authenticated_app() -> App {
        let mut app = App::new(client());
        app.state = ScreenState::Authenticated(AuthSession {
            user_id: Uuid::new_v4(),
            expires_in: 43200,
        });
        app
    }

    #[test]
    fn login_form_masks_password_and_supports_field_navigation() {
        let mut app = App::new(client());
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(
            app.state,
            ScreenState::EditingLogin {
                focus: LoginField::Password,
                ..
            }
        ));
        for character in "secret".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("create test terminal");
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
    fn connection_form_requires_create_password_and_masks_it() {
        let mut form = ConnectionForm::create();
        form.name = "Local".to_owned();
        form.host = "localhost".to_owned();
        form.database = "app".to_owned();
        form.username = "app".to_owned();
        assert!(form.input().is_err());
        form.password.push_str("secret-db-password");
        assert!(form.input().is_ok());
        let mut app = authenticated_app();
        app.form = Some(form);
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(
            app.form
                .as_ref()
                .is_some_and(|form| form.name.ends_with('q'))
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render connection form");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Password"));
        assert!(rendered.contains('•'));
        assert!(!rendered.contains("secret-db-password"));
    }

    #[test]
    fn editing_connection_without_password_preserves_credential() {
        let connection = Connection {
            id: Uuid::new_v4(),
            name: "local".to_owned(),
            host: "localhost".to_owned(),
            port: 5432,
            database: "app".to_owned(),
            username: "app".to_owned(),
            tls_enabled: true,
        };
        let form = ConnectionForm::edit(&connection);
        let input = form.input().expect("editing without password is valid");
        assert!(input.password.is_none());
    }

    #[test]
    fn connection_actions_select_test_and_require_delete_confirmation() {
        let mut app = authenticated_app();
        app.view = 1;
        app.connections.push(Connection {
            id: Uuid::new_v4(),
            name: "local".to_owned(),
            host: "localhost".to_owned(),
            port: 5432,
            database: "app".to_owned(),
            username: "app".to_owned(),
            tls_enabled: true,
        });
        let id = app.connections[0].id;
        assert!(
            matches!(app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)), UiAction::TestConnection(test_id) if test_id == id)
        );
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            UiAction::None
        ));
        assert_eq!(app.confirm_delete, Some(id));
        assert!(
            matches!(app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)), UiAction::DeleteConnection(delete_id) if delete_id == id)
        );
    }

    #[test]
    fn selected_connection_opens_database_list_and_escape_returns() {
        let mut app = authenticated_app();
        app.view = 1;
        let connection = Connection {
            id: Uuid::new_v4(),
            name: "local".to_owned(),
            host: "localhost".to_owned(),
            port: 5432,
            database: "app".to_owned(),
            username: "app".to_owned(),
            tls_enabled: true,
        };
        app.connections.push(connection.clone());
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
            UiAction::LoadDatabases(connection_id) if connection_id == connection.id
        ));
        assert_eq!(app.view, 2);
        app.databases = vec![
            Database {
                name: "alpha".to_owned(),
            },
            Database {
                name: "beta".to_owned(),
            },
        ];
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected_database, 1);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.view, 1);

        let other = Connection {
            id: Uuid::new_v4(),
            name: "other".to_owned(),
            ..connection
        };
        app.connections = vec![other.clone()];
        app.selected_connection = 0;
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
            UiAction::LoadDatabases(connection_id) if connection_id == other.id
        ));
        assert!(app.databases.is_empty());
        assert_eq!(app.selected_database, 0);
    }

    #[test]
    fn connections_page_navigation_uses_full_page_as_next_page_signal() {
        let mut app = authenticated_app();
        app.view = 1;
        app.connections = (0..100)
            .map(|_| Connection {
                id: Uuid::new_v4(),
                name: "connection".to_owned(),
                host: "localhost".to_owned(),
                port: 5432,
                database: "app".to_owned(),
                username: "app".to_owned(),
                tls_enabled: true,
            })
            .collect();
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
            UiAction::LoadConnections(2)
        ));
        assert_eq!(app.connection_page, 2);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE)),
            UiAction::LoadConnections(1)
        ));
    }
}
