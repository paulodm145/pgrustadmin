use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use pgrustadmin_postgres_driver::{
    ConnectionData, ConnectionSummary, PgPool, PostgresError, create_connection,
    delete_connection_for_owner, find_connection_for_owner, list_connections_for_owner,
    update_connection_for_owner,
};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{auth::session::session_user_id_from_headers, credentials::CredentialCipher};

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/api/v1/connections", get(list).post(create))
        .route(
            "/api/v1/connections/{id}",
            get(read).put(update).delete(remove),
        )
        .with_state(pool)
}

#[derive(Deserialize)]
struct ConnectionRequest {
    name: String,
    host: String,
    port: u64,
    database: String,
    username: String,
    #[serde(default = "tls_enabled_by_default")]
    tls_enabled: bool,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
struct PaginationQuery {
    page: Option<u64>,
    per_page: Option<u64>,
}

struct ValidatedConnection {
    name: String,
    host: String,
    port: u16,
    database_name: String,
    username: String,
    tls_enabled: bool,
    password: Option<Zeroizing<String>>,
}

#[derive(Clone, Copy)]
enum AuthenticationError {
    Unauthorized,
    Internal,
}

#[derive(Serialize)]
struct ConnectionResponse {
    id: Uuid,
    name: String,
    host: String,
    port: u16,
    database: String,
    username: String,
    tls_enabled: bool,
}

#[derive(Serialize)]
struct ConnectionListResponse {
    items: Vec<ConnectionResponse>,
    page: u64,
    per_page: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

async fn list(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    query: Result<Query<PaginationQuery>, QueryRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };

    let Ok(Query(query)) = query else {
        return bad_request();
    };
    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(25);
    if page == 0 || per_page == 0 || per_page > 100 {
        return bad_request();
    }
    let Some(offset) = page.checked_sub(1).and_then(|n| n.checked_mul(per_page)) else {
        return bad_request();
    };

    let Ok(offset) = i64::try_from(offset) else {
        return bad_request();
    };
    let Ok(limit) = i64::try_from(per_page) else {
        return bad_request();
    };

    match list_connections_for_owner(&pool, owner_id, limit, offset).await {
        Ok(items) => match items
            .into_iter()
            .map(ConnectionResponse::try_from)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(items) => (
                StatusCode::OK,
                Json(ConnectionListResponse {
                    items,
                    page,
                    per_page,
                }),
            )
                .into_response(),
            Err(()) => internal_error(),
        },
        Err(_) => internal_error(),
    }
}

async fn read(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    connection_path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path(connection_id)) = connection_path else {
        return bad_request();
    };

    match find_connection_for_owner(&pool, owner_id, connection_id).await {
        Ok(Some(connection)) => match ConnectionResponse::try_from(connection) {
            Ok(connection) => (StatusCode::OK, Json(connection)).into_response(),
            Err(()) => internal_error(),
        },
        Ok(None) => not_found(),
        Err(_) => internal_error(),
    }
}

async fn create(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    request: Result<Json<ConnectionRequest>, JsonRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Json(request)) = request else {
        return bad_request();
    };
    let validated = match validate_request(request, true) {
        Ok(validated) => validated,
        Err(()) => return bad_request(),
    };
    let Some(password) = validated.password.as_ref() else {
        return bad_request();
    };
    let connection_id = Uuid::new_v4();
    let cipher = match CredentialCipher::from_env() {
        Ok(cipher) => cipher,
        Err(_) => return credential_configuration_error(),
    };
    let credential_envelope = match cipher.encrypt(password.as_bytes(), owner_id, connection_id) {
        Ok(envelope) => envelope,
        Err(_) => return internal_error(),
    };

    let connection_data = connection_data(&validated);
    match create_connection(
        &pool,
        owner_id,
        connection_id,
        &connection_data,
        &credential_envelope,
    )
    .await
    {
        Ok(connection) => match ConnectionResponse::try_from(connection) {
            Ok(connection) => (StatusCode::CREATED, Json(connection)).into_response(),
            Err(()) => internal_error(),
        },
        Err(error) if is_unique_violation(&error) => conflict(),
        Err(_) => internal_error(),
    }
}

