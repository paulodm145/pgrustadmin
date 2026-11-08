mod grid;
mod sql_editor;
mod sql_tabs;
mod theme;

use std::{io, time::Duration};

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
};
use futures_util::StreamExt;
use grid::{Grid, GridKey, GridRequest};
use pgrustadmin_api_client::{
    ApiClient, ApiError, AuthSession, Column, ColumnGenerated, ColumnIdentity, Connection,
    ConnectionInput, ConnectionTest, Database, Execution, ExecutionRows, ExecutionStatus, Index,
    Relation, RelationKind, RelationRef, RelationRowsPage, RelationRowsRequest, Schema, SqlFile,
    SqlFileContent,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs},
};
use sql_tabs::{MAX_SQL_TABS, SqlTab, SqlTabs, TabExecution, TabFile, TabId};
use theme::{THEME_ENV, Theme, detect_color_mode, parse_theme_name};
use tokio::{sync::oneshot, task::JoinHandle};
use uuid::Uuid;
use zeroize::Zeroizing;

const RELATION_PAGE_SIZE: usize = 100;
const FILE_LIST_SIZE: u64 = 100;
const EXECUTION_POLL_INTERVAL: Duration = Duration::from_millis(300);
/// View index of the SQL tabs section; catalog views use 2..=5.
const SQL_VIEW: usize = 6;
const MENU_ITEMS: [&str; 2] = ["Overview", "Connections"];
const CONNECTION_FIELDS: [&str; 8] = [
    "Name",
    "Host",
    "Port",
    "Database",
    "Username",
    "Password",
    "TLS",
    "Read-only",
];
const TLS_FIELD: usize = 6;
const READ_ONLY_FIELD: usize = 7;

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
    LoadRelationRows { connection_id: Uuid },
    SaveConnection { id: Option<Uuid> },
    TestConnection { id: Uuid },
    DeleteConnection { id: Uuid },
    ListFiles,
    OpenFile { id: Uuid },
    SaveFile { tab: TabId },
    CreateFile { tab: TabId },
    OverwriteFile { tab: TabId },
    CancelExecution { id: Uuid },
}

enum OperationResult {
    Login(Result<AuthSession, ApiError>),
    Connections(Result<Vec<Connection>, ApiError>),
    Databases(Result<Vec<Database>, ApiError>),
    Schemas(Result<Vec<Schema>, ApiError>),
    Relations(Result<Vec<Relation>, ApiError>),
    RelationDetail(Result<(Vec<Column>, Vec<Index>), ApiError>),
    RelationRows(Result<RelationRowsPage, ApiError>),
    Saved(Result<Connection, ApiError>),
    Tested(Result<ConnectionTest, ApiError>),
    Deleted(Result<(), ApiError>),
    Files(Result<Vec<SqlFile>, ApiError>),
    OpenedFile(Result<SqlFileContent, ApiError>),
    SavedFile {
        tab: TabId,
        content: String,
        result: Result<SqlFile, ApiError>,
    },
    CancelRequested(Result<(), ApiError>),
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
    LoadRelationRows(Uuid),
    SaveConnection,
    TestConnection(Uuid),
    DeleteConnection(Uuid),
    ListFiles,
    OpenFile(Uuid),
    SaveFile(TabId),
    CreateFile(TabId),
    OverwriteFile(TabId),
    ExecuteSql(TabId),
    CancelExecution(Uuid),
}

/// A finished execution and, when it succeeded, the request for its rows.
struct FinishedExecution {
    execution: Execution,
    rows: Option<Result<ExecutionRows, ApiError>>,
}

/// An execution followed by a background task for one tab.
struct TrackedExecution {
    tab: TabId,
    handle: JoinHandle<Result<FinishedExecution, ApiError>>,
    /// Delivers the server's execution ID once the start request succeeds.
    id_receiver: oneshot::Receiver<Uuid>,
    id: Option<Uuid>,
}

impl TrackedExecution {
    fn id(&mut self) -> Option<Uuid> {
        if self.id.is_none() {
            self.id = self.id_receiver.try_recv().ok();
        }
        self.id
    }
}

/// Name typed for "save as" on one SQL tab.
struct FilePrompt {
    tab: TabId,
    name: String,
}

struct FilePicker {
    files: Vec<SqlFile>,
    selected: usize,
}

