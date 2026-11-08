//! Cliente HTTP assíncrono e tipado da API PGRustAdmin.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url, header::AUTHORIZATION};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{env, error::Error, fmt, time::Duration};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const API_BASE_URL_ENV: &str = "PGRUSTADMIN_API_BASE_URL";
const DEFAULT_API_BASE_URL: &str = "http://127.0.0.1:3000";
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Cloning shares the HTTP connection pool and copies the session token, so
/// long-running work such as execution polling can run beside other calls.
#[derive(Clone)]
pub struct ApiClient {
    base_url: Url,
    http: Client,
    access_token: Option<Zeroizing<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiError {
    InvalidBaseUrl,
    ClientConfiguration,
    Unauthenticated,
    Transport,
    InvalidResponse,
    Http {
        status: u16,
        code: Option<&'static str>,
    },
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl => formatter.write_str("API base URL must be a valid HTTP(S) URL"),
            Self::ClientConfiguration => formatter.write_str("could not configure HTTP client"),
            Self::Unauthenticated => formatter.write_str("authentication is required"),
            Self::Transport => formatter.write_str("could not communicate with the API"),
            Self::InvalidResponse => formatter.write_str("API returned an invalid response"),
            Self::Http {
                status,
                code: Some(code),
            } => {
                write!(formatter, "API returned HTTP {status} ({code})")
            }
            Self::Http { status, code: None } => write!(formatter, "API returned HTTP {status}"),
        }
    }
}