async fn update(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    connection_path: Result<Path<Uuid>, PathRejection>,
    request: Result<Json<ConnectionRequest>, JsonRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path(connection_id)) = connection_path else {
        return bad_request();
    };
    match find_connection_for_owner(&pool, owner_id, connection_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    }
    let Ok(Json(request)) = request else {
        return bad_request();
    };
    let validated = match validate_request(request, false) {
        Ok(validated) => validated,
        Err(()) => return bad_request(),
    };

    let credential_envelope = if let Some(password) = validated.password.as_ref() {
        let cipher = match CredentialCipher::from_env() {
            Ok(cipher) => cipher,
            Err(_) => return credential_configuration_error(),
        };
        match cipher.encrypt(password.as_bytes(), owner_id, connection_id) {
            Ok(envelope) => Some(envelope),
            Err(_) => return internal_error(),
        }
    } else {
        None
    };

    let connection_data = connection_data(&validated);
    match update_connection_for_owner(
        &pool,
        owner_id,
        connection_id,
        &connection_data,
        credential_envelope.as_deref(),
    )
    .await
    {
        Ok(Some(connection)) => match ConnectionResponse::try_from(connection) {
            Ok(connection) => (StatusCode::OK, Json(connection)).into_response(),
            Err(()) => internal_error(),
        },
        Ok(None) => not_found(),
        Err(error) if is_unique_violation(&error) => conflict(),
        Err(_) => internal_error(),
    }
}

async fn remove(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    connection_path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path(connection_id)) = connection_path else {
        return bad_request();
    };

    match delete_connection_for_owner(&pool, owner_id, connection_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(_) => internal_error(),
    }
}

async fn authenticated_owner(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Uuid, AuthenticationError> {
    match session_user_id_from_headers(pool, headers).await {
        Ok(Some(owner_id)) => Ok(owner_id),
        Ok(None) => Err(AuthenticationError::Unauthorized),
        Err(_) => Err(AuthenticationError::Internal),
    }
}

fn authentication_error_response(error: AuthenticationError) -> Response {
    match error {
        AuthenticationError::Unauthorized => unauthorized(),
        AuthenticationError::Internal => internal_error(),
    }
}

fn validate_request(
    request: ConnectionRequest,
    require_password: bool,
) -> Result<ValidatedConnection, ()> {
    let name = request.name.trim().to_owned();
    let host = request.host.trim().to_owned();
    let database_name = request.database.trim().to_owned();
    let username = request.username.trim().to_owned();
    let password = request.password.map(Zeroizing::new);

    if name.is_empty()
        || name.chars().count() > 100
        || host.is_empty()
        || host.chars().count() > 255
        || database_name.is_empty()
        || username.is_empty()
        || request.port == 0
        || request.port > u64::from(u16::MAX)
        || (require_password && password.is_none())
        || password.as_deref().is_some_and(|value| value.is_empty())
    {
        return Err(());
    }

    let port = u16::try_from(request.port).map_err(|_| ())?;
    Ok(ValidatedConnection {
        name,
        host,
        port,
        database_name,
        username,
        tls_enabled: request.tls_enabled,
        password,
    })
}

fn connection_data(connection: &ValidatedConnection) -> ConnectionData<'_> {
    ConnectionData {
        name: &connection.name,
        host: &connection.host,
        port: connection.port,
        database_name: &connection.database_name,
        username: &connection.username,
        tls_enabled: connection.tls_enabled,
    }
}

fn is_unique_violation(error: &PostgresError) -> bool {
    error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .as_deref()
        == Some("23505")
}

fn tls_enabled_by_default() -> bool {
    true
}

impl TryFrom<ConnectionSummary> for ConnectionResponse {
    type Error = ();

    fn try_from(connection: ConnectionSummary) -> Result<Self, Self::Error> {
        Ok(Self {
            id: connection.id,
            name: connection.name,
            host: connection.host,
            port: u16::try_from(connection.port).map_err(|_| ())?,
            database: connection.database_name,
            username: connection.username,
            tls_enabled: connection.tls_enabled,
        })
    }
}

fn credential_configuration_error() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "credential_encryption_unavailable",
        }),
    )
        .into_response()
}

fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid_request",
        }),
    )
        .into_response()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "unauthorized",
        }),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse { error: "not_found" }),
    )
        .into_response()
}

