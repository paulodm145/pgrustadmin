mod sql_editor;
mod sql_tabs;

use std::{io, time::Duration};

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
};
use futures_util::StreamExt;
use pgrustadmin_api_client::{
    ApiClient, ApiError, AuthSession, Column, ColumnGenerated, ColumnIdentity, Connection,
    ConnectionInput, ConnectionTest, Database, Index, Relation, RelationKind, RelationRef, Schema,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
};
use sql_tabs::{MAX_SQL_TABS, SqlTab, SqlTabs};
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

const RELATION_PAGE_SIZE: usize = 100;
/// View index of the SQL tabs section; catalog views use 2..=5.
const SQL_VIEW: usize = 6;
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
    let result = match execute!(io::stdout(), EnableBracketedPaste) {
        Ok(()) => run_app(&mut terminal, client).await,
        Err(error) => Err(error),
    };
    let disable_paste = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::try_restore()?;
    result.and(disable_paste)
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
    LoadSchemas { connection_id: Uuid },
    LoadRelations { connection_id: Uuid, page: u64 },
    LoadRelationDetail { connection_id: Uuid },
    SaveConnection { id: Option<Uuid> },
    TestConnection { id: Uuid },
    DeleteConnection { id: Uuid },
}

enum OperationResult {
    Login(Result<AuthSession, ApiError>),
    Connections(Result<Vec<Connection>, ApiError>),
    Databases(Result<Vec<Database>, ApiError>),
    Schemas(Result<Vec<Schema>, ApiError>),
    Relations(Result<Vec<Relation>, ApiError>),
    RelationDetail(Result<(Vec<Column>, Vec<Index>), ApiError>),
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
    LoadSchemas(Uuid),
    LoadRelations(Uuid, u64),
    LoadRelationDetail(Uuid),
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
    catalog_database: String,
    schemas: Vec<Schema>,
    selected_schema: usize,
    catalog_schema: String,
    relations: Vec<Relation>,
    catalog_relation: String,
    columns: Vec<Column>,
    indexes: Vec<Index>,
    selected_column: usize,
    sql_tabs: SqlTabs,
    sql_editing: bool,
    confirm_close_tab: bool,
    confirm_quit: bool,
    selected_relation: usize,
    relation_page: u64,
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
            catalog_database: String::new(),
            schemas: Vec::new(),
            selected_schema: 0,
            catalog_schema: String::new(),
            relations: Vec::new(),
            selected_relation: 0,
            relation_page: 1,
            catalog_relation: String::new(),
            columns: Vec::new(),
            indexes: Vec::new(),
            selected_column: 0,
            sql_tabs: SqlTabs::default(),
            sql_editing: false,
            confirm_close_tab: false,
            confirm_quit: false,
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
        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if self.confirm_quit {
            match key.code {
                _ if ctrl_c => self.should_quit = true,
                KeyCode::Char('y') | KeyCode::Char('Y') => self.should_quit = true,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_quit = false;
                    self.message = "Quit cancelled.".to_owned();
                }
                _ => {}
            }
            return UiAction::None;
        }
        if ctrl_c {
            self.request_quit();
            return UiAction::None;
        }
        if self.confirm_close_tab {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_close_tab = false;
                    self.close_sql_tab();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_close_tab = false;
                    self.message = "Tab kept open.".to_owned();
                }
                _ => {}
            }
            return UiAction::None;
        }
        if self.sql_editing {
            self.handle_editor_key(key);
            return UiAction::None;
        }
        if key.code == KeyCode::Esc
            && self.view == SQL_VIEW
            && self.form.is_none()
            && matches!(self.state, ScreenState::Authenticated(_))
        {
            self.view = 1;
            return UiAction::None;
        }
        if key.code == KeyCode::Esc
            && matches!(self.view, 2..=5)
            && self.form.is_none()
            && matches!(self.state, ScreenState::Authenticated(_))
        {
            self.view -= 1;
            return UiAction::None;
        }
        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
            && !matches!(self.state, ScreenState::EditingLogin { .. })
            && self.form.is_none()
        {
            if self.form.take().is_some() || self.confirm_delete.take().is_some() {
                return UiAction::None;
            }
            self.request_quit();
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
                if self.view == SQL_VIEW {
                    match key.code {
                        KeyCode::Char(']') => self.sql_tabs.select_next(),
                        KeyCode::Char('[') => self.sql_tabs.select_previous(),
                        KeyCode::Enter | KeyCode::Char('i') if self.sql_tabs.active().is_some() => {
                            self.sql_editing = true;
                        }
                        KeyCode::Char('x') => {
                            if self.sql_tabs.active().is_some_and(SqlTab::is_dirty) {
                                self.confirm_close_tab = true;
                            } else {
                                self.close_sql_tab();
                            }
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 1,
                        _ => {}
                    }
                } else if self.view == 5 {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') if !self.columns.is_empty() => {
                            self.selected_column = (self.selected_column + self.columns.len() - 1)
                                % self.columns.len();
                        }
                        KeyCode::Down | KeyCode::Char('j') if !self.columns.is_empty() => {
                            self.selected_column = (self.selected_column + 1) % self.columns.len();
                        }
                        KeyCode::Char('r') => {
                            if let Some(connection_id) = self.catalog_connection {
                                return UiAction::LoadRelationDetail(connection_id);
                            }
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 4,
                        _ => {}
                    }
                } else if self.view == 4 {
                    let Some(connection_id) = self.catalog_connection else {
                        return UiAction::None;
                    };
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') if !self.relations.is_empty() => {
                            self.selected_relation =
                                (self.selected_relation + self.relations.len() - 1)
                                    % self.relations.len();
                        }
                        KeyCode::Down | KeyCode::Char('j') if !self.relations.is_empty() => {
                            self.selected_relation =
                                (self.selected_relation + 1) % self.relations.len();
                        }
                        KeyCode::Char('r') => {
                            return UiAction::LoadRelations(connection_id, self.relation_page);
                        }
                        KeyCode::Char('[') if self.relation_page > 1 => {
                            self.relation_page -= 1;
                            self.selected_relation = 0;
                            return UiAction::LoadRelations(connection_id, self.relation_page);
                        }
                        KeyCode::Char(']') if self.relations.len() == RELATION_PAGE_SIZE => {
                            self.relation_page += 1;
                            self.selected_relation = 0;
                            return UiAction::LoadRelations(connection_id, self.relation_page);
                        }
                        KeyCode::Enter => {
                            if let Some(relation) = self.relations.get(self.selected_relation) {
                                self.catalog_relation = relation.name.clone();
                                self.columns.clear();
                                self.indexes.clear();
                                self.selected_column = 0;
                                self.view = 5;
                                return UiAction::LoadRelationDetail(connection_id);
                            }
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 3,
                        _ => {}
                    }
                } else if self.view == 3 {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') if !self.schemas.is_empty() => {
                            self.selected_schema = (self.selected_schema + self.schemas.len() - 1)
                                % self.schemas.len();
                        }
                        KeyCode::Down | KeyCode::Char('j') if !self.schemas.is_empty() => {
                            self.selected_schema = (self.selected_schema + 1) % self.schemas.len();
                        }
                        KeyCode::Char('r') => {
                            if let Some(connection_id) = self.catalog_connection {
                                return UiAction::LoadSchemas(connection_id);
                            }
                        }
                        KeyCode::Enter => {
                            if let (Some(connection_id), Some(schema)) = (
                                self.catalog_connection,
                                self.schemas.get(self.selected_schema),
                            ) {
                                self.catalog_schema = schema.name.clone();
                                self.relations.clear();
                                self.selected_relation = 0;
                                self.relation_page = 1;
                                self.view = 4;
                                return UiAction::LoadRelations(connection_id, 1);
                            }
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 2,
                        _ => {}
                    }
                } else if self.view == 2 {
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
                        KeyCode::Enter => {
                            if let (Some(connection_id), Some(database)) = (
                                self.catalog_connection,
                                self.databases.get(self.selected_database),
                            ) {
                                self.catalog_database = database.name.clone();
                                self.schemas.clear();
                                self.selected_schema = 0;
                                self.view = 3;
                                return UiAction::LoadSchemas(connection_id);
                            }
                        }
                        KeyCode::Char('s') => self.open_sql_tab(),
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
                        KeyCode::Char('s') => self.show_sql_tabs(),
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.view = 0,
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                            self.view = 1
                        }
                        KeyCode::Char('s') => self.show_sql_tabs(),
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

    fn request_quit(&mut self) {
        let dirty = self.sql_tabs.dirty_count();
        if dirty == 0 {
            self.should_quit = true;
        } else {
            self.confirm_quit = true;
            self.message = format!("{dirty} SQL tab(s) have unsaved changes.");
        }
    }

    fn close_sql_tab(&mut self) {
        self.sql_tabs.close_active();
        if self.sql_tabs.is_empty() {
            self.view = 1;
            self.message = "All SQL tabs are closed.".to_owned();
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        let Some(tab) = self.sql_tabs.active_mut().filter(|_| self.view == SQL_VIEW) else {
            self.sql_editing = false;
            return;
        };
        let editor = &mut tab.editor;
        match key.code {
            KeyCode::Esc => self.sql_editing = false,
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                editor.insert_char(character);
            }
            KeyCode::Enter => editor.new_line(),
            KeyCode::Tab => editor.insert_tab(),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => editor.move_left(),
            KeyCode::Right => editor.move_right(),
            KeyCode::Up => editor.move_up(),
            KeyCode::Down => editor.move_down(),
            KeyCode::Home => editor.move_home(),
            KeyCode::End => editor.move_end(),
            KeyCode::PageUp => editor.page_up(),
            KeyCode::PageDown => editor.page_down(),
            _ => {}
        }
    }

    /// Inserts bracketed-paste text into the SQL editor, or types it into the
    /// focused login/connection field as before bracketed paste was enabled.
    fn handle_paste(&mut self, text: &str) {
        if self.confirm_quit || self.confirm_close_tab {
            return;
        }
        if self.sql_editing {
            if let Some(tab) = self.sql_tabs.active_mut() {
                tab.editor.insert_str(text);
            }
            return;
        }
        if self.form.is_some() || matches!(self.state, ScreenState::EditingLogin { .. }) {
            for character in text.chars().filter(|character| !character.is_control()) {
                self.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
            }
        }
    }

    fn open_sql_tab(&mut self) {
        let (Some(connection_id), Some(database)) = (
            self.catalog_connection,
            self.databases.get(self.selected_database),
        ) else {
            return;
        };
        match self
            .sql_tabs
            .open(connection_id, &self.catalog_connection_name, &database.name)
        {
            Ok(_) => {
                self.view = SQL_VIEW;
                self.message.clear();
            }
            Err(_) => {
                self.message =
                    format!("Close a SQL tab before opening another (limit {MAX_SQL_TABS}).");
            }
        }
    }

    fn show_sql_tabs(&mut self) {
        if self.sql_tabs.is_empty() {
            self.message = "No SQL tabs open. Press s on a database to open one.".to_owned();
        } else {
            self.view = SQL_VIEW;
        }
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
            UiAction::LoadSchemas(_) => self.message = "Loading schemas…".to_owned(),
            UiAction::LoadRelations(..) => self.message = "Loading relations…".to_owned(),
            UiAction::LoadRelationDetail(_) => {
                self.message = "Loading columns and indexes…".to_owned();
            }
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
        let catalog = matches!(
            operation,
            Operation::LoadSchemas { .. }
                | Operation::LoadRelations { .. }
                | Operation::LoadRelationDetail { .. }
        )
        .then(|| CatalogPath {
            database: self.catalog_database.clone(),
            schema: self.catalog_schema.clone(),
            relation: self.catalog_relation.clone(),
        })
        .unwrap_or_default();
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
                &catalog,
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
                    OperationResult::Schemas(Ok(schemas)) => {
                        self.schemas = schemas;
                        self.selected_schema = self
                            .selected_schema
                            .min(self.schemas.len().saturating_sub(1));
                        self.message = format!("Loaded {} schema(s).", self.schemas.len());
                    }
                    OperationResult::Relations(Ok(relations)) => {
                        self.relations = relations;
                        self.selected_relation = self
                            .selected_relation
                            .min(self.relations.len().saturating_sub(1));
                        self.message = format!("Loaded {} relation(s).", self.relations.len());
                    }
                    OperationResult::RelationDetail(Ok((columns, indexes))) => {
                        self.columns = columns;
                        self.indexes = indexes;
                        self.selected_column = self
                            .selected_column
                            .min(self.columns.len().saturating_sub(1));
                        self.message = format!(
                            "Loaded {} column(s) and {} index(es).",
                            self.columns.len(),
                            self.indexes.len()
                        );
                    }
                    OperationResult::RelationDetail(Err(error)) => {
                        self.columns.clear();
                        self.indexes.clear();
                        self.selected_column = 0;
                        self.message = api_error_message(error).to_owned();
                    }
                    OperationResult::Relations(Err(error)) => {
                        self.relations.clear();
                        self.selected_relation = 0;
                        self.message = api_error_message(error).to_owned();
                    }
                    OperationResult::Schemas(Err(error)) => {
                        self.schemas.clear();
                        self.selected_schema = 0;
                        self.message = api_error_message(error).to_owned();
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
        } => "Could not read the catalog from this connection.",
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
                    Some(Ok(Event::Paste(text))) => app.handle_paste(&text),
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
        UiAction::LoadSchemas(connection_id) => Operation::LoadSchemas { connection_id },
        UiAction::LoadRelationDetail(connection_id) => {
            Operation::LoadRelationDetail { connection_id }
        }
        UiAction::LoadRelations(connection_id, page) => Operation::LoadRelations {
            connection_id,
            page,
        },
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
                let items = app
                    .databases
                    .iter()
                    .map(|database| ListItem::new(database.name.as_str()))
                    .collect();
                let title = format!(" Databases — {} ", app.catalog_connection_name);
                render_catalog_list(
                    frame,
                    sections[1],
                    &title,
                    items,
                    app.selected_database,
                    &app.message,
                );
            } else if app.view == 3 {
                let items = app
                    .schemas
                    .iter()
                    .map(|schema| ListItem::new(schema.name.as_str()))
                    .collect();
                let title = format!(
                    " Schemas — {} / {} ",
                    app.catalog_connection_name, app.catalog_database
                );
                render_catalog_list(
                    frame,
                    sections[1],
                    &title,
                    items,
                    app.selected_schema,
                    &app.message,
                );
            } else if app.view == SQL_VIEW {
                render_sql_tabs(frame, sections[1], app);
            } else if app.view == 5 {
                render_relation_detail(frame, sections[1], app);
            } else if app.view == 4 {
                let items = app
                    .relations
                    .iter()
                    .map(|relation| {
                        ListItem::new(Line::from(vec![
                            Span::raw(relation.name.as_str()),
                            Span::styled(
                                format!("  {}", relation_kind_label(relation.kind)),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]))
                    })
                    .collect();
                let title = format!(
                    " Relations — {} / {} / {} · page {} ",
                    app.catalog_connection_name,
                    app.catalog_database,
                    app.catalog_schema,
                    app.relation_page
                );
                render_catalog_list(
                    frame,
                    sections[1],
                    &title,
                    items,
                    app.selected_relation,
                    &app.message,
                );
            } else {
                render_workspace(frame, sections[1], app, session);
            }
        }
        state => render_login(frame, sections[1], app, state),
    }

    let footer_text = if app.confirm_quit {
        "Unsaved SQL changes will be lost. Quit anyway? y: quit   n/Esc: cancel"
    } else if app.confirm_close_tab {
        "Close tab with unsaved changes? y: close   n/Esc: cancel"
    } else if app.sql_editing {
        "EDIT   Esc: commands   ←↑↓→ Home End PgUp PgDn: move   Ctrl+C: quit"
    } else if app.pending.is_some() {
        "Request in progress…   q/Esc: quit"
    } else if app.form.is_some() {
        "Tab: next field   Enter: save   Esc: cancel"
    } else if app.confirm_delete.is_some() {
        "Delete this connection? y: confirm   n/Esc: cancel"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 1 {
        "n: new   e: edit   t: test   c: databases   d: delete   s: SQL   [ / ]: pages   r: reload   Tab: overview   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == SQL_VIEW {
        "Enter/i: edit   [ / ]: switch tab   x: close tab   Tab/Esc: connections   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 2 {
        "↑/↓: navigate   Enter: schemas   s: new SQL tab   r: reload   Tab/Esc: connections   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 3 {
        "↑/↓: navigate   Enter: relations   r: reload   Tab/Esc: databases   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 4 {
        "↑/↓: navigate   Enter: details   [ / ]: pages   r: reload   Tab/Esc: schemas   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 5 {
        "↑/↓: navigate columns   r: reload   Tab/Esc: relations   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) {
        "Tab/Enter: open Connections   s: SQL tabs   ↑/↓: select   q/Esc: quit"
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
        Line::from("Press s to show open SQL tabs."),
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

fn relation_kind_label(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Table => "table",
        RelationKind::PartitionedTable => "partitioned table",
        RelationKind::View => "view",
        RelationKind::MaterializedView => "materialized view",
        RelationKind::ForeignTable => "foreign table",
    }
}

fn column_line(column: &Column) -> Line<'_> {
    let mut details = Vec::new();
    if !column.nullable {
        details.push("not null".to_owned());
    }
    if column.primary_key {
        details.push("primary key".to_owned());
    }
    match column.identity {
        Some(ColumnIdentity::Always) => details.push("identity always".to_owned()),
        Some(ColumnIdentity::ByDefault) => details.push("identity by default".to_owned()),
        None => {}
    }
    match column.generated {
        Some(ColumnGenerated::Stored) => details.push("generated stored".to_owned()),
        Some(ColumnGenerated::Virtual) => details.push("generated virtual".to_owned()),
        None => {}
    }
    if let Some(default) = &column.default {
        details.push(format!("default {default}"));
    }
    Line::from(vec![
        Span::raw(column.name.as_str()),
        Span::styled(
            format!("  {}", column.data_type),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            format!("  {}", details.join(", ")),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn index_line(index: &Index) -> Line<'_> {
    let mut flags = Vec::new();
    if index.primary {
        flags.push("primary");
    }
    if index.unique {
        flags.push("unique");
    }
    Line::from(vec![
        Span::raw(index.name.as_str()),
        Span::styled(
            format!("  {} ({})", index.method, index.columns.join(", ")),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            format!("  {}", flags.join(", ")),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn render_sql_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    let titles = app.sql_tabs.tabs().iter().map(|tab| {
        let marker = if tab.is_dirty() { " *" } else { "" };
        Line::from(format!(
            "{} {}@{}{marker}",
            tab.id.get(),
            tab.database,
            tab.connection_name
        ))
    });
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(" SQL tabs "))
        .select(app.sql_tabs.active_index())
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        );
    frame.render_widget(tabs, panes[0]);

    let Some(tab) = app.sql_tabs.active() else {
        frame.render_widget(Paragraph::new(app.message.as_str()), panes[1]);
        return;
    };
    let mode = if app.sql_editing { " [EDIT]" } else { "" };
    let title = format!(
        " {} — {} / {}{mode} ",
        tab.file.as_deref().unwrap_or("untitled"),
        tab.connection_name,
        tab.database
    );
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(panes[1]);
    frame.render_widget(block, panes[1]);

    let editor = &tab.editor;
    if !app.sql_editing && editor.text().is_empty() {
        frame.render_widget(
            Paragraph::new("No SQL yet. Press Enter to edit.")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let height = usize::from(inner.height);
    let width = usize::from(inner.width);
    let (scroll_row, scroll_col) = editor.viewport_offset(height, width);
    let lines: Vec<Line> = editor
        .lines()
        .iter()
        .skip(scroll_row)
        .take(height)
        .map(|line| {
            Line::from(
                line.chars()
                    .skip(scroll_col)
                    .take(width)
                    .collect::<String>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if app.sql_editing {
        let (row, col) = editor.cursor();
        let x = u16::try_from(col - scroll_col).unwrap_or(inner.width);
        let y = u16::try_from(row - scroll_row).unwrap_or(inner.height);
        frame.set_cursor_position((inner.x.saturating_add(x), inner.y.saturating_add(y)));
    }
}

fn render_relation_detail(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);
    let title = format!(
        " Columns — {} / {} / {}.{} ",
        app.catalog_connection_name, app.catalog_database, app.catalog_schema, app.catalog_relation
    );
    let items = app
        .columns
        .iter()
        .map(|column| ListItem::new(column_line(column)))
        .collect();
    render_catalog_list(
        frame,
        panes[0],
        &title,
        items,
        app.selected_column,
        &app.message,
    );
    let block = Block::default().borders(Borders::ALL).title(" Indexes ");
    if app.indexes.is_empty() {
        let text = if app.columns.is_empty() {
            ""
        } else {
            "No indexes."
        };
        frame.render_widget(Paragraph::new(text).block(block), panes[1]);
    } else {
        let items: Vec<_> = app
            .indexes
            .iter()
            .map(|index| ListItem::new(index_line(index)))
            .collect();
        frame.render_widget(List::new(items).block(block), panes[1]);
    }
}

fn render_catalog_list(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    items: Vec<ListItem<'_>>,
    selected: usize,
    message: &str,
) {
    let is_empty = items.is_empty();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default().with_selected((!is_empty).then_some(selected));
    frame.render_stateful_widget(list, area, &mut list_state);
    if is_empty {
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(Color::Yellow)),
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

/// Catalog names selected in the TUI, sent with catalog operations.
#[derive(Default)]
struct CatalogPath {
    database: String,
    schema: String,
    relation: String,
}

async fn run_operation(
    client: &mut ApiClient,
    operation: Operation,
    email: &str,
    password: &str,
    catalog: &CatalogPath,
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
        Operation::LoadRelations {
            connection_id,
            page,
        } => OperationResult::Relations(
            client
                .list_relations(
                    connection_id,
                    &catalog.database,
                    &catalog.schema,
                    page,
                    RELATION_PAGE_SIZE as u64,
                )
                .await
                .map(|relations| relations.items),
        ),
        Operation::LoadRelationDetail { connection_id } => {
            let relation = RelationRef {
                connection_id,
                database: &catalog.database,
                schema: &catalog.schema,
                relation: &catalog.relation,
            };
            let result = async {
                let columns = client.list_columns(relation).await?.items;
                let indexes = client.list_indexes(relation).await?.items;
                Ok((columns, indexes))
            }
            .await;
            OperationResult::RelationDetail(result)
        }
        Operation::LoadSchemas { connection_id } => OperationResult::Schemas(
            client
                .list_schemas(connection_id, &catalog.database)
                .await
                .map(|schemas| schemas.items),
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
    use pgrustadmin_api_client::{
        ApiClient, AuthSession, Column, Connection, Database, Index, Relation, RelationKind, Schema,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use uuid::Uuid;

    use super::{
        App, ConnectionForm, LoginField, MAX_SQL_TABS, SQL_VIEW, ScreenState, UiAction, render,
    };

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
    fn selected_database_opens_schema_list_and_escape_returns() {
        let mut app = authenticated_app();
        let connection_id = Uuid::new_v4();
        app.view = 2;
        app.catalog_connection = Some(connection_id);
        app.databases = vec![
            Database {
                name: "alpha".to_owned(),
            },
            Database {
                name: "beta".to_owned(),
            },
        ];
        app.selected_database = 1;
        app.schemas = vec![Schema {
            name: "stale".to_owned(),
        }];
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            UiAction::LoadSchemas(id) if id == connection_id
        ));
        assert_eq!(app.view, 3);
        assert_eq!(app.catalog_database, "beta");
        assert!(app.schemas.is_empty());

        app.schemas = vec![
            Schema {
                name: "pg_catalog".to_owned(),
            },
            Schema {
                name: "public".to_owned(),
            },
        ];
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.selected_schema, 1);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            UiAction::LoadSchemas(id) if id == connection_id
        ));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.view, 2);
        assert_eq!(app.selected_database, 1);
    }

    #[test]
    fn selected_schema_opens_paged_relation_list_and_escape_returns() {
        let mut app = authenticated_app();
        let connection_id = Uuid::new_v4();
        app.view = 3;
        app.catalog_connection = Some(connection_id);
        app.schemas = vec![Schema {
            name: "public".to_owned(),
        }];
        app.relations = vec![Relation {
            name: "stale".to_owned(),
            kind: RelationKind::Table,
        }];
        app.relation_page = 4;
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            UiAction::LoadRelations(id, 1) if id == connection_id
        ));
        assert_eq!(app.view, 4);
        assert_eq!(app.catalog_schema, "public");
        assert!(app.relations.is_empty());

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
            UiAction::None
        ));
        app.relations = (0..100)
            .map(|index| Relation {
                name: format!("table_{index:03}"),
                kind: RelationKind::View,
            })
            .collect();
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
            UiAction::LoadRelations(id, 2) if id == connection_id
        ));
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE)),
            UiAction::LoadRelations(id, 1) if id == connection_id
        ));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.view, 3);
    }

    #[test]
    fn selected_relation_opens_details_and_escape_returns() {
        let mut app = authenticated_app();
        let connection_id = Uuid::new_v4();
        app.view = 4;
        app.catalog_connection = Some(connection_id);
        app.relations = vec![
            Relation {
                name: "accounts".to_owned(),
                kind: RelationKind::Table,
            },
            Relation {
                name: "users".to_owned(),
                kind: RelationKind::Table,
            },
        ];
        app.selected_relation = 1;
        app.columns = vec![column("stale")];
        app.selected_column = 3;
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            UiAction::LoadRelationDetail(id) if id == connection_id
        ));
        assert_eq!(app.view, 5);
        assert_eq!(app.catalog_relation, "users");
        assert!(app.columns.is_empty());
        assert!(app.indexes.is_empty());
        assert_eq!(app.selected_column, 0);

        app.columns = vec![column("id"), column("email")];
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected_column, 1);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            UiAction::LoadRelationDetail(id) if id == connection_id
        ));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.view, 4);
        assert_eq!(app.selected_relation, 1);
    }

    #[test]
    fn relation_detail_renders_columns_and_indexes() {
        let mut app = authenticated_app();
        app.view = 5;
        app.catalog_relation = "users".to_owned();
        app.columns = vec![Column {
            primary_key: true,
            nullable: false,
            ..column("id")
        }];
        app.indexes = vec![Index {
            name: "users_pkey".to_owned(),
            method: "btree".to_owned(),
            unique: true,
            primary: true,
            columns: vec!["id".to_owned()],
        }];
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render relation detail");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Columns"));
        assert!(rendered.contains("integer  not null, primary key"));
        assert!(rendered.contains("users_pkey  btree (id)  primary, unique"));
    }

    #[test]
    fn database_list_opens_sql_tabs_and_sql_view_manages_them() {
        let mut app = authenticated_app();
        let connection_id = Uuid::new_v4();
        app.view = 2;
        app.catalog_connection = Some(connection_id);
        app.catalog_connection_name = "local".to_owned();
        app.databases = vec![
            Database {
                name: "app".to_owned(),
            },
            Database {
                name: "analytics".to_owned(),
            },
        ];
        app.selected_database = 1;

        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.view, SQL_VIEW);
        let active = app.sql_tabs.active().expect("tab opened");
        assert_eq!(active.connection_id, connection_id);
        assert_eq!(active.database, "analytics");

        app.view = 2;
        app.selected_database = 0;
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.sql_tabs.tabs().len(), 2);
        assert_eq!(app.sql_tabs.active_index(), 1);
        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert_eq!(app.sql_tabs.active_index(), 0);
        app.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
        assert_eq!(app.sql_tabs.active_index(), 1);

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render sql tabs");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("1 analytics@local"));
        assert!(rendered.contains("2 app@local"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.view, 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.view, SQL_VIEW);

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.sql_tabs.tabs().len(), 1);
        assert_eq!(app.view, SQL_VIEW);
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(app.sql_tabs.is_empty());
        assert_eq!(app.view, 1);

        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.view, 1);
    }

    #[test]
    fn opening_sql_tab_beyond_limit_keeps_existing_tabs() {
        let mut app = authenticated_app();
        let connection_id = Uuid::new_v4();
        for _ in 0..MAX_SQL_TABS {
            app.sql_tabs
                .open(connection_id, "local", "app")
                .expect("open tab");
        }
        app.view = 2;
        app.catalog_connection = Some(connection_id);
        app.databases = vec![Database {
            name: "app".to_owned(),
        }];
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.view, 2);
        assert_eq!(app.sql_tabs.tabs().len(), MAX_SQL_TABS);
        assert!(app.message.contains("20"));
    }

    fn app_with_sql_tab() -> App {
        let mut app = authenticated_app();
        app.sql_tabs
            .open(Uuid::new_v4(), "local", "app")
            .expect("open tab");
        app.view = SQL_VIEW;
        app
    }

    fn press(app: &mut App, code: KeyCode) -> UiAction {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn sql_view_edit_mode_types_into_active_tab_and_escape_leaves_it() {
        let mut app = app_with_sql_tab();
        press(&mut app, KeyCode::Char('x'));
        assert!(app.sql_tabs.is_empty(), "clean tab closes without asking");

        let mut app = app_with_sql_tab();
        press(&mut app, KeyCode::Enter);
        assert!(app.sql_editing);
        for character in "select 1".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('q'));
        let tab = app.sql_tabs.active().expect("active tab");
        assert_eq!(tab.editor.text(), "select 1\n    xq");
        assert!(tab.is_dirty());
        assert!(!app.should_quit);

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render sql editor");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("select 1"));
        assert!(rendered.contains("1 app@local *"));
        assert!(rendered.contains("EDIT"));

        press(&mut app, KeyCode::Esc);
        assert!(!app.sql_editing);
        assert_eq!(app.view, SQL_VIEW);
        press(&mut app, KeyCode::Char('i'));
        assert!(app.sql_editing);
    }

    #[test]
    fn closing_dirty_sql_tab_requires_confirmation() {
        let mut app = app_with_sql_tab();
        app.sql_tabs
            .active_mut()
            .expect("active tab")
            .editor
            .insert_str("select 1");

        press(&mut app, KeyCode::Char('x'));
        assert!(app.confirm_close_tab);
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.confirm_close_tab);
        assert_eq!(app.sql_tabs.tabs().len(), 1);

        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.sql_tabs.tabs().len(), 1);
        assert_eq!(app.view, SQL_VIEW);

        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.sql_tabs.is_empty());
        assert_eq!(app.view, 1);
    }

    #[test]
    fn quitting_with_dirty_sql_tabs_requires_confirmation() {
        let mut app = app_with_sql_tab();
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit, "no dirty tabs quits immediately");

        let mut app = app_with_sql_tab();
        app.sql_tabs
            .active_mut()
            .expect("active tab")
            .editor
            .insert_str("select 1");
        app.view = 1;
        press(&mut app, KeyCode::Char('q'));
        assert!(app.confirm_quit);
        assert!(!app.should_quit);
        assert!(app.message.contains("1 SQL tab"));
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.confirm_quit);
        assert!(!app.should_quit);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_key(ctrl_c);
        assert!(app.confirm_quit);
        assert!(!app.should_quit);
        app.handle_key(ctrl_c);
        assert!(app.should_quit);

        let mut app = app_with_sql_tab();
        app.sql_tabs
            .active_mut()
            .expect("active tab")
            .editor
            .insert_str("select 1");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.confirm_quit);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.should_quit);
    }

    #[test]
    fn paste_goes_to_sql_editor_or_text_fields_only() {
        let mut app = app_with_sql_tab();
        app.handle_paste("ignored in command mode");
        assert_eq!(app.sql_tabs.active().expect("active tab").editor.text(), "");
        press(&mut app, KeyCode::Enter);
        app.handle_paste("select 1;\r\nselect 2;");
        assert_eq!(
            app.sql_tabs.active().expect("active tab").editor.text(),
            "select 1;\nselect 2;"
        );

        let mut app = App::new(client());
        app.handle_paste("user@example.test\n");
        assert_eq!(app.login_email, "user@example.test");
    }

    fn column(name: &str) -> Column {
        Column {
            name: name.to_owned(),
            position: 1,
            data_type: "integer".to_owned(),
            nullable: true,
            default: None,
            primary_key: false,
            identity: None,
            generated: None,
        }
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