impl Error for ApiError {}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Health {
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Version {
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthSession {
    pub user_id: Uuid,
    pub expires_in: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CurrentSession {
    pub user_id: Uuid,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    email: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct LoginResponse {
    access_token: String,
    expires_in: u64,
    user_id: Uuid,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct Connection {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub tls_enabled: bool,
    /// Blocks data editing in the grid; missing from older servers.
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Serialize)]
pub struct ConnectionInput {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub tls_enabled: bool,
    pub read_only: bool,
    pub password: Option<String>,
}

impl Drop for ConnectionInput {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

#[derive(Deserialize, PartialEq, Eq)]
pub struct ConnectionList {
    pub items: Vec<Connection>,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct ConnectionTest {
    pub success: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Database {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DatabaseList {
    pub items: Vec<Database>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SchemaList {
    pub items: Vec<Schema>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Table,
    PartitionedTable,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    pub kind: RelationKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RelationList {
    pub items: Vec<Relation>,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColumnIdentity {
    Always,
    ByDefault,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColumnGenerated {
    Stored,
    Virtual,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub position: i16,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    pub identity: Option<ColumnIdentity>,
    pub generated: Option<ColumnGenerated>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ColumnList {
    pub items: Vec<Column>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub method: String,
    pub unique: bool,
    pub primary: bool,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct IndexList {
    pub items: Vec<Index>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SqlFile {
    pub id: Uuid,
    pub name: String,
    pub size_bytes: u32,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SqlFileContent {
    #[serde(flatten)]
    pub file: SqlFile,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SqlFileList {
    pub items: Vec<SqlFile>,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Serialize)]
struct CreateSqlFileRequest<'a> {
    name: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct UpdateSqlFileRequest<'a> {
    content: &'a str,
    expected_sha256: &'a str,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct ExecutionResult {
    pub statement_count: u64,
    pub rows_returned: u64,
    pub rows_affected: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExecutionError {
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Execution {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub database: String,
    pub status: ExecutionStatus,
    /// Server-measured duration, present once the execution finished.
    pub duration_ms: Option<u64>,
    pub result: Option<ExecutionResult>,
    pub error: Option<ExecutionError>,
}

/// Columns and text values of the last statement, possibly truncated.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ExecutionRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub stored_rows: u64,
    pub total_rows: u64,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct StartedExecution {
    pub id: Uuid,
    pub status: ExecutionStatus,
}

#[derive(Serialize)]
struct ExecutionRequest<'a> {
    sql: &'a str,
}

/// One page of a relation's rows as PostgreSQL text values.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RelationRowsPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_rows: u64,
    pub page: u64,
    pub per_page: u64,
}

/// Page, order and search of a relation rows request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationRowsRequest<'a> {
    pub page: u64,
    pub per_page: u64,
    /// Column name and whether the order is descending.
    pub sort: Option<(&'a str, bool)>,
    pub search: &'a str,
}

/// Identifies one relation in the catalog of a saved connection.
#[derive(Clone, Copy, Debug)]
pub struct RelationRef<'a> {
    pub connection_id: Uuid,
    pub database: &'a str,
    pub schema: &'a str,
    pub relation: &'a str,
}

impl RelationRef<'_> {
    fn path(&self, detail: &str) -> String {
        format!(
            "api/v1/connections/{}/databases/{}/schemas/{}/relations/{}/{detail}",
            self.connection_id,
            utf8_percent_encode(self.database, NON_ALPHANUMERIC),
            utf8_percent_encode(self.schema, NON_ALPHANUMERIC),
            utf8_percent_encode(self.relation, NON_ALPHANUMERIC),
        )
    }
}

impl ApiClient {
    pub fn new(base_url: &str) -> Result<Self, ApiError> {
        let mut base_url = Url::parse(base_url).map_err(|_| ApiError::InvalidBaseUrl)?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(ApiError::InvalidBaseUrl);
        }
        let base_path = base_url.path().trim_end_matches('/');
        base_url.set_path(&format!("{base_path}/"));
        let http = Client::builder()
            .timeout(API_TIMEOUT)
            .build()
            .map_err(|_| ApiError::ClientConfiguration)?;

        Ok(Self {
            base_url,
            http,
            access_token: None,
        })
    }

    pub fn from_env() -> Result<Self, ApiError> {
        let base_url = match env::var(API_BASE_URL_ENV) {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => DEFAULT_API_BASE_URL.to_owned(),
            Err(env::VarError::NotUnicode(_)) => return Err(ApiError::InvalidBaseUrl),
        };
        Self::new(&base_url)
    }

    pub async fn health(&self) -> Result<Health, ApiError> {
        self.get_json("api/v1/health", false).await
    }

    pub async fn version(&self) -> Result<Version, ApiError> {
        self.get_json("api/v1/version", false).await
    }

    pub async fn login(&mut self, email: &str, password: &str) -> Result<AuthSession, ApiError> {
        let request = self
            .request(Method::POST, "api/v1/auth/login", false)?
            .json(&LoginRequest { email, password });
        let response: LoginResponse =
            decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await?;
        self.access_token = Some(Zeroizing::new(response.access_token));
        Ok(AuthSession {
            user_id: response.user_id,
            expires_in: response.expires_in,
        })
    }

    pub async fn current_session(&self) -> Result<CurrentSession, ApiError> {
        self.get_json("api/v1/auth/session", true).await
    }

    pub async fn logout(&mut self) -> Result<(), ApiError> {
        let response = self
            .request(Method::POST, "api/v1/auth/logout", true)?
            .send()
            .await
            .map_err(|_| ApiError::Transport)?;
        self.access_token = None;
        expect_no_content(response).await
    }

    pub async fn list_connections(
        &self,
        page: u64,
        per_page: u64,
    ) -> Result<ConnectionList, ApiError> {
        let mut url = self.url("api/v1/connections")?;
        url.query_pairs_mut()
            .append_pair("page", &page.to_string())
            .append_pair("per_page", &per_page.to_string());
        let request = self.authenticated_request(self.http.get(url))?;
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    pub async fn get_connection(&self, id: Uuid) -> Result<Connection, ApiError> {
        self.get_json(&format!("api/v1/connections/{id}"), true)
            .await
    }

    pub async fn create_connection(&self, input: &ConnectionInput) -> Result<Connection, ApiError> {
        self.send_json(Method::POST, "api/v1/connections", input)
            .await
    }

    pub async fn update_connection(
        &self,
        id: Uuid,
        input: &ConnectionInput,
    ) -> Result<Connection, ApiError> {
        self.send_json(Method::PUT, &format!("api/v1/connections/{id}"), input)
            .await
    }

    pub async fn delete_connection(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .request(Method::DELETE, &format!("api/v1/connections/{id}"), true)?
            .send()
            .await
            .map_err(|_| ApiError::Transport)?;
        expect_no_content(response).await
    }

    pub async fn list_sql_files(&self, page: u64, per_page: u64) -> Result<SqlFileList, ApiError> {
        let mut url = self.url("api/v1/sql-files")?;
        url.query_pairs_mut()
            .append_pair("page", &page.to_string())
            .append_pair("per_page", &per_page.to_string());
        let request = self.authenticated_request(self.http.get(url))?;
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    pub async fn create_sql_file(&self, name: &str, content: &str) -> Result<SqlFile, ApiError> {
        self.send_json(
            Method::POST,
            "api/v1/sql-files",
            &CreateSqlFileRequest { name, content },
        )
        .await
    }

    pub async fn read_sql_file(&self, id: Uuid) -> Result<SqlFileContent, ApiError> {
        self.get_json(&format!("api/v1/sql-files/{id}"), true).await
    }

    /// Replaces the content only if the server still holds `expected_sha256`.
    pub async fn update_sql_file(
        &self,
        id: Uuid,
        content: &str,
        expected_sha256: &str,
    ) -> Result<SqlFile, ApiError> {
        self.send_json(
            Method::PUT,
            &format!("api/v1/sql-files/{id}"),
            &UpdateSqlFileRequest {
                content,
                expected_sha256,
            },
        )
        .await
    }

    pub async fn delete_sql_file(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .request(Method::DELETE, &format!("api/v1/sql-files/{id}"), true)?
            .send()
            .await
            .map_err(|_| ApiError::Transport)?;
        expect_no_content(response).await
    }

    pub async fn start_execution(
        &self,
        connection_id: Uuid,
        database: &str,
        sql: &str,
    ) -> Result<StartedExecution, ApiError> {
        let database = utf8_percent_encode(database, NON_ALPHANUMERIC);
        self.send_json(
            Method::POST,
            &format!("api/v1/connections/{connection_id}/databases/{database}/executions"),
            &ExecutionRequest { sql },
        )
        .await
    }

    pub async fn execution(&self, id: Uuid) -> Result<Execution, ApiError> {
        self.get_json(&format!("api/v1/executions/{id}"), true)
            .await
    }

    /// Asks the server to stop a running execution.
    pub async fn cancel_execution(&self, id: Uuid) -> Result<(), ApiError> {
        let response = self
            .request(
                Method::POST,
                &format!("api/v1/executions/{id}/cancel"),
                true,
            )?
            .send()
            .await
            .map_err(|_| ApiError::Transport)?;
        expect_no_content(response).await
    }

    pub async fn execution_rows(&self, id: Uuid) -> Result<ExecutionRows, ApiError> {
        self.get_json(&format!("api/v1/executions/{id}/result"), true)
            .await
    }

    pub async fn test_connection(&self, id: Uuid) -> Result<ConnectionTest, ApiError> {
        let request = self.request(Method::POST, &format!("api/v1/connections/{id}/test"), true)?;
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    pub async fn list_databases(&self, connection_id: Uuid) -> Result<DatabaseList, ApiError> {
        self.get_json(
            &format!("api/v1/connections/{connection_id}/databases"),
            true,
        )
        .await
    }

    pub async fn list_schemas(
        &self,
        connection_id: Uuid,
        database: &str,
    ) -> Result<SchemaList, ApiError> {
        let database = utf8_percent_encode(database, NON_ALPHANUMERIC);
        self.get_json(
            &format!("api/v1/connections/{connection_id}/databases/{database}/schemas"),
            true,
        )
        .await
    }

    pub async fn list_relations(
        &self,
        connection_id: Uuid,
        database: &str,
        schema: &str,
        page: u64,
        per_page: u64,
    ) -> Result<RelationList, ApiError> {
        let database = utf8_percent_encode(database, NON_ALPHANUMERIC);
        let schema = utf8_percent_encode(schema, NON_ALPHANUMERIC);
        let mut url = self.url(&format!(
            "api/v1/connections/{connection_id}/databases/{database}/schemas/{schema}/relations"
        ))?;
        url.query_pairs_mut()
            .append_pair("page", &page.to_string())
            .append_pair("per_page", &per_page.to_string());
        let request = self.authenticated_request(self.http.get(url))?;
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    pub async fn list_columns(&self, relation: RelationRef<'_>) -> Result<ColumnList, ApiError> {
        self.get_json(&relation.path("columns"), true).await
    }

    pub async fn list_relation_rows(
        &self,
        relation: RelationRef<'_>,
        request: RelationRowsRequest<'_>,
    ) -> Result<RelationRowsPage, ApiError> {
        let mut url = self.url(&relation.path("rows"))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("page", &request.page.to_string())
                .append_pair("per_page", &request.per_page.to_string());
            if let Some((column, descending)) = request.sort {
                query
                    .append_pair("sort", column)
                    .append_pair("direction", if descending { "desc" } else { "asc" });
            }
            if !request.search.is_empty() {
                query.append_pair("search", request.search);
            }
        }
        let request = self.authenticated_request(self.http.get(url))?;
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    pub async fn list_indexes(&self, relation: RelationRef<'_>) -> Result<IndexList, ApiError> {
        self.get_json(&relation.path("indexes"), true).await
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        authenticated: bool,
    ) -> Result<T, ApiError> {
        let request = self.request(Method::GET, path, authenticated)?;
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    async fn send_json<T: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &T,
    ) -> Result<R, ApiError> {
        let request = self.request(method, path, true)?.json(body);
        decode_response(request.send().await.map_err(|_| ApiError::Transport)?).await
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        authenticated: bool,
    ) -> Result<RequestBuilder, ApiError> {
        let request = self.http.request(method, self.url(path)?);
        if authenticated {
            self.authenticated_request(request)
        } else {
            Ok(request)
        }
    }

    fn authenticated_request(&self, request: RequestBuilder) -> Result<RequestBuilder, ApiError> {
        let token = self
            .access_token
            .as_deref()
            .ok_or(ApiError::Unauthenticated)?;
        Ok(request.header(AUTHORIZATION, format!("Bearer {token}")))
    }

    fn url(&self, path: &str) -> Result<Url, ApiError> {
        self.base_url
            .join(path)
            .map_err(|_| ApiError::InvalidBaseUrl)
    }
}

async fn decode_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ApiError> {
    if !response.status().is_success() {
        return Err(http_error(response).await);
    }
    response.json().await.map_err(|_| ApiError::InvalidResponse)
}

async fn expect_no_content(response: reqwest::Response) -> Result<(), ApiError> {
    if response.status() == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(http_error(response).await)
    }
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
}

async fn http_error(response: reqwest::Response) -> ApiError {
    let status = response.status().as_u16();
    let code = response
        .json::<ErrorResponse>()
        .await
        .ok()
        .and_then(|body| match body.error.as_str() {
            "invalid_credentials" => Some("invalid_credentials"),
            "unauthorized" => Some("unauthorized"),
            "invalid_request" => Some("invalid_request"),
            "not_found" => Some("not_found"),
            "connection_name_conflict" => Some("connection_name_conflict"),
            "credential_encryption_unavailable" => Some("credential_encryption_unavailable"),
            "internal_error" => Some("internal_error"),
            "method_not_allowed" => Some("method_not_allowed"),
            "catalog_unavailable" => Some("catalog_unavailable"),
            "file_name_conflict" => Some("file_name_conflict"),
            "file_conflict" => Some("file_conflict"),
            "file_too_large" => Some("file_too_large"),
            "file_storage_unavailable" => Some("file_storage_unavailable"),
            "execution_limit_reached" => Some("execution_limit_reached"),
            "server_execution_limit_reached" => Some("server_execution_limit_reached"),
            "result_unavailable" => Some("result_unavailable"),
            "result_expired" => Some("result_expired"),
            "execution_not_running" => Some("execution_not_running"),
            "relation_not_readable" => Some("relation_not_readable"),
            _ => None,
        });
    ApiError::Http { status, code }
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        extract::{Path, Query, State},
        http::{HeaderMap, StatusCode, header::AUTHORIZATION},
        routing::{get, post},
    };
    use serde::{Deserialize, Serialize};

    const SQL_FILE_ID: &str = "7c0f9a6e-2a43-4b8e-9d1f-3f1c2b5a6d7e";

    #[derive(Deserialize)]
    struct RelationPageQuery {
        page: u64,
        per_page: u64,
    }
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::{ApiClient, ApiError, ConnectionInput};

    #[derive(Clone)]
    struct TestState {
        user_id: Uuid,
    }

    #[derive(Serialize)]
    struct TestLoginResponse {
        access_token: &'static str,
        expires_in: u64,
        user_id: Uuid,
    }

    async fn login(State(state): State<TestState>) -> Json<TestLoginResponse> {
        Json(TestLoginResponse {
            access_token: "mock-access-token",
            expires_in: 43200,
            user_id: state.user_id,
        })
    }

    async fn session(
        headers: HeaderMap,
        State(state): State<TestState>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer mock-access-token")
        {
            (
                StatusCode::OK,
                Json(serde_json::json!({"user_id": state.user_id})),
            )
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "unauthorized"})),
            )
        }
    }

    async fn logout(headers: HeaderMap) -> StatusCode {
        if headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer mock-access-token")
        {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::UNAUTHORIZED
        }
    }

    async fn internal_error() -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "internal_error",
                "diagnostic": "mock secret that must not be exposed"
            })),
        )
    }

    async fn start_server(state: TestState) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/api/v1/auth/login", post(login))
            .route("/api/v1/auth/session", get(session))
            .route("/api/v1/auth/logout", post(logout))
            .route(
                "/api/v1/version",
                get(|| async { Json(serde_json::json!({"version":"test"})) }),
            )
            .route(
                "/api/v1/connections/{id}/databases",
                get(|| async { Json(serde_json::json!({"items":[{"name":"app_db"}]})) }),
            )
            .route(
                "/api/v1/connections/{id}/databases/{database}/schemas",
                get(|Path((_, database)): Path<(Uuid, String)>| async move {
                    Json(serde_json::json!({"items":[{"name": database}]}))
                }),
            )
            .route(
                "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations",
                get(
                    |Path((_, database, schema)): Path<(Uuid, String, String)>,
                     Query(page): Query<RelationPageQuery>| async move {
                        Json(serde_json::json!({
                            "items": [{"name": format!("{database}.{schema}"), "kind": "materialized_view"}],
                            "page": page.page,
                            "per_page": page.per_page,
                        }))
                    },
                ),
            )
            .route(
                "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}/columns",
                get(
                    |Path((_, _, _, relation)): Path<(Uuid, String, String, String)>| async move {
                        Json(serde_json::json!({"items": [{
                            "name": relation, "position": 1, "data_type": "integer",
                            "nullable": false, "default": null, "primary_key": true,
                            "identity": "by_default", "generated": null,
                        }]}))
                    },
                ),
            )
            .route(
                "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}/indexes",
                get(|| async {
                    Json(serde_json::json!({"items": [{
                        "name": "pkey", "method": "btree", "unique": true, "primary": true,
                        "columns": ["id"],
                    }]}))
                }),
            )
            .route(
                "/api/v1/sql-files",
                get(|| async {
                    Json(serde_json::json!({
                        "items": [{"id": SQL_FILE_ID, "name": "a.sql", "size_bytes": 8, "sha256": "aa"}],
                        "page": 1,
                        "per_page": 100,
                    }))
                })
                .post(|Json(body): Json<serde_json::Value>| async move {
                    (
                        axum::http::StatusCode::CREATED,
                        Json(serde_json::json!({
                            "id": SQL_FILE_ID, "name": body["name"], "size_bytes": 8, "sha256": "bb",
                        })),
                    )
                }),
            )
            .route(
                "/api/v1/sql-files/{id}",
                get(|| async {
                    Json(serde_json::json!({
                        "id": SQL_FILE_ID, "name": "a.sql", "size_bytes": 8, "sha256": "aa",
                        "content": "SELECT 1",
                    }))
                })
                .put(|Json(body): Json<serde_json::Value>| async move {
                    if body["expected_sha256"] == "stale" {
                        (
                            axum::http::StatusCode::CONFLICT,
                            Json(serde_json::json!({"error": "file_conflict"})),
                        )
                    } else {
                        (
                            axum::http::StatusCode::OK,
                            Json(serde_json::json!({
                                "id": SQL_FILE_ID, "name": "a.sql", "size_bytes": 8, "sha256": "cc",
                            })),
                        )
                    }
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/connections/{id}/databases/{database}/executions",
                post(|Path((_, database)): Path<(Uuid, String)>| async move {
                    let limit = match database.as_str() {
                        "busy db" => Some("execution_limit_reached"),
                        "full server" => Some("server_execution_limit_reached"),
                        _ => None,
                    };
                    if let Some(limit) = limit {
                        return (
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            Json(serde_json::json!({"error": limit})),
                        );
                    }
                    (
                        axum::http::StatusCode::ACCEPTED,
                        Json(serde_json::json!({"id": SQL_FILE_ID, "status": "running"})),
                    )
                }),
            )
            .route(
                "/api/v1/executions/{id}",
                get(|| async {
                    Json(serde_json::json!({
                        "id": SQL_FILE_ID,
                        "connection_id": SQL_FILE_ID,
                        "database": "app",
                        "status": "failed",
                        "duration_ms": 12,
                        "error": {"code": "42601", "message": "syntax error"},
                    }))
                }),
            )
            .route(
                "/api/v1/executions/{id}/result",
                get(|| async {
                    (
                        axum::http::StatusCode::GONE,
                        Json(serde_json::json!({"error": "result_expired"})),
                    )
                }),
            )
            .route(
                "/api/v1/executions/{id}/cancel",
                post(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}/rows",
                get(|Query(query): Query<std::collections::HashMap<String, String>>| async move {
                    Json(serde_json::json!({
                        "columns": ["query"],
                        "rows": [[format!(
                            "{}|{}|{}|{}|{}",
                            query["page"],
                            query["per_page"],
                            query.get("sort").map_or("-", String::as_str),
                            query.get("direction").map_or("-", String::as_str),
                            query.get("search").map_or("-", String::as_str),
                        )], [null]],
                        "total_rows": 2,
                        "page": 1,
                        "per_page": 25,
                    }))
                }),
            )
            .route("/api/v1/failure", get(internal_error))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test API");
        let address = listener.local_addr().expect("read test API address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test API");
        });
        (format!("http://{address}"), task)
    }

    #[test]
    fn rejects_non_http_urls_and_embedded_credentials() {
        assert_eq!(
            ApiClient::new("ftp://localhost").err(),
            Some(ApiError::InvalidBaseUrl)
        );
        assert_eq!(
            ApiClient::new("http://user:password@localhost").err(),
            Some(ApiError::InvalidBaseUrl)
        );
    }

    #[tokio::test]
    async fn login_session_and_logout_use_bearer_without_returning_the_token() {
        let user_id = Uuid::new_v4();
        let (base_url, server) = start_server(TestState { user_id }).await;
        let mut client = ApiClient::new(&base_url).expect("valid test API URL");
        let login = client
            .login("user@example.test", "secret-password")
            .await
            .expect("login succeeds");
        assert_eq!(login.user_id, user_id);
        assert_eq!(login.expires_in, 43200);
        assert_eq!(
            client
                .current_session()
                .await
                .expect("session succeeds")
                .user_id,
            user_id
        );
        assert_eq!(
            client
                .list_databases(Uuid::new_v4())
                .await
                .expect("database catalog succeeds")
                .items,
            vec![super::Database {
                name: "app_db".to_owned()
            }]
        );
        assert_eq!(
            client
                .list_schemas(Uuid::new_v4(), "odd/db name?%")
                .await
                .expect("schema catalog succeeds")
                .items,
            vec![super::Schema {
                name: "odd/db name?%".to_owned()
            }]
        );
        assert_eq!(
            client
                .list_relations(Uuid::new_v4(), "odd/db", "my schema", 3, 100)
                .await
                .expect("relation catalog succeeds"),
            super::RelationList {
                items: vec![super::Relation {
                    name: "odd/db.my schema".to_owned(),
                    kind: super::RelationKind::MaterializedView,
                }],
                page: 3,
                per_page: 100,
            }
        );
        let relation = super::RelationRef {
            connection_id: Uuid::new_v4(),
            database: "db",
            schema: "public",
            relation: "odd/table name",
        };
        let rows = client
            .list_relation_rows(
                relation,
                super::RelationRowsRequest {
                    page: 3,
                    per_page: 50,
                    sort: Some(("Mixed Col", true)),
                    search: "50% off&x=1",
                },
            )
            .await
            .expect("relation rows");
        assert_eq!(
            rows.rows,
            vec![
                vec![Some("3|50|Mixed Col|desc|50% off&x=1".to_owned())],
                vec![None]
            ]
        );
        let unsorted = client
            .list_relation_rows(
                relation,
                super::RelationRowsRequest {
                    page: 1,
                    per_page: 25,
                    sort: None,
                    search: "",
                },
            )
            .await
            .expect("relation rows");
        assert_eq!(unsorted.rows[0], vec![Some("1|25|-|-|-".to_owned())]);
        assert_eq!(
            client
                .list_columns(relation)
                .await
                .expect("column catalog succeeds")
                .items,
            vec![super::Column {
                name: "odd/table name".to_owned(),
                position: 1,
                data_type: "integer".to_owned(),
                nullable: false,
                default: None,
                primary_key: true,
                identity: Some(super::ColumnIdentity::ByDefault),
                generated: None,
            }]
        );
        assert_eq!(
            client
                .list_indexes(relation)
                .await
                .expect("index catalog succeeds")
                .items,
            vec![super::Index {
                name: "pkey".to_owned(),
                method: "btree".to_owned(),
                unique: true,
                primary: true,
                columns: vec!["id".to_owned()],
            }]
        );
        let file_id = Uuid::parse_str(SQL_FILE_ID).expect("valid file id");
        let listed = client.list_sql_files(1, 100).await.expect("list files");
        assert_eq!(listed.items[0].name, "a.sql");
        assert_eq!(
            client
                .create_sql_file("new.sql", "SELECT 1")
                .await
                .expect("create file")
                .name,
            "new.sql"
        );
        let read = client.read_sql_file(file_id).await.expect("read file");
        assert_eq!(read.content, "SELECT 1");
        assert_eq!(read.file.sha256, "aa");
        assert_eq!(
            client
                .update_sql_file(file_id, "SELECT 2", "aa")
                .await
                .expect("update file")
                .sha256,
            "cc"
        );
        assert_eq!(
            client.update_sql_file(file_id, "SELECT 2", "stale").await,
            Err(ApiError::Http {
                status: 409,
                code: Some("file_conflict"),
            })
        );
        client.delete_sql_file(file_id).await.expect("delete file");
        let started = client
            .start_execution(file_id, "app", "SELEC 1")
            .await
            .expect("start execution");
        assert_eq!(started.status, super::ExecutionStatus::Running);
        assert_eq!(
            client.start_execution(file_id, "busy db", "SELECT 1").await,
            Err(ApiError::Http {
                status: 429,
                code: Some("execution_limit_reached"),
            })
        );
        assert_eq!(
            client
                .start_execution(file_id, "full server", "SELECT 1")
                .await,
            Err(ApiError::Http {
                status: 429,
                code: Some("server_execution_limit_reached"),
            })
        );
        client
            .cancel_execution(started.id)
            .await
            .expect("cancel execution");
        let execution = client.execution(started.id).await.expect("execution");
        assert_eq!(execution.status, super::ExecutionStatus::Failed);
        assert_eq!(execution.duration_ms, Some(12));
        assert_eq!(
            client.execution_rows(started.id).await,
            Err(ApiError::Http {
                status: 410,
                code: Some("result_expired"),
            })
        );
        assert_eq!(execution.result, None);
        assert_eq!(
            execution.error,
            Some(super::ExecutionError {
                code: Some("42601".to_owned()),
                message: "syntax error".to_owned(),
            })
        );
        client.logout().await.expect("logout succeeds");
        assert_eq!(
            client.current_session().await,
            Err(ApiError::Unauthenticated)
        );
        server.abort();
    }

    #[tokio::test]
    async fn errors_hide_untrusted_server_body_and_connection_types_are_serializable() {
        let (base_url, server) = start_server(TestState {
            user_id: Uuid::new_v4(),
        })
        .await;
        let client = ApiClient::new(&base_url).expect("valid test API URL");
        let error = client
            .get_json::<serde_json::Value>("api/v1/failure", false)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            ApiError::Http {
                status: 500,
                code: Some("internal_error")
            }
        );
        assert!(!error.to_string().contains("mock secret"));

        let input = ConnectionInput {
            name: "local".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 5432,
            database: "app".to_owned(),
            username: "app".to_owned(),
            tls_enabled: true,
            read_only: true,
            password: Some("do-not-log".to_owned()),
        };
        let encoded = serde_json::to_value(input).expect("connection request serializes");
        assert_eq!(encoded["name"], "local");
        assert_eq!(encoded["read_only"], true);
        server.abort();
    }
}