/// Data a file operation sends, captured when the operation starts.
#[derive(Default)]
struct FileRequest {
    file_id: Option<Uuid>,
    name: String,
    content: String,
    expected_sha256: String,
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
    read_only: bool,
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
            read_only: false,
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
            read_only: connection.read_only,
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
            read_only: self.read_only,
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
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == TLS_FIELD => {
                self.tls_enabled = !self.tls_enabled;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if self.focus == READ_ONLY_FIELD =>
            {
                self.read_only = !self.read_only;
            }
            KeyCode::Enter if self.focus == READ_ONLY_FIELD => return true,
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
    file_prompt: Option<FilePrompt>,
    /// Name submitted from the prompt for the pending create operation.
    file_name: String,
    save_conflict: Option<TabId>,
    file_picker: Option<FilePicker>,
    /// Authenticated client copy for execution polling, which runs beside
    /// other API operations.
    execution_client: Option<ApiClient>,
    executions: Vec<TrackedExecution>,
    /// Tab waiting for confirmation before running `EXPLAIN ANALYZE`.
    confirm_explain: Option<TabId>,
    /// Whether the SQL section's arrow keys scroll the active tab's result.
    result_focus: bool,
    /// Data preview of the relation shown in the detail view.
    relation_rows: Option<Grid>,
    relation_grid_focus: bool,
    /// A rows reload was requested while another API call was running.
    relation_rows_stale: bool,
    theme: Theme,
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
            file_prompt: None,
            file_name: String::new(),
            save_conflict: None,
            file_picker: None,
            execution_client: None,
            executions: Vec::new(),
            confirm_explain: None,
            result_focus: false,
            relation_rows: None,
            relation_grid_focus: false,
            relation_rows_stale: false,
            theme: Theme::default(),
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
        if key.code == KeyCode::F(2) {
            self.theme = self.theme.toggled();
            self.message = format!("Theme: {}.", self.theme.name().label());
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
        if let Some(tab) = self.confirm_explain {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_explain = None;
                    return UiAction::ExecuteSql(tab);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_explain = None;
                    self.message = "Execution cancelled.".to_owned();
                }
                _ => {}
            }
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
        if self.file_prompt.is_some() {
            return self.handle_file_prompt_key(key);
        }
        if let Some(tab) = self.save_conflict {
            match key.code {
                KeyCode::Char('o') => {
                    self.save_conflict = None;
                    return UiAction::OverwriteFile(tab);
                }
                KeyCode::Char('a') => {
                    self.save_conflict = None;
                    self.file_prompt = Some(FilePrompt {
                        tab,
                        name: String::new(),
                    });
                }
                KeyCode::Esc => {
                    self.save_conflict = None;
                    self.message = "Save cancelled.".to_owned();
                }
                _ => {}
            }
            return UiAction::None;
        }
        if let Some(picker) = &mut self.file_picker {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    picker.selected =
                        (picker.selected + picker.files.len() - 1) % picker.files.len();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    picker.selected = (picker.selected + 1) % picker.files.len();
                }
                KeyCode::Enter if self.pending.is_none() => {
                    let id = picker.files[picker.selected].id;
                    self.file_picker = None;
                    return UiAction::OpenFile(id);
                }
                KeyCode::Esc => self.file_picker = None,
                _ => {}
            }
            return UiAction::None;
        }
        if self.sql_editing {
            self.handle_editor_key(key);
            return UiAction::None;
        }
        if let Some(action) = self.route_grid_key(key) {
            return action;
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
                            self.result_focus = false;
                        }
                        KeyCode::Char('w') => {
                            if let Some(tab) = self.sql_tabs.active() {
                                if tab.file.is_some() {
                                    return UiAction::SaveFile(tab.id);
                                }
                                self.file_prompt = Some(FilePrompt {
                                    tab: tab.id,
                                    name: String::new(),
                                });
                            }
                        }
                        KeyCode::Char('W') => {
                            if let Some(tab) = self.sql_tabs.active() {
                                self.file_prompt = Some(FilePrompt {
                                    tab: tab.id,
                                    name: String::new(),
                                });
                            }
                        }
                        KeyCode::Char('o') => return UiAction::ListFiles,
                        KeyCode::Char('e') => {
                            if let Some(tab) = self.sql_tabs.active() {
                                if tab.execution == TabExecution::Running {
                                    self.message = "This tab is already running SQL.".to_owned();
                                } else if tab.editor.text().trim().is_empty() {
                                    self.message = "This tab has no SQL to execute.".to_owned();
                                } else if mentions_explain_analyze(&tab.editor.text()) {
                                    self.confirm_explain = Some(tab.id);
                                } else {
                                    return UiAction::ExecuteSql(tab.id);
                                }
                            }
                        }
                        KeyCode::Char('c') => return self.cancel_action(),
                        KeyCode::Char('x') => {
                            if self.sql_tabs.active().is_some_and(SqlTab::is_dirty) {
                                self.confirm_close_tab = true;
                            } else {
                                self.close_sql_tab();
                            }
                        }
                        KeyCode::Tab => self.toggle_result_focus(),
                        KeyCode::Left | KeyCode::Right => self.view = 1,
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
                        KeyCode::Tab if self.relation_rows.is_some() => {
                            self.relation_grid_focus = true;
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
                                self.relation_rows = None;
                                self.relation_grid_focus = false;
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

    fn handle_file_prompt_key(&mut self, key: KeyEvent) -> UiAction {
        let Some(prompt) = &mut self.file_prompt else {
            return UiAction::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.file_prompt = None;
                self.message = "Save cancelled.".to_owned();
            }
            KeyCode::Backspace => {
                prompt.name.pop();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                prompt.name.push(character);
            }
            KeyCode::Enter if self.pending.is_none() && !prompt.name.trim().is_empty() => {
                let mut name = prompt.name.clone();
                if !name.ends_with(".sql") {
                    name.push_str(".sql");
                }
                let tab = prompt.tab;
                self.file_prompt = None;
                self.file_name = name;
                return UiAction::CreateFile(tab);
            }
            _ => {}
        }
        UiAction::None
    }

    fn file_request(&self, operation: Operation) -> FileRequest {
        let tab = match operation {
            Operation::SaveFile { tab }
            | Operation::CreateFile { tab }
            | Operation::OverwriteFile { tab } => tab,
            _ => return FileRequest::default(),
        };
        let Some(tab) = self.sql_tabs.tabs().iter().find(|item| item.id == tab) else {
            return FileRequest::default();
        };
        FileRequest {
            file_id: tab.file.as_ref().map(|file| file.id),
            name: self.file_name.clone(),
            content: tab.editor.text(),
            expected_sha256: tab
                .file
                .as_ref()
                .map(|file| file.sha256.clone())
                .unwrap_or_default(),
        }
    }

    /// Starts the tab's SQL in a task that polls until the execution ends.
    fn start_execution(&mut self, tab_id: TabId) {
        let Some(client) = self.execution_client.clone() else {
            self.message = "Sign in again to execute SQL.".to_owned();
            return;
        };
        let Some(tab) = self.sql_tabs.get_mut(tab_id) else {
            return;
        };
        let sql = tab.editor.text();
        let connection_id = tab.connection_id;
        let database = tab.database.clone();
        tab.execution = TabExecution::Running;
        // An old result must not be mistaken for the one being produced.
        tab.result = None;
        self.result_focus = false;
        self.message.clear();
        let (id_sender, id_receiver) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let started = client
                .start_execution(connection_id, &database, &sql)
                .await?;
            // The TUI may have stopped tracking the tab; nothing to notify then.
            let _ = id_sender.send(started.id);
            loop {
                tokio::time::sleep(EXECUTION_POLL_INTERVAL).await;
                let execution = client.execution(started.id).await?;
                if execution.status == ExecutionStatus::Running {
                    continue;
                }
                let rows = if execution.status == ExecutionStatus::Succeeded {
                    Some(client.execution_rows(started.id).await)
                } else {
                    None
                };
                return Ok(FinishedExecution { execution, rows });
            }
        });
        self.executions.push(TrackedExecution {
            tab: tab_id,
            handle,
            id_receiver,
            id: None,
        });
    }

    async fn complete_executions(&mut self) {
        let (finished, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.executions)
            .into_iter()
            .partition(|tracked| tracked.handle.is_finished());
        self.executions = running;
        for tracked in finished {
            let result = tracked
                .handle
                .await
                .unwrap_or(Err(ApiError::InvalidResponse));
            self.finish_execution(tracked.tab, result);
        }
    }

    fn finish_execution(&mut self, tab_id: TabId, result: Result<FinishedExecution, ApiError>) {
        let Some(tab) = self.sql_tabs.get_mut(tab_id) else {
            return;
        };
        let (execution, rows) = match result {
            Ok(FinishedExecution { execution, rows }) => (execution, rows),
            Err(error) => {
                tab.execution = TabExecution::Failed {
                    code: None,
                    message: execution_error_message(error).to_owned(),
                    duration_ms: None,
                };
                return;
            }
        };
        let duration_ms = execution.duration_ms;
        tab.execution = match execution {
            Execution {
                status: ExecutionStatus::Succeeded,
                result: Some(counts),
                ..
            } => TabExecution::Succeeded {
                counts,
                duration_ms,
            },
            Execution {
                status: ExecutionStatus::Cancelled,
                ..
            } => TabExecution::Cancelled { duration_ms },
            Execution {
                status: ExecutionStatus::TimedOut,
                ..
            } => TabExecution::TimedOut { duration_ms },
            Execution {
                error: Some(error), ..
            } => TabExecution::Failed {
                code: error.code,
                message: error.message,
                duration_ms,
            },
            _ => TabExecution::Failed {
                code: None,
                message: "The API returned an incomplete execution.".to_owned(),
                duration_ms,
            },
        };
        match rows {
            Some(Ok(rows)) => {
                tab.result = Some(Grid::local(rows.columns, rows.rows, rows.total_rows));
            }
            Some(Err(error)) => {
                tab.result = None;
                self.message = format!(
                    "Result rows are unavailable: {}",
                    execution_error_message(error)
                );
            }
            None => tab.result = None,
        }
    }

    fn cancel_action(&mut self) -> UiAction {
        let Some(tab) = self.sql_tabs.active().map(|tab| tab.id) else {
            return UiAction::None;
        };
        let Some(tracked) = self
            .executions
            .iter_mut()
            .find(|tracked| tracked.tab == tab)
        else {
            self.message = "This tab is not running SQL.".to_owned();
            return UiAction::None;
        };
        match tracked.id() {
            Some(id) => UiAction::CancelExecution(id),
            None => {
                self.message = "The execution is still starting. Try again.".to_owned();
                UiAction::None
            }
        }
    }

    /// Sends the key to the focused grid. `None` means the screen handles it.
    fn route_grid_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        if !matches!(self.state, ScreenState::Authenticated(_)) {
            return None;
        }
        if self.view == SQL_VIEW && self.result_focus {
            let grid = self
                .sql_tabs
                .active_mut()
                .and_then(|tab| tab.result.as_mut())?;
            if key.code == KeyCode::Tab {
                self.result_focus = false;
                return Some(UiAction::None);
            }
            return match grid.handle_key(key) {
                GridKey::Ignored => None,
                GridKey::Handled | GridKey::Reload => Some(UiAction::None),
            };
        }
        if self.view == 5 && self.relation_grid_focus {
            let grid = self.relation_rows.as_mut()?;
            if key.code == KeyCode::Tab {
                self.relation_grid_focus = false;
                return Some(UiAction::None);
            }
            return match grid.handle_key(key) {
                GridKey::Ignored => None,
                GridKey::Handled => Some(UiAction::None),
                GridKey::Reload => Some(
                    self.catalog_connection
                        .map_or(UiAction::None, UiAction::LoadRelationRows),
                ),
            };
        }
        None
    }

    fn toggle_result_focus(&mut self) {
        if self.result_focus {
            self.result_focus = false;
        } else if self
            .sql_tabs
            .active()
            .is_some_and(|tab| tab.result.is_some())
        {
            self.result_focus = true;
        } else {
            self.message = "This tab has no result yet. Press e to execute.".to_owned();
        }
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
        if let Some(closed) = self.sql_tabs.close_active() {
            self.executions.retain(|tracked| {
                if tracked.tab == closed.id {
                    tracked.handle.abort();
                }
                tracked.tab != closed.id
            });
        }
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
            UiAction::LoadRelationRows(_) => {}
            UiAction::LoadRelationDetail(_) => {
                self.message = "Loading columns and indexes…".to_owned();
            }
            UiAction::SaveConnection => self.message = "Saving connection…".to_owned(),
            UiAction::TestConnection(_) => self.message = "Testing connection…".to_owned(),
            UiAction::DeleteConnection(_) => self.message = "Deleting connection…".to_owned(),
            UiAction::ListFiles => {
                if self.sql_tabs.active().is_none() {
                    self.client = Some(client);
                    self.message = "Open a SQL tab before opening a file.".to_owned();
                    return;
                }
                self.message = "Loading SQL files…".to_owned();
            }
            UiAction::OpenFile(_) => self.message = "Opening SQL file…".to_owned(),
            UiAction::ExecuteSql(_) => {
                self.client = Some(client);
                return;
            }
            UiAction::CancelExecution(_) => self.message = "Cancelling execution…".to_owned(),
            UiAction::SaveFile(_) | UiAction::CreateFile(_) | UiAction::OverwriteFile(_) => {
                self.message = "Saving SQL file…".to_owned();
            }
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
                | Operation::LoadRelationRows { .. }
        )
        .then(|| CatalogPath {
            database: self.catalog_database.clone(),
            schema: self.catalog_schema.clone(),
            relation: self.catalog_relation.clone(),
            rows: self
                .relation_rows
                .as_ref()
                .map(Grid::request)
                .unwrap_or_default(),
        })
        .unwrap_or_default();
        let file_request = self.file_request(operation);
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
                file_request,
            )
            .await;
            (client, result)
        }));
    }

    fn apply_result(&mut self, result: OperationResult) {
        match result {
            OperationResult::Login(Ok(session)) => {
                self.state = ScreenState::Authenticated(session);
                self.execution_client = self.client.clone();
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
                match &mut self.relation_rows {
                    Some(grid) => grid.mark_loading(),
                    None => self.relation_rows = Some(Grid::remote()),
                }
                if let Some(connection_id) = self.catalog_connection {
                    self.start_operation(
                        Operation::LoadRelationRows { connection_id },
                        UiAction::LoadRelationRows(connection_id),
                    );
                }
            }
            OperationResult::RelationRows(Ok(page)) => {
                if let Some(grid) = &mut self.relation_rows {
                    grid.set_remote_page(page.columns, page.rows, page.total_rows);
                }
            }
            OperationResult::RelationRows(Err(error)) => {
                if let Some(grid) = &mut self.relation_rows {
                    let message = match error {
                        ApiError::Http {
                            code: Some("relation_not_readable"),
                            ..
                        } => "The saved credential cannot read this relation.",
                        // An older server answers 404 for the rows route as well.
                        ApiError::Http {
                            code: Some("not_found"),
                            ..
                        } => "Table not found, or the server is out of date: restart the server.",
                        error => api_error_message(error),
                    };
                    grid.set_remote_error(message.to_owned());
                }
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
            OperationResult::CancelRequested(Ok(())) => {
                self.message = "Cancellation requested.".to_owned();
            }
            OperationResult::CancelRequested(Err(ApiError::Http {
                code: Some("execution_not_running"),
                ..
            })) => {
                self.message = "The execution already finished.".to_owned();
            }
            OperationResult::CancelRequested(Err(error)) => {
                self.message = execution_error_message(error).to_owned();
            }
            OperationResult::Files(Ok(files)) => {
                if files.is_empty() {
                    self.message = "No saved SQL files.".to_owned();
                } else {
                    self.message.clear();
                    self.file_picker = Some(FilePicker { files, selected: 0 });
                }
            }
            OperationResult::Files(Err(error)) => {
                self.message = file_error_message(error).to_owned();
            }
            OperationResult::OpenedFile(Ok(opened)) => self.open_file_tab(opened),
            OperationResult::OpenedFile(Err(error)) => {
                self.message = file_error_message(error).to_owned();
            }
            OperationResult::SavedFile {
                tab,
                content,
                result: Ok(file),
            } => {
                self.message = format!("Saved {}.", file.name);
                if let Some(tab) = self.sql_tabs.get_mut(tab) {
                    tab.mark_saved(
                        TabFile {
                            id: file.id,
                            name: file.name,
                            sha256: file.sha256,
                        },
                        content,
                    );
                }
            }
            OperationResult::SavedFile {
                tab,
                result:
                    Err(ApiError::Http {
                        code: Some("file_conflict"),
                        ..
                    }),
                ..
            } => {
                self.save_conflict = Some(tab);
                self.message = "The file changed on the server since it was opened.".to_owned();
            }
            OperationResult::SavedFile {
                result: Err(error), ..
            } => {
                self.message = file_error_message(error).to_owned();
            }
        }
    }

    /// Opens a downloaded file in a new tab bound to the active tab's
    /// connection and database.
    fn open_file_tab(&mut self, opened: SqlFileContent) {
        let Some((connection_id, connection_name, database)) = self.sql_tabs.active().map(|tab| {
            (
                tab.connection_id,
                tab.connection_name.clone(),
                tab.database.clone(),
            )
        }) else {
            self.message = "Open a SQL tab before opening a file.".to_owned();
            return;
        };
        let name = opened.file.name.clone();
        let file = TabFile {
            id: opened.file.id,
            name: opened.file.name,
            sha256: opened.file.sha256,
        };
        match self.sql_tabs.open_file(
            connection_id,
            &connection_name,
            &database,
            file,
            opened.content,
        ) {
            Ok(_) => {
                self.view = SQL_VIEW;
                self.message = format!("Opened {name}.");
            }
            Err(_) => {
                self.message =
                    format!("Close a SQL tab before opening another (limit {MAX_SQL_TABS}).");
            }
        }
    }

    /// Reloads the relation data grid when its Auto page no longer fits.
    fn refit_relation_rows(&mut self) {
        if self.view != 5 || self.pending.is_some() {
            return;
        }
        let (Some(grid), Some(connection_id)) = (&mut self.relation_rows, self.catalog_connection)
        else {
            return;
        };
        if grid.needs_refit() {
            grid.refit();
            self.start_operation(
                Operation::LoadRelationRows { connection_id },
                UiAction::LoadRelationRows(connection_id),
            );
        }
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
                self.apply_result(result);
                if std::mem::take(&mut self.relation_rows_stale)
                    && self.view == 5
                    && self.pending.is_none()
                    && let Some(connection_id) = self.catalog_connection
                {
                    self.start_operation(
                        Operation::LoadRelationRows { connection_id },
                        UiAction::LoadRelationRows(connection_id),
                    );
                }
            }
            Err(_) => {
                self.client = ApiClient::from_env().ok();
                self.message = "The API operation did not complete.".to_owned();
            }
        }
    }
}

