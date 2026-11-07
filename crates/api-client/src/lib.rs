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
}

#[derive(Serialize)]
pub struct ConnectionInput {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub tls_enabled: bool,
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
            password: Some("do-not-log".to_owned()),
        };
        let encoded = serde_json::to_value(input).expect("connection request serializes");
        assert_eq!(encoded["name"], "local");
        server.abort();
    }
}