fn conflict() -> Response {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: "connection_name_conflict",
        }),
    )
        .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "internal_error",
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header::AUTHORIZATION},
        response::Response,
    };
    use pgrustadmin_postgres_driver::{
        connect_metadata_pool, create_session, run_metadata_migrations,
    };
    use std::env;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{ConnectionRequest, validate_request};

    fn request() -> ConnectionRequest {
        ConnectionRequest {
            name: "  dev db  ".to_owned(),
            host: " localhost ".to_owned(),
            port: 5432,
            database: " pgrustadmin ".to_owned(),
            username: " app ".to_owned(),
            tls_enabled: true,
            password: Some("secret".to_owned()),
        }
    }

    #[test]
    fn trims_fields_and_applies_tls_default() {
        let request: ConnectionRequest = serde_json::from_str(
            r#"{"name":" dev ","host":" localhost ","port":5432,"database":" db ","username":" user ","password":"secret"}"#,
        )
        .expect("connection request decodes");
        assert!(request.tls_enabled);
        let validated = validate_request(request, true).expect("valid connection request");

        assert_eq!(validated.name, "dev");
        assert_eq!(validated.host, "localhost");
        assert_eq!(validated.database_name, "db");
        assert_eq!(validated.username, "user");
        assert_eq!(validated.port, 5432);
        assert!(validated.tls_enabled);
        assert_eq!(
            validated.password.as_deref().map(String::as_str),
            Some("secret")
        );
    }

    #[test]
    fn rejects_invalid_fields_and_missing_create_password() {
        let mut invalid = request();
        invalid.name = "  ".to_owned();
        assert!(validate_request(invalid, true).is_err());

        let mut invalid = request();
        invalid.port = 0;
        assert!(validate_request(invalid, true).is_err());

        let mut invalid = request();
        invalid.port = 65536;
        assert!(validate_request(invalid, true).is_err());

        let mut no_password = request();
        no_password.password = None;
        assert!(validate_request(no_password, true).is_err());
        assert!(validate_request(request_without_password(), false).is_ok());
    }

    fn request_without_password() -> ConnectionRequest {
        let mut request = request();
        request.password = None;
        request
    }

    #[tokio::test]
    async fn connection_endpoints_enforce_owner_scope() {
        let database_url = env::var("PGRUSTADMIN_TEST_DATABASE_URL")
            .expect("PGRUSTADMIN_TEST_DATABASE_URL is required for database tests");
        let pool = connect_metadata_pool(&database_url)
            .await
            .expect("connect to the isolated test metadata database");
        run_metadata_migrations(&pool)
            .await
            .expect("apply metadata migrations to the test database");

        let owner_id = Uuid::new_v4();
        let other_owner_id = Uuid::new_v4();
        let owner_email = format!("{}@example.test", owner_id);
        let other_owner_email = format!("{}@example.test", other_owner_id);
        for (user_id, email) in [
            (owner_id, &owner_email),
            (other_owner_id, &other_owner_email),
        ] {
            sqlx::query(
                "INSERT INTO users (id, email, password_hash, is_active) \
                 VALUES ($1, $2, 'test-only-hash', TRUE)",
            )
            .bind(user_id)
            .bind(email)
            .execute(&pool)
            .await
            .expect("insert isolated test user");
        }

        let owner_token = crate::auth::session::SessionToken::generate();
        let other_owner_token = crate::auth::session::SessionToken::generate();
        create_session(&pool, owner_id, &owner_token.token_hash())
            .await
            .expect("create owner session");
        create_session(&pool, other_owner_id, &other_owner_token.token_hash())
            .await
            .expect("create second owner session");

        let app = super::router(pool.clone());
        let payload = serde_json::json!({
            "name": "authorization test",
            "host": "127.0.0.1",
            "port": 5432,
            "database": "sample",
            "username": "readonly",
            "password": "test-credential"
        });
        let created = send_request(
            app.clone(),
            Method::POST,
            "/api/v1/connections",
            Some(owner_token.as_str()),
            Some(payload.clone()),
        )
        .await;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created_body = to_bytes(created.into_body(), 4096)
            .await
            .expect("read creation response");
        let created_json: serde_json::Value =
            serde_json::from_slice(&created_body).expect("decode creation response");
        let connection_id = created_json["id"]
            .as_str()
            .expect("response has connection ID");
        assert_eq!(created_json["tls_enabled"], true);
        assert!(created_json.get("password").is_none());
        assert!(created_json.get("credential_envelope").is_none());

        for (method, body) in [
            (Method::GET, None),
            (Method::PUT, Some(payload.clone())),
            (Method::DELETE, None),
        ] {
            let response = send_request(
                app.clone(),
                method,
                &format!("/api/v1/connections/{connection_id}"),
                Some(other_owner_token.as_str()),
                body,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        let other_owner_list = send_request(
            app.clone(),
            Method::GET,
            "/api/v1/connections",
            Some(other_owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(other_owner_list.status(), StatusCode::OK);
        let list_body = to_bytes(other_owner_list.into_body(), 4096)
            .await
            .expect("read list response");
        let list_json: serde_json::Value =
            serde_json::from_slice(&list_body).expect("decode list response");
        assert_eq!(list_json["items"], serde_json::json!([]));

        let delete_owner_record = send_request(
            app,
            Method::DELETE,
            &format!("/api/v1/connections/{connection_id}"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(delete_owner_record.status(), StatusCode::NO_CONTENT);

        sqlx::query("DELETE FROM users WHERE id = $1 OR id = $2")
            .bind(owner_id)
            .bind(other_owner_id)
            .execute(&pool)
            .await
            .expect("remove isolated test users and sessions");
        pool.close().await;
    }

    async fn send_request(
        app: Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
        payload: Option<serde_json::Value>,
    ) -> Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if payload.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let body = payload.map_or_else(Body::empty, |value| Body::from(value.to_string()));
        app.oneshot(builder.body(body).expect("build test request"))
            .await
            .expect("connection router responds")
    }
}