fn execution_error_message(error: ApiError) -> &'static str {
    match error {
        ApiError::Http {
            code: Some("execution_limit_reached"),
            ..
        } => "Too many SQL executions are running. Wait for one to finish.",
        ApiError::Http {
            code: Some("server_execution_limit_reached"),
            ..
        } => "The server is busy with other SQL executions. Try again shortly.",
        ApiError::Http {
            code: Some("not_found"),
            ..
        } => "The connection or execution is no longer available.",
        ApiError::Http {
            code: Some("credential_encryption_unavailable"),
            ..
        } => "The server cannot decrypt saved credentials.",
        error => api_error_message(error),
    }
}

fn file_error_message(error: ApiError) -> &'static str {
    match error {
        ApiError::Http {
            code: Some("not_found"),
            ..
        } => "The SQL file is no longer available.",
        ApiError::Http {
            code: Some("file_name_conflict"),
            ..
        } => "A SQL file with this name already exists.",
        ApiError::Http {
            code: Some("file_too_large"),
            ..
        } => "SQL files are limited to 1 MiB.",
        ApiError::Http {
            code: Some("invalid_request"),
            ..
        } => "Invalid file name. Use a plain name ending in .sql.",
        ApiError::Http {
            code: Some("file_storage_unavailable"),
            ..
        } => "SQL file storage is not configured on the server.",
        error => api_error_message(error),
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
    let mode = detect_color_mode(
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("COLORTERM").as_deref(),
    );
    let name = match parse_theme_name(std::env::var_os(THEME_ENV).as_deref()) {
        Ok(name) => name.unwrap_or(theme::ThemeName::Dark),
        Err(()) => {
            app.message = format!("Unknown {THEME_ENV}; using the dark theme.");
            theme::ThemeName::Dark
        }
    };
    app.theme = Theme::new(name, mode);
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
                app.complete_executions().await;
                app.refit_relation_rows();
                terminal.draw(|frame| render(frame, &app))?;
            }
        }
    }
    Ok(())
}

fn start_action(app: &mut App, action: UiAction) {
    let operation = match action {
        UiAction::None => return,
        UiAction::LoadRelationRows(_) if app.pending.is_some() => {
            app.relation_rows_stale = true;
            return;
        }
        UiAction::LoadRelationRows(connection_id) => Operation::LoadRelationRows { connection_id },
        UiAction::ExecuteSql(tab) => {
            app.start_execution(tab);
            return;
        }
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
        UiAction::ListFiles => Operation::ListFiles,
        UiAction::OpenFile(id) => Operation::OpenFile { id },
        UiAction::SaveFile(tab) => Operation::SaveFile { tab },
        UiAction::CreateFile(tab) => Operation::CreateFile { tab },
        UiAction::OverwriteFile(tab) => Operation::OverwriteFile { tab },
        UiAction::CancelExecution(id) => Operation::CancelExecution { id },
    };
    app.start_operation(operation, action);
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let theme = &app.theme;
    let area = frame.area();
    frame.render_widget(Block::default().style(theme.base()), area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            // Bordered footer: one line of shortcuts between the borders.
            Constraint::Length(3),
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
                render_connection_form(frame, theme, sections[1], form, &app.message);
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
                    theme,
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
                    theme,
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
                                theme.muted(),
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
                    theme,
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
    } else if app.confirm_explain.is_some() {
        "EXPLAIN ANALYZE runs the analyzed statement. Execute anyway? y: run   n/Esc: cancel"
    } else if app.confirm_close_tab {
        "Close tab with unsaved changes? y: close   n/Esc: cancel"
    } else if app.file_prompt.is_some() {
        "Type a file name   Enter: save   Esc: cancel"
    } else if app.save_conflict.is_some() {
        "File changed on the server. o: overwrite   a: save as   Esc: cancel"
    } else if app.file_picker.is_some() {
        "↑/↓: select   Enter: open in new tab   Esc: close"
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
        if app.result_focus {
            "↑↓←→: move  [ ]: page  g/G: first/last  +/-: size  s: sort  /: search  Enter: value  L: lines  Tab: editor"
        } else {
            "Enter/i: edit  e: run  c: cancel  Tab: result  w/W: save  o: open  [ ]: tabs  x: close"
        }
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 2 {
        "↑/↓: navigate   Enter: schemas   s: new SQL tab   r: reload   Tab/Esc: connections   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 3 {
        "↑/↓: navigate   Enter: relations   r: reload   Tab/Esc: databases   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 4 {
        "↑/↓: navigate   Enter: details   [ / ]: pages   r: reload   Tab/Esc: schemas   q: quit"
    } else if matches!(app.state, ScreenState::Authenticated(_)) && app.view == 5 {
        if app.relation_grid_focus {
            "↑↓←→: move  [ ]: page  g/G: first/last  +/-: size  s: sort  /: search  Enter: value  L: lines  Tab: columns"
        } else {
            "↑/↓: columns   Tab: data grid   r: reload   Esc: relations   q: quit"
        }
    } else if matches!(app.state, ScreenState::Authenticated(_)) {
        "Tab/Enter: open Connections   s: SQL tabs   F2: theme   q/Esc: quit"
    } else {
        "Tab: next field   Enter: continue/sign in   Esc: quit"
    };
    frame.render_widget(
        Paragraph::new(footer_text).block(Block::default().borders(Borders::ALL)),
        sections[2],
    );
}

fn render_login(frame: &mut Frame<'_>, area: Rect, app: &App, state: &ScreenState) {
    let theme = &app.theme;
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
        Paragraph::new(app.login_email.as_str()).block(field_block(
            theme,
            "Email",
            focus == Some(LoginField::Email),
        )),
        form[0],
    );
    let masked_password = "•".repeat(app.login_password.chars().count());
    frame.render_widget(
        Paragraph::new(masked_password).block(field_block(
            theme,
            "Password",
            focus == Some(LoginField::Password),
        )),
        form[1],
    );
    let status = match state {
        ScreenState::EditingLogin {
            error: Some(error), ..
        } => error.as_str(),
        ScreenState::EditingLogin { error: None, .. } if !app.message.is_empty() => {
            app.message.as_str()
        }
        ScreenState::EditingLogin { error: None, .. } => "Sign in with your PGRustAdmin account.",
        ScreenState::Authenticating => "Signing in and checking session…",
        ScreenState::Authenticated(_) => "",
    };
    frame.render_widget(Paragraph::new(status).style(theme.warning()), form[2]);
}

fn field_block(theme: &Theme, title: &'static str, focused: bool) -> Block<'static> {
    let style = if focused {
        theme.focus()
    } else {
        Style::default()
    };
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(style)
}

fn render_workspace(frame: &mut Frame<'_>, area: Rect, app: &App, session: &AuthSession) {
    let theme = &app.theme;
    let layout = responsive_layout(area);
    let menu = List::new(
        MENU_ITEMS
            .iter()
            .map(|item| ListItem::new(Line::from(format!(" {item}")))),
    )
    .block(Block::default().borders(Borders::ALL).title(" Sections "))
    .highlight_style(theme.selection())
    .highlight_symbol("› ");
    let mut menu_state = ListState::default().with_selected(Some(app.view));
    frame.render_stateful_widget(menu, layout[0], &mut menu_state);
    let content = Text::from(vec![
        Line::from("Overview").style(theme.focus()),
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
    let theme = &app.theme;
    let layout = responsive_layout(area);
    let items = app.connections.iter().map(|connection| {
        ListItem::new(vec![
            Line::from(if connection.read_only {
                format!("{} [read-only]", connection.name)
            } else {
                connection.name.clone()
            }),
            Line::from(format!("  {}:{}", connection.host, connection.port)),
        ])
    });
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Connections "),
        )
        .highlight_style(theme.selection())
        .highlight_symbol("› ");
    let mut list_state = ListState::default()
        .with_selected((!app.connections.is_empty()).then_some(app.selected_connection));
    frame.render_stateful_widget(list, layout[0], &mut list_state);

    let details = if let Some(connection) = app.connections.get(app.selected_connection) {
        Text::from(vec![
            Line::from(connection.name.as_str()).style(theme.focus()),
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
            Line::from(format!(
                "Read-only: {}",
                if connection.read_only { "yes" } else { "no" }
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

fn column_line<'a>(theme: &Theme, column: &'a Column) -> Line<'a> {
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
        Span::styled(format!("  {}", column.data_type), theme.accent()),
        Span::styled(format!("  {}", details.join(", ")), theme.muted()),
    ])
}

fn index_line<'a>(theme: &Theme, index: &'a Index) -> Line<'a> {
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
            theme.accent(),
        ),
        Span::styled(format!("  {}", flags.join(", ")), theme.muted()),
    ])
}

fn render_sql_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = &app.theme;
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
    let execution = app
        .sql_tabs
        .active()
        .map(|tab| execution_line(theme, &tab.execution))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(vec![
            execution,
            Line::styled(app.message.as_str(), theme.warning()),
        ]),
        panes[2],
    );
    render_file_overlays(frame, panes[1], app);
    let titles = app.sql_tabs.tabs().iter().map(|tab| {
        let marker = match (tab.is_dirty(), tab.execution == TabExecution::Running) {
            (true, true) => " * …",
            (true, false) => " *",
            (false, true) => " …",
            (false, false) => "",
        };
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
        .highlight_style(theme.selection());
    frame.render_widget(tabs, panes[0]);

    let Some(tab) = app.sql_tabs.active() else {
        frame.render_widget(Paragraph::new(app.message.as_str()), panes[1]);
        return;
    };
    let mode = if app.sql_editing { " [EDIT]" } else { "" };
    let title = format!(
        " {} — {} / {}{mode} ",
        tab.file
            .as_ref()
            .map_or("untitled", |file| file.name.as_str()),
        tab.connection_name,
        tab.database
    );
    let editor_area = match &tab.result {
        Some(result) => {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(panes[1]);
            let duration = match tab.execution {
                TabExecution::Succeeded { duration_ms, .. } => duration_text(" ·", duration_ms),
                _ => String::new(),
            };
            result.render(
                frame,
                split[1],
                theme,
                &format!(" Result{duration} "),
                app.result_focus,
            );
            split[0]
        }
        None => panes[1],
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(editor_area);
    frame.render_widget(block, editor_area);

    let editor = &tab.editor;
    if !app.sql_editing && editor.text().is_empty() {
        frame.render_widget(
            Paragraph::new("No SQL yet. Press Enter to edit.").style(theme.muted()),
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

fn execution_line(theme: &Theme, execution: &TabExecution) -> Line<'static> {
    match execution {
        TabExecution::Idle => Line::default(),
        TabExecution::Running => Line::styled("Running…", theme.accent()),
        TabExecution::Succeeded {
            counts,
            duration_ms,
        } => Line::styled(
            format!(
                "Succeeded{}: {} statement(s); last returned {} row(s), affected {}.",
                duration_text(" in", *duration_ms),
                counts.statement_count,
                counts.rows_returned,
                counts.rows_affected
            ),
            theme.success(),
        ),
        TabExecution::Failed {
            code,
            message,
            duration_ms,
        } => {
            let code = code
                .as_deref()
                .map(|code| format!(" [{code}]"))
                .unwrap_or_default();
            Line::styled(
                format!(
                    "Failed{}{code}: {message}",
                    duration_text(" after", *duration_ms)
                ),
                theme.error(),
            )
        }
        TabExecution::Cancelled { duration_ms } => Line::styled(
            format!("Cancelled{}.", duration_text(" after", *duration_ms)),
            theme.warning(),
        ),
        TabExecution::TimedOut { duration_ms } => Line::styled(
            format!("Timed out{}.", duration_text(" after", *duration_ms)),
            theme.error(),
        ),
    }
}

/// Heuristic warning check: whether a word `EXPLAIN` is followed by
/// `ANALYZE`/`ANALYSE` before the next `;`, ignoring `--` and `/* */`
/// comments and letter case. It may warn needlessly (for example inside
/// string literals) and never changes or blocks the SQL.
fn mentions_explain_analyze(sql: &str) -> bool {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = sql.chars().peekable();
    let finish_word = |word: &mut String, words: &mut Vec<String>| {
        if !word.is_empty() {
            words.push(std::mem::take(word).to_ascii_lowercase());
        }
    };
    while let Some(character) = chars.next() {
        match character {
            '-' if chars.peek() == Some(&'-') => {
                finish_word(&mut word, &mut words);
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                finish_word(&mut word, &mut words);
                chars.next();
                let mut previous = '\0';
                for skipped in chars.by_ref() {
                    if previous == '*' && skipped == '/' {
                        break;
                    }
                    previous = skipped;
                }
            }
            ';' => {
                finish_word(&mut word, &mut words);
                words.push(";".to_owned());
            }
            character if character.is_alphanumeric() || character == '_' => word.push(character),
            _ => finish_word(&mut word, &mut words),
        }
    }
    finish_word(&mut word, &mut words);

    let mut after_explain = false;
    for word in words {
        match word.as_str() {
            "explain" => after_explain = true,
            "analyze" | "analyse" if after_explain => return true,
            ";" => after_explain = false,
            _ => {}
        }
    }
    false
}

fn duration_text(prefix: &str, duration_ms: Option<u64>) -> String {
    duration_ms
        .map(|duration| format!("{prefix} {duration} ms"))
        .unwrap_or_default()
}

/// Draws the save-as prompt or the file picker over `area`.
fn render_file_overlays(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = &app.theme;
    if let Some(prompt) = &app.file_prompt {
        let popup = centered(area, 60, 3);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(format!("{}▏", prompt.name)).block(
                Block::default()
                    .borders(Borders::ALL)
                    .style(theme.surface())
                    .title(" Save SQL file as "),
            ),
            popup,
        );
    } else if let Some(picker) = &app.file_picker {
        let height = u16::try_from(picker.files.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2);
        let popup = centered(area, 60, height);
        let items: Vec<_> = picker
            .files
            .iter()
            .map(|file| {
                ListItem::new(Line::from(vec![
                    Span::raw(file.name.as_str()),
                    Span::styled(format!("  {} bytes", file.size_bytes), theme.muted()),
                ]))
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .style(theme.surface())
                    .title(" Open SQL file "),
            )
            .highlight_style(theme.selection())
            .highlight_symbol("› ");
        let mut state = ListState::default().with_selected(Some(picker.selected));
        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(list, popup, &mut state);
    }
}

/// A rectangle `width_percent` wide and `height` rows tall, centered in `area`
/// and clamped to it.
fn centered(area: Rect, width_percent: u16, height: u16) -> Rect {
    let width = area.width.saturating_mul(width_percent) / 100;
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn render_relation_detail(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = &app.theme;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    let structure = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let title = format!(
        " Columns — {} / {} / {}.{} ",
        app.catalog_connection_name, app.catalog_database, app.catalog_schema, app.catalog_relation
    );
    let items = app
        .columns
        .iter()
        .map(|column| ListItem::new(column_line(theme, column)))
        .collect();
    render_catalog_list(
        frame,
        theme,
        structure[0],
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
        frame.render_widget(Paragraph::new(text).block(block), structure[1]);
    } else {
        let items: Vec<_> = app
            .indexes
            .iter()
            .map(|index| ListItem::new(index_line(theme, index)))
            .collect();
        frame.render_widget(List::new(items).block(block), structure[1]);
    }
    if let Some(grid) = &app.relation_rows {
        grid.render(
            frame,
            rows[1],
            theme,
            &format!(" Data — {}.{} ", app.catalog_schema, app.catalog_relation),
            app.relation_grid_focus,
        );
    }
}

fn render_catalog_list(
    frame: &mut Frame<'_>,
    theme: &Theme,
    area: Rect,
    title: &str,
    items: Vec<ListItem<'_>>,
    selected: usize,
    message: &str,
) {
    let is_empty = items.is_empty();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(theme.selection())
        .highlight_symbol("› ");
    let mut list_state = ListState::default().with_selected((!is_empty).then_some(selected));
    frame.render_stateful_widget(list, area, &mut list_state);
    if is_empty {
        frame.render_widget(Paragraph::new(message).style(theme.warning()), area);
    }
}

fn render_connection_form(
    frame: &mut Frame<'_>,
    theme: &Theme,
    area: Rect,
    form: &ConnectionForm,
    message: &str,
) {
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
        if form.read_only {
            "Yes (data editing blocked)".to_owned()
        } else {
            "No".to_owned()
        },
    ];
    let lines = CONNECTION_FIELDS
        .iter()
        .zip(values)
        .enumerate()
        .map(|(index, (label, value))| {
            let marker = if index == form.focus { "›" } else { " " };
            let style = if index == form.focus {
                theme.focus()
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
    rows: GridRequest,
}

async fn run_operation(
    client: &mut ApiClient,
    operation: Operation,
    email: &str,
    password: &str,
    catalog: &CatalogPath,
    input: Option<&ConnectionInput>,
    file: FileRequest,
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
        Operation::LoadRelationRows { connection_id } => {
            let relation = RelationRef {
                connection_id,
                database: &catalog.database,
                schema: &catalog.schema,
                relation: &catalog.relation,
            };
            let request = RelationRowsRequest {
                page: catalog.rows.page,
                per_page: catalog.rows.per_page,
                sort: catalog
                    .rows
                    .sort
                    .as_ref()
                    .map(|(column, descending)| (column.as_str(), *descending)),
                search: &catalog.rows.search,
            };
            OperationResult::RelationRows(client.list_relation_rows(relation, request).await)
        }
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
        Operation::CancelExecution { id } => {
            OperationResult::CancelRequested(client.cancel_execution(id).await)
        }
        Operation::ListFiles => OperationResult::Files(
            client
                .list_sql_files(1, FILE_LIST_SIZE)
                .await
                .map(|files| files.items),
        ),
        Operation::OpenFile { id } => OperationResult::OpenedFile(client.read_sql_file(id).await),
        Operation::SaveFile { tab } => {
            let result = match file.file_id {
                Some(id) => {
                    client
                        .update_sql_file(id, &file.content, &file.expected_sha256)
                        .await
                }
                None => Err(ApiError::InvalidResponse),
            };
            OperationResult::SavedFile {
                tab,
                content: file.content,
                result,
            }
        }
        Operation::CreateFile { tab } => {
            let result = client.create_sql_file(&file.name, &file.content).await;
            OperationResult::SavedFile {
                tab,
                content: file.content,
                result,
            }
        }
        Operation::OverwriteFile { tab } => {
            let result = match file.file_id {
                Some(id) => {
                    async {
                        let current = client.read_sql_file(id).await?;
                        client
                            .update_sql_file(id, &file.content, &current.file.sha256)
                            .await
                    }
                    .await
                }
                None => Err(ApiError::InvalidResponse),
            };
            OperationResult::SavedFile {
                tab,
                content: file.content,
                result,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pgrustadmin_api_client::{
        ApiClient, ApiError, AuthSession, Column, Connection, Database, Execution, ExecutionError,
        ExecutionResult, ExecutionRows, ExecutionStatus, Index, Relation, RelationKind,
        RelationRowsPage, Schema, SqlFile, SqlFileContent,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use uuid::Uuid;

    use crate::grid::Grid;

    use super::{
        App, ConnectionForm, FinishedExecution, LoginField, MAX_SQL_TABS, OperationResult,
        READ_ONLY_FIELD, SQL_VIEW, ScreenState, TLS_FIELD, TabExecution, TabId, TrackedExecution,
        UiAction, mentions_explain_analyze, render,
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
            read_only: false,
        };
        let mut form = ConnectionForm::edit(&connection);
        let input = form.input().expect("editing without password is valid");
        assert!(input.password.is_none());
        assert!(!input.read_only);

        form.focus = READ_ONLY_FIELD;
        assert!(!form.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(form.read_only, "space toggles read-only");
        assert!(form.input().expect("valid").read_only);
        form.focus = TLS_FIELD;
        assert!(
            !form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            "Enter on TLS moves on"
        );
        assert_eq!(form.focus, READ_ONLY_FIELD);
        assert!(
            form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            "Enter on the last field submits"
        );
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
            read_only: false,
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
            read_only: false,
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

    fn sql_file(name: &str, sha256: &str) -> SqlFile {
        SqlFile {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            size_bytes: 8,
            sha256: sha256.to_owned(),
        }
    }

    #[test]
    fn saving_untitled_tab_asks_for_name_and_records_the_file() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        app.sql_tabs
            .active_mut()
            .expect("active tab")
            .editor
            .insert_str("SELECT 1;");

        assert!(matches!(
            press(&mut app, KeyCode::Char('w')),
            UiAction::None
        ));
        assert!(app.file_prompt.is_some());
        for character in "report".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Char('t'));
        assert!(matches!(
            press(&mut app, KeyCode::Enter),
            UiAction::CreateFile(id) if id == tab_id
        ));
        assert!(app.file_prompt.is_none());
        assert_eq!(app.file_name, "report.sql");

        let file = sql_file("report.sql", "aa");
        app.apply_result(OperationResult::SavedFile {
            tab: tab_id,
            content: "SELECT 1;".to_owned(),
            result: Ok(file.clone()),
        });
        let tab = app.sql_tabs.active().expect("active tab");
        assert!(!tab.is_dirty());
        assert_eq!(tab.file.as_ref().map(|file| file.id), Some(file.id));
        assert!(app.message.contains("report.sql"));

        assert!(matches!(
            press(&mut app, KeyCode::Char('w')),
            UiAction::SaveFile(id) if id == tab_id
        ));
        press(&mut app, KeyCode::Char('W'));
        assert!(app.file_prompt.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.file_prompt.is_none());
        assert_eq!(app.view, SQL_VIEW);
    }

    #[test]
    fn save_conflict_offers_overwrite_or_save_as() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        app.apply_result(OperationResult::SavedFile {
            tab: tab_id,
            content: "SELECT 1;".to_owned(),
            result: Err(ApiError::Http {
                status: 409,
                code: Some("file_conflict"),
            }),
        });
        assert_eq!(app.save_conflict, Some(tab_id));
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit, "conflict prompt captures keys");
        assert!(matches!(
            press(&mut app, KeyCode::Char('o')),
            UiAction::OverwriteFile(id) if id == tab_id
        ));
        assert!(app.save_conflict.is_none());

        app.save_conflict = Some(tab_id);
        press(&mut app, KeyCode::Char('a'));
        assert!(app.save_conflict.is_none());
        assert!(app.file_prompt.is_some());
        press(&mut app, KeyCode::Esc);

        app.save_conflict = Some(tab_id);
        press(&mut app, KeyCode::Esc);
        assert!(app.save_conflict.is_none());
        assert!(app.file_prompt.is_none());
    }

    #[test]
    fn opening_a_file_lists_files_and_opens_a_clean_tab_on_the_same_connection() {
        let mut app = authenticated_app();
        assert!(matches!(
            press(&mut app, KeyCode::Char('o')),
            UiAction::None
        ));
        let mut app = app_with_sql_tab();
        let connection_id = app.sql_tabs.active().expect("active tab").connection_id;
        assert!(matches!(
            press(&mut app, KeyCode::Char('o')),
            UiAction::ListFiles
        ));
        let first = sql_file("a.sql", "aa");
        let second = sql_file("b.sql", "bb");
        app.apply_result(OperationResult::Files(Ok(vec![first, second.clone()])));
        assert!(app.file_picker.is_some());

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, &app))
            .expect("render file picker");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Open SQL file"));
        assert!(rendered.contains("b.sql"));

        press(&mut app, KeyCode::Down);
        assert!(matches!(
            press(&mut app, KeyCode::Enter),
            UiAction::OpenFile(id) if id == second.id
        ));
        assert!(app.file_picker.is_none());

        app.apply_result(OperationResult::OpenedFile(Ok(SqlFileContent {
            file: second.clone(),
            content: "SELECT 2;".to_owned(),
        })));
        assert_eq!(app.sql_tabs.tabs().len(), 2);
        let tab = app.sql_tabs.active().expect("opened tab");
        assert_eq!(tab.editor.text(), "SELECT 2;");
        assert_eq!(tab.connection_id, connection_id);
        assert_eq!(tab.file.as_ref().map(|file| file.id), Some(second.id));
        assert!(!tab.is_dirty());

        app.apply_result(OperationResult::Files(Ok(Vec::new())));
        assert!(app.file_picker.is_none());
        assert!(app.message.contains("No saved SQL files"));
    }

    fn execution(status: ExecutionStatus) -> Execution {
        Execution {
            id: Uuid::new_v4(),
            connection_id: Uuid::new_v4(),
            database: "app".to_owned(),
            status,
            duration_ms: None,
            result: None,
            error: None,
        }
    }

    #[test]
    fn execute_key_runs_non_empty_sql_once_per_tab() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        assert!(matches!(
            press(&mut app, KeyCode::Char('e')),
            UiAction::None
        ));
        assert!(app.message.contains("no SQL"));

        app.sql_tabs
            .active_mut()
            .expect("active tab")
            .editor
            .insert_str("SELECT 1;");
        assert!(matches!(
            press(&mut app, KeyCode::Char('e')),
            UiAction::ExecuteSql(id) if id == tab_id
        ));
        app.sql_tabs.active_mut().expect("active tab").execution = TabExecution::Running;
        assert!(matches!(
            press(&mut app, KeyCode::Char('e')),
            UiAction::None
        ));
        assert!(app.message.contains("already running"));
    }

    #[test]
    fn finished_executions_update_their_tab_and_render() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        app.finish_execution(
            tab_id,
            Ok(FinishedExecution {
                execution: Execution {
                    duration_ms: Some(12),
                    result: Some(ExecutionResult {
                        statement_count: 3,
                        rows_returned: 2,
                        rows_affected: 2,
                    }),
                    ..execution(ExecutionStatus::Succeeded)
                },
                rows: Some(Ok(ExecutionRows {
                    columns: vec!["id".to_owned(), "label".to_owned()],
                    rows: vec![
                        vec![Some("1".to_owned()), None],
                        vec![Some("2".to_owned()), Some("ção".to_owned())],
                    ],
                    stored_rows: 2,
                    total_rows: 5,
                    truncated: true,
                })),
            }),
        );
        let rendered = render_text(&app);
        assert!(
            rendered.contains(
                "Succeeded in 12 ms: 3 statement(s); last returned 2 row(s), affected 2."
            )
        );
        assert!(rendered.contains("Result · 12 ms"));
        assert!(rendered.contains("first 2 of 5 stored"));
        assert!(
            rendered.contains(" # │ id   │ label"),
            "grid lines between columns"
        );
        assert!(rendered.contains("label"));
        assert!(rendered.contains("NULL"));

        app.finish_execution(
            tab_id,
            Ok(FinishedExecution {
                execution: Execution {
                    duration_ms: Some(3),
                    error: Some(ExecutionError {
                        code: Some("42601".to_owned()),
                        message: "syntax error at or near \"SELEC\"".to_owned(),
                    }),
                    ..execution(ExecutionStatus::Failed)
                },
                rows: None,
            }),
        );
        let rendered = render_text(&app);
        assert!(rendered.contains("Failed after 3 ms [42601]: syntax error at or near \"SELEC\""));
        assert!(!rendered.contains("Result —"), "failed runs show no result");

        app.finish_execution(
            tab_id,
            Ok(FinishedExecution {
                execution: Execution {
                    result: Some(ExecutionResult {
                        statement_count: 1,
                        rows_returned: 1,
                        rows_affected: 1,
                    }),
                    ..execution(ExecutionStatus::Succeeded)
                },
                rows: Some(Err(ApiError::Http {
                    status: 410,
                    code: Some("result_expired"),
                })),
            }),
        );
        assert!(app.sql_tabs.active().expect("tab").result.is_none());
        assert!(app.message.contains("Result rows are unavailable"));

        app.finish_execution(
            tab_id,
            Err(ApiError::Http {
                status: 429,
                code: Some("execution_limit_reached"),
            }),
        );
        assert!(render_text(&app).contains("Too many SQL executions are running"));
        app.finish_execution(
            tab_id,
            Err(ApiError::Http {
                status: 429,
                code: Some("server_execution_limit_reached"),
            }),
        );
        assert!(render_text(&app).contains("The server is busy with other SQL executions"));

        let closed = app
            .sql_tabs
            .open(Uuid::new_v4(), "local", "other")
            .expect("open tab");
        app.sql_tabs.close_active();
        app.finish_execution(closed, Err(ApiError::Transport));
        assert_eq!(
            app.sql_tabs.tabs().len(),
            1,
            "results for closed tabs are dropped"
        );
    }

    fn render_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, app))
            .expect("render app");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn rows_result(columns: usize, rows: usize) -> ExecutionRows {
        ExecutionRows {
            columns: (0..columns)
                .map(|index| format!("column_{index}"))
                .collect(),
            rows: (0..rows)
                .map(|row| {
                    (0..columns)
                        .map(|column| Some(format!("r{row}c{column}")))
                        .collect()
                })
                .collect(),
            stored_rows: rows as u64,
            total_rows: rows as u64,
            truncated: false,
        }
    }

    #[test]
    fn tab_focuses_the_result_grid_which_handles_its_own_keys() {
        let mut app = app_with_sql_tab();
        press(&mut app, KeyCode::Tab);
        assert!(!app.result_focus);
        assert!(app.message.contains("no result"));
        assert_eq!(app.view, SQL_VIEW, "Tab no longer leaves the SQL section");

        app.sql_tabs.active_mut().expect("tab").result = Some(grid_from(rows_result(3, 30)));
        press(&mut app, KeyCode::Tab);
        assert!(app.result_focus);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('s'));
        let rendered = render_text(&app);
        assert!(
            rendered.contains("column_1 ▼"),
            "sorted by the active column"
        );
        assert!(rendered.contains("sort: column_1 ▼"));
        assert!(rendered.contains("« ‹"));
        assert_eq!(
            app.view, SQL_VIEW,
            "arrows move the cursor instead of leaving"
        );

        press(&mut app, KeyCode::Enter);
        assert!(render_text(&app).contains("column_1 — Esc to close"));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, SQL_VIEW, "Esc closes the value popup first");
        assert!(!render_text(&app).contains("Esc to close"));

        press(&mut app, KeyCode::Tab);
        assert!(!app.result_focus);
        press(&mut app, KeyCode::Enter);
        assert!(app.sql_editing, "Enter edits once the editor has the focus");
    }

    fn grid_from(rows: ExecutionRows) -> Grid {
        Grid::local(rows.columns, rows.rows, rows.total_rows)
    }

    fn rows_page(count: usize) -> RelationRowsPage {
        RelationRowsPage {
            columns: vec!["id".to_owned(), "email".to_owned()],
            rows: (0..count)
                .map(|index| vec![Some(index.to_string()), Some(format!("user{index}@x.io"))])
                .collect(),
            total_rows: 120,
            page: 1,
            per_page: 25,
        }
    }

    #[tokio::test]
    async fn relation_detail_loads_a_data_grid_that_pages_on_the_server() {
        let mut app = authenticated_app();
        let connection_id = Uuid::new_v4();
        app.view = 4;
        app.catalog_connection = Some(connection_id);
        app.relations = vec![Relation {
            name: "users".to_owned(),
            kind: RelationKind::Table,
        }];
        assert!(matches!(
            press(&mut app, KeyCode::Enter),
            UiAction::LoadRelationDetail(id) if id == connection_id
        ));
        app.apply_result(OperationResult::RelationDetail(Ok((
            vec![column("id")],
            Vec::new(),
        ))));
        assert!(
            app.relation_rows.is_some(),
            "the data grid opens with the detail"
        );
        assert!(app.pending.is_some(), "rows are requested right away");
        if let Some(task) = app.pending.take() {
            task.abort();
        }
        app.client = Some(client());
        assert!(render_text(&app).contains("Loading rows…"));

        app.apply_result(OperationResult::RelationRows(Ok(rows_page(25))));
        let rendered = render_text(&app);
        assert!(rendered.contains("Data — .users"));
        assert!(rendered.contains("user0@x.io"));
        assert!(rendered.contains("of 120"));

        press(&mut app, KeyCode::Tab);
        assert!(app.relation_grid_focus);
        assert!(render_text(&app).contains("+/-: size"));
        assert!(matches!(
            press(&mut app, KeyCode::Char(']')),
            UiAction::LoadRelationRows(id) if id == connection_id
        ));
        let grid = app.relation_rows.as_ref().expect("grid");
        assert_eq!(grid.request().page, 2);

        app.pending = Some(tokio::spawn(std::future::pending()));
        super::start_action(&mut app, UiAction::LoadRelationRows(connection_id));
        assert!(app.relation_rows_stale, "reload waits for the running call");
        if let Some(task) = app.pending.take() {
            task.abort();
        }

        app.apply_result(OperationResult::RelationRows(Err(ApiError::Http {
            status: 403,
            code: Some("relation_not_readable"),
        })));
        assert!(render_text(&app).contains("cannot read this relation"));
        app.apply_result(OperationResult::RelationRows(Err(ApiError::Http {
            status: 404,
            code: Some("not_found"),
        })));
        assert!(render_text(&app).contains("server is out of date: restart the server"));

        press(&mut app, KeyCode::Tab);
        assert!(!app.relation_grid_focus);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, 4, "Esc returns to the relations");
    }

    #[tokio::test]
    async fn new_execution_removes_the_previous_result() {
        let mut app = app_with_sql_tab();
        app.execution_client = Some(client());
        let tab_id = app.sql_tabs.active().expect("tab").id;
        let tab = app.sql_tabs.active_mut().expect("tab");
        tab.editor.insert_str("SELECT 1");
        tab.result = Some(grid_from(rows_result(1, 1)));
        app.result_focus = true;
        app.start_execution(tab_id);
        let tab = app.sql_tabs.active().expect("tab");
        assert!(tab.result.is_none());
        assert_eq!(tab.execution, TabExecution::Running);
        assert!(!app.result_focus);
        assert_eq!(app.executions.len(), 1);
        for tracked in app.executions.drain(..) {
            tracked.handle.abort();
        }
    }

    #[tokio::test]
    async fn closing_a_tab_stops_tracking_its_execution() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        app.executions.push(tracked(tab_id, None));
        press(&mut app, KeyCode::Char('x'));
        assert!(app.sql_tabs.is_empty());
        assert!(app.executions.is_empty());
    }

    /// A tracked execution whose server ID is `id` once delivered.
    fn tracked(tab: TabId, id: Option<Uuid>) -> TrackedExecution {
        let (sender, id_receiver) = tokio::sync::oneshot::channel();
        if let Some(id) = id {
            sender.send(id).expect("deliver execution id");
        }
        TrackedExecution {
            tab,
            handle: tokio::spawn(std::future::pending()),
            id_receiver,
            id: None,
        }
    }

    #[tokio::test]
    async fn cancel_key_needs_a_running_execution_with_a_known_id() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        assert!(matches!(
            press(&mut app, KeyCode::Char('c')),
            UiAction::None
        ));
        assert!(app.message.contains("not running"));

        app.executions.push(tracked(tab_id, None));
        assert!(matches!(
            press(&mut app, KeyCode::Char('c')),
            UiAction::None
        ));
        assert!(app.message.contains("still starting"));

        let id = Uuid::new_v4();
        for tracked in app.executions.drain(..) {
            tracked.handle.abort();
        }
        app.executions.push(tracked(tab_id, Some(id)));
        assert!(matches!(
            press(&mut app, KeyCode::Char('c')),
            UiAction::CancelExecution(cancelled) if cancelled == id
        ));
        app.apply_result(OperationResult::CancelRequested(Err(ApiError::Http {
            status: 409,
            code: Some("execution_not_running"),
        })));
        assert!(app.message.contains("already finished"));
        for tracked in app.executions.drain(..) {
            tracked.handle.abort();
        }
    }

    #[test]
    fn cancelled_and_timed_out_executions_render_their_duration() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        for (status, duration_ms, expected) in [
            (ExecutionStatus::Cancelled, 40, "Cancelled after 40 ms."),
            (ExecutionStatus::TimedOut, 1002, "Timed out after 1002 ms."),
        ] {
            app.finish_execution(
                tab_id,
                Ok(FinishedExecution {
                    execution: Execution {
                        duration_ms: Some(duration_ms),
                        ..execution(status)
                    },
                    rows: None,
                }),
            );
            assert!(render_text(&app).contains(expected), "{expected}");
        }
    }

    fn background_at_origin(app: &App) -> Option<ratatui::style::Color> {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("create test terminal");
        terminal
            .draw(|frame| render(frame, app))
            .expect("render app");
        terminal
            .backend()
            .buffer()
            .content
            .first()
            .map(|cell| cell.bg)
    }

    #[test]
    fn f2_switches_theme_everywhere_without_typing() {
        use ratatui::style::Color;

        let mut app = App::new(client());
        press(&mut app, KeyCode::F(2));
        assert_eq!(app.login_email, "", "F2 is not text");
        assert!(
            render_text(&app).contains("Theme: light."),
            "login shows messages"
        );
        assert_eq!(app.theme.name(), crate::theme::ThemeName::Light);
        assert_eq!(
            background_at_origin(&app),
            Some(Color::Rgb(0xF5, 0xF7, 0xFA))
        );

        let mut app = authenticated_app();
        assert_eq!(
            background_at_origin(&app),
            Some(Color::Rgb(0x17, 0x19, 0x1F))
        );
        assert!(render_text(&app).contains("F2: theme"));
        press(&mut app, KeyCode::F(2));
        assert!(app.message.contains("light"));
        press(&mut app, KeyCode::F(2));
        assert_eq!(app.theme.name(), crate::theme::ThemeName::Dark);

        app.theme = crate::theme::Theme::new(
            crate::theme::ThemeName::Dark,
            crate::theme::ColorMode::Monochrome,
        );
        assert_eq!(background_at_origin(&app), Some(Color::Reset));
    }

    #[test]
    fn footer_shows_shortcuts_and_confirmation_prompts() {
        let mut app = authenticated_app();
        app.view = 1;
        assert!(render_text(&app).contains("n: new"));
        app.confirm_delete = Some(Uuid::new_v4());
        assert!(render_text(&app).contains("Delete this connection? y: confirm"));
    }

    #[test]
    fn explain_analyze_heuristic_ignores_comments_and_case() {
        for sql in [
            "EXPLAIN ANALYZE SELECT 1",
            "explain (analyse, buffers) select 1",
            "-- check plan\n/* note */ Explain\n(FORMAT json, ANALYZE true) DELETE FROM t",
            "SELECT 1; EXPLAIN ANALYZE UPDATE t SET x = 1",
        ] {
            assert!(mentions_explain_analyze(sql), "{sql}");
        }
        for sql in [
            "EXPLAIN SELECT 1",
            "ANALYZE t",
            "EXPLAIN SELECT 1; ANALYZE t",
            "-- EXPLAIN ANALYZE\nSELECT 1",
            "/* explain analyze */ SELECT 1",
            "SELECT explain_analyze FROM t",
        ] {
            assert!(!mentions_explain_analyze(sql), "{sql}");
        }
    }

    #[test]
    fn executing_explain_analyze_asks_for_confirmation() {
        let mut app = app_with_sql_tab();
        let tab_id = app.sql_tabs.active().expect("active tab").id;
        app.sql_tabs
            .active_mut()
            .expect("active tab")
            .editor
            .insert_str("EXPLAIN ANALYZE DELETE FROM t");
        assert!(matches!(
            press(&mut app, KeyCode::Char('e')),
            UiAction::None
        ));
        assert_eq!(app.confirm_explain, Some(tab_id));
        assert!(render_text(&app).contains("EXPLAIN ANALYZE runs the analyzed statement"));
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit, "the prompt captures keys");
        press(&mut app, KeyCode::Esc);
        assert!(app.confirm_explain.is_none());
        assert_eq!(app.view, SQL_VIEW);

        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(
            press(&mut app, KeyCode::Char('y')),
            UiAction::ExecuteSql(id) if id == tab_id
        ));
        assert!(app.confirm_explain.is_none());
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
                read_only: false,
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
