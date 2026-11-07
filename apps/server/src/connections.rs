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
    Column, ColumnGenerated, ColumnIdentity, ConnectionData, ConnectionSummary, PgPool,
    PostgresError, RelationKind, StoredConnectionCredentials, create_connection,
    delete_connection_for_owner, find_connection_credentials_for_owner, find_connection_for_owner,
    list_columns, list_connections_for_owner, list_databases, list_indexes, list_relations,
    list_schemas, test_database_connection, update_connection_for_owner,
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
        .route("/api/v1/connections/{id}/test", axum::routing::post(test))
        .route(
            "/api/v1/connections/{id}/databases",
            get(list_remote_databases),
        )
        .route(
            "/api/v1/connections/{id}/databases/{database}/schemas",
            get(list_remote_schemas),
        )
        .route(
            "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations",
            get(list_remote_relations),
        )
        .route(
            "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}/columns",
            get(list_remote_columns),
        )
        .route(
            "/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}/indexes",
            get(list_remote_indexes),
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

/// Validated `page`/`per_page` with the SQL limit and offset they select.
struct Page {
    page: u64,
    per_page: u64,
    limit: i64,
    offset: i64,
}

impl PaginationQuery {
    fn validate(self) -> Option<Page> {
        let page = self.page.unwrap_or(1);
        let per_page = self.per_page.unwrap_or(25);
        if page == 0 || per_page == 0 || per_page > 100 {
            return None;
        }
        let offset = page.checked_sub(1)?.checked_mul(per_page)?;
        Some(Page {
            page,
            per_page,
            limit: i64::try_from(per_page).ok()?,
            offset: i64::try_from(offset).ok()?,
        })
    }
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

#[derive(Serialize)]
struct ConnectionTestResponse {
    success: bool,
}

#[derive(Serialize)]
struct DatabaseListResponse {
    items: Vec<DatabaseResponse>,
}

#[derive(Serialize)]
struct DatabaseResponse {
    name: String,
}

#[derive(Serialize)]
struct SchemaListResponse {
    items: Vec<SchemaResponse>,
}

#[derive(Serialize)]
struct SchemaResponse {
    name: String,
}

#[derive(Serialize)]
struct RelationListResponse {
    items: Vec<RelationResponse>,
    page: u64,
    per_page: u64,
}

#[derive(Serialize)]
struct RelationResponse {
    name: String,
    kind: &'static str,
}

fn relation_kind_name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Table => "table",
        RelationKind::PartitionedTable => "partitioned_table",
        RelationKind::View => "view",
        RelationKind::MaterializedView => "materialized_view",
        RelationKind::ForeignTable => "foreign_table",
    }
}

#[derive(Serialize)]
struct ColumnListResponse {
    items: Vec<ColumnResponse>,
}

#[derive(Serialize)]
struct ColumnResponse {
    name: String,
    position: i16,
    data_type: String,
    nullable: bool,
    default: Option<String>,
    primary_key: bool,
    identity: Option<&'static str>,
    generated: Option<&'static str>,
}

impl From<Column> for ColumnResponse {
    fn from(column: Column) -> Self {
        Self {
            name: column.name,
            position: column.position,
            data_type: column.data_type,
            nullable: column.nullable,
            default: column.default,
            primary_key: column.primary_key,
            identity: column.identity.map(|identity| match identity {
                ColumnIdentity::Always => "always",
                ColumnIdentity::ByDefault => "by_default",
            }),
            generated: column.generated.map(|generated| match generated {
                ColumnGenerated::Stored => "stored",
                ColumnGenerated::Virtual => "virtual",
            }),
        }
    }
}

#[derive(Serialize)]
struct IndexListResponse {
    items: Vec<IndexResponse>,
}

#[derive(Serialize)]
struct IndexResponse {
    name: String,
    method: String,
    unique: bool,
    primary: bool,
    columns: Vec<String>,
}

/// PostgreSQL identifiers are truncated beyond NAMEDATALEN - 1 bytes.
const MAX_IDENTIFIER_BYTES: usize = 63;

fn is_valid_identifier(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_IDENTIFIER_BYTES && !name.contains('\0')
}

enum SavedConnectionError {
    NotFound,
    CredentialKeyUnavailable,
    Internal,
}

impl IntoResponse for SavedConnectionError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => not_found(),
            Self::CredentialKeyUnavailable => credential_configuration_error(),
            Self::Internal => internal_error(),
        }
    }
}

/// Loads an owner's saved connection and decrypts its password in memory.
async fn load_saved_connection(
    pool: &PgPool,
    owner_id: Uuid,
    connection_id: Uuid,
) -> Result<(StoredConnectionCredentials, Zeroizing<String>), SavedConnectionError> {
    let connection = find_connection_credentials_for_owner(pool, owner_id, connection_id)
        .await
        .map_err(|_| SavedConnectionError::Internal)?
        .ok_or(SavedConnectionError::NotFound)?;
    let cipher =
        CredentialCipher::from_env().map_err(|_| SavedConnectionError::CredentialKeyUnavailable)?;
    let password = cipher
        .decrypt(&connection.credential_envelope, owner_id, connection_id)
        .map_err(|_| SavedConnectionError::Internal)?;
    let password = std::str::from_utf8(&password).map_err(|_| SavedConnectionError::Internal)?;
    Ok((connection, Zeroizing::new(password.to_owned())))
}

async fn test(
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
    let (connection, password) = match load_saved_connection(&pool, owner_id, connection_id).await {
        Ok(saved) => saved,
        Err(error) => return error.into_response(),
    };
    let success = test_database_connection(&connection, &password)
        .await
        .is_ok();
    (StatusCode::OK, Json(ConnectionTestResponse { success })).into_response()
}

async fn list_remote_databases(
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
    let (connection, password) = match load_saved_connection(&pool, owner_id, connection_id).await {
        Ok(saved) => saved,
        Err(error) => return error.into_response(),
    };

    match list_databases(&connection, &password).await {
        Ok(items) => (
            StatusCode::OK,
            Json(DatabaseListResponse {
                items: items
                    .into_iter()
                    .map(|name| DatabaseResponse { name })
                    .collect(),
            }),
        )
            .into_response(),
        Err(_) => catalog_unavailable(),
    }
}

async fn list_remote_schemas(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    schema_path: Result<Path<(Uuid, String)>, PathRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path((connection_id, database))) = schema_path else {
        return bad_request();
    };
    if !is_valid_identifier(&database) {
        return bad_request();
    }
    let (connection, password) = match load_saved_connection(&pool, owner_id, connection_id).await {
        Ok(saved) => saved,
        Err(error) => return error.into_response(),
    };

    match list_schemas(&connection, &database, &password).await {
        Ok(items) => (
            StatusCode::OK,
            Json(SchemaListResponse {
                items: items
                    .into_iter()
                    .map(|name| SchemaResponse { name })
                    .collect(),
            }),
        )
            .into_response(),
        Err(_) => catalog_unavailable(),
    }
}

async fn list_remote_relations(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    relation_path: Result<Path<(Uuid, String, String)>, PathRejection>,
    query: Result<Query<PaginationQuery>, QueryRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path((connection_id, database, schema))) = relation_path else {
        return bad_request();
    };
    if !is_valid_identifier(&database) || !is_valid_identifier(&schema) {
        return bad_request();
    }
    let Some(Page {
        page,
        per_page,
        limit,
        offset,
    }) = query.ok().and_then(|Query(query)| query.validate())
    else {
        return bad_request();
    };
    let (connection, password) = match load_saved_connection(&pool, owner_id, connection_id).await {
        Ok(saved) => saved,
        Err(error) => return error.into_response(),
    };

    match list_relations(&connection, &database, &schema, &password, limit, offset).await {
        Ok(items) => (
            StatusCode::OK,
            Json(RelationListResponse {
                items: items
                    .into_iter()
                    .map(|relation| RelationResponse {
                        name: relation.name,
                        kind: relation_kind_name(relation.kind),
                    })
                    .collect(),
                page,
                per_page,
            }),
        )
            .into_response(),
        Err(_) => catalog_unavailable(),
    }
}

type RelationDetailPath = Path<(Uuid, String, String, String)>;

struct RelationTarget {
    connection: StoredConnectionCredentials,
    password: Zeroizing<String>,
    database: String,
    schema: String,
    relation: String,
}

enum RelationTargetError {
    Authentication(AuthenticationError),
    BadRequest,
    SavedConnection(SavedConnectionError),
}

impl IntoResponse for RelationTargetError {
    fn into_response(self) -> Response {
        match self {
            Self::Authentication(error) => authentication_error_response(error),
            Self::BadRequest => bad_request(),
            Self::SavedConnection(error) => error.into_response(),
        }
    }
}

/// Authenticates, validates a relation detail path and loads its connection.
async fn relation_detail_target(
    pool: &PgPool,
    headers: &HeaderMap,
    path: Result<RelationDetailPath, PathRejection>,
) -> Result<RelationTarget, RelationTargetError> {
    let owner_id = authenticated_owner(pool, headers)
        .await
        .map_err(RelationTargetError::Authentication)?;
    let Ok(Path((connection_id, database, schema, relation))) = path else {
        return Err(RelationTargetError::BadRequest);
    };
    if ![&database, &schema, &relation]
        .iter()
        .all(|name| is_valid_identifier(name))
    {
        return Err(RelationTargetError::BadRequest);
    }
    let (connection, password) = load_saved_connection(pool, owner_id, connection_id)
        .await
        .map_err(RelationTargetError::SavedConnection)?;
    Ok(RelationTarget {
        connection,
        password,
        database,
        schema,
        relation,
    })
}

async fn list_remote_columns(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    path: Result<RelationDetailPath, PathRejection>,
) -> Response {
    let target = match relation_detail_target(&pool, &headers, path).await {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    match list_columns(
        &target.connection,
        &target.database,
        &target.schema,
        &target.relation,
        &target.password,
    )
    .await
    {
        Ok(items) => (
            StatusCode::OK,
            Json(ColumnListResponse {
                items: items.into_iter().map(ColumnResponse::from).collect(),
            }),
        )
            .into_response(),
        Err(_) => catalog_unavailable(),
    }
}

async fn list_remote_indexes(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    path: Result<RelationDetailPath, PathRejection>,
) -> Response {
    let target = match relation_detail_target(&pool, &headers, path).await {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    match list_indexes(
        &target.connection,
        &target.database,
        &target.schema,
        &target.relation,
        &target.password,
    )
    .await
    {
        Ok(items) => (
            StatusCode::OK,
            Json(IndexListResponse {
                items: items
                    .into_iter()
                    .map(|index| IndexResponse {
                        name: index.name,
                        method: index.method,
                        unique: index.unique,
                        primary: index.primary,
                        columns: index.columns,
                    })
                    .collect(),
            }),
        )
            .into_response(),
        Err(_) => catalog_unavailable(),
    }
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

    let Some(Page {
        page,
        per_page,
        limit,
        offset,
    }) = query.ok().and_then(|Query(query)| query.validate())
    else {
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

fn catalog_unavailable() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: "catalog_unavailable",
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
        let test_password = env::var("PGRUSTADMIN_TEST_CONNECTION_PASSWORD")
            .expect("test connection password must be configured");
        let test_port = env::var("PGRUSTADMIN_TEST_CONNECTION_PORT")
            .expect("test connection port must be configured")
            .parse::<u16>()
            .expect("test connection port must be valid");
        let payload = serde_json::json!({
            "name": "authorization test",
            "host": "127.0.0.1",
            "port": test_port,
            "database": "pgrustadmin_test",
            "username": "pgrustadmin",
            "tls_enabled": false,
            "password": test_password
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
        assert_eq!(created_json["tls_enabled"], false);
        assert!(created_json.get("password").is_none());
        assert!(created_json.get("credential_envelope").is_none());

        let connection_test = send_request(
            app.clone(),
            Method::POST,
            &format!("/api/v1/connections/{connection_id}/test"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(connection_test.status(), StatusCode::OK);
        let test_body = to_bytes(connection_test.into_body(), 4096)
            .await
            .expect("read connection test response");
        let test_json: serde_json::Value =
            serde_json::from_slice(&test_body).expect("decode connection test response");
        assert_eq!(test_json, serde_json::json!({"success": true}));

        let databases = send_request(
            app.clone(),
            Method::GET,
            &format!("/api/v1/connections/{connection_id}/databases"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(databases.status(), StatusCode::OK);
        let databases_body = to_bytes(databases.into_body(), 4096)
            .await
            .expect("read database catalog response");
        let databases_json: serde_json::Value =
            serde_json::from_slice(&databases_body).expect("decode database catalog response");
        let database_names = databases_json["items"]
            .as_array()
            .expect("catalog response has items")
            .iter()
            .filter_map(|database| database["name"].as_str())
            .collect::<Vec<_>>();
        assert!(database_names.contains(&"pgrustadmin_test"));
        assert!(database_names.windows(2).all(|pair| pair[0] <= pair[1]));

        let unauthenticated_databases = send_request(
            app.clone(),
            Method::GET,
            &format!("/api/v1/connections/{connection_id}/databases"),
            None,
            None,
        )
        .await;
        assert_eq!(unauthenticated_databases.status(), StatusCode::UNAUTHORIZED);

        let other_owner_databases = send_request(
            app.clone(),
            Method::GET,
            &format!("/api/v1/connections/{connection_id}/databases"),
            Some(other_owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(other_owner_databases.status(), StatusCode::NOT_FOUND);

        let schemas_path =
            format!("/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas");
        let schemas = send_request(
            app.clone(),
            Method::GET,
            &schemas_path,
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(schemas.status(), StatusCode::OK);
        let schemas_body = to_bytes(schemas.into_body(), 4096)
            .await
            .expect("read schema catalog response");
        let schemas_json: serde_json::Value =
            serde_json::from_slice(&schemas_body).expect("decode schema catalog response");
        let schema_names = schemas_json["items"]
            .as_array()
            .expect("schema response has items")
            .iter()
            .filter_map(|schema| schema["name"].as_str())
            .collect::<Vec<_>>();
        for expected in ["information_schema", "pg_catalog", "public"] {
            assert!(
                schema_names.contains(&expected),
                "missing schema {expected}"
            );
        }
        assert!(
            schema_names
                .iter()
                .all(|name| !name.starts_with("pg_toast") && !name.starts_with("pg_temp_"))
        );
        assert!(schema_names.windows(2).all(|pair| pair[0] <= pair[1]));

        let other_owner_schemas = send_request(
            app.clone(),
            Method::GET,
            &schemas_path,
            Some(other_owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(other_owner_schemas.status(), StatusCode::NOT_FOUND);

        let overlong_database = "a".repeat(64);
        let invalid_database_schemas = send_request(
            app.clone(),
            Method::GET,
            &format!("/api/v1/connections/{connection_id}/databases/{overlong_database}/schemas"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(invalid_database_schemas.status(), StatusCode::BAD_REQUEST);

        let missing_database_schemas = send_request(
            app.clone(),
            Method::GET,
            &format!("/api/v1/connections/{connection_id}/databases/pgrustadmin_missing/schemas"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(missing_database_schemas.status(), StatusCode::BAD_GATEWAY);

        let catalog_schema = format!("cat003_{}", Uuid::new_v4().simple());
        for statement in [
            format!("CREATE SCHEMA {catalog_schema}"),
            format!(
                "CREATE TABLE {catalog_schema}.a_table (\
                 id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
                 email varchar(255) NOT NULL, \
                 created_at timestamptz DEFAULT now(), \
                 email_lower text GENERATED ALWAYS AS (lower(email)) STORED)"
            ),
            format!(
                "CREATE UNIQUE INDEX a_table_email_lower_idx ON {catalog_schema}.a_table \
                 (lower(email)) INCLUDE (created_at)"
            ),
            format!(
                "CREATE INDEX a_table_created_at_idx ON {catalog_schema}.a_table \
                 (created_at DESC, email)"
            ),
            format!("CREATE TABLE {catalog_schema}.b_partitioned (id int) PARTITION BY RANGE (id)"),
            format!(
                "CREATE TABLE {catalog_schema}.b_partition_1 PARTITION OF \
                 {catalog_schema}.b_partitioned FOR VALUES FROM (0) TO (10)"
            ),
            format!("CREATE VIEW {catalog_schema}.c_view AS SELECT 1 AS id"),
            format!("CREATE MATERIALIZED VIEW {catalog_schema}.d_matview AS SELECT 1 AS id"),
            format!("CREATE SEQUENCE {catalog_schema}.e_sequence"),
        ] {
            // Fixture names are generated from a UUID in this test.
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .expect("create catalog fixture");
        }
        let relations_path = format!(
            "/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas/{catalog_schema}/relations"
        );
        let relations = send_request(
            app.clone(),
            Method::GET,
            &format!("{relations_path}?per_page=100"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(relations.status(), StatusCode::OK);
        let relations_body = to_bytes(relations.into_body(), 4096)
            .await
            .expect("read relation catalog response");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&relations_body)
                .expect("decode relation catalog response"),
            serde_json::json!({
                "items": [
                    {"name": "a_table", "kind": "table"},
                    {"name": "b_partitioned", "kind": "partitioned_table"},
                    {"name": "c_view", "kind": "view"},
                    {"name": "d_matview", "kind": "materialized_view"},
                ],
                "page": 1,
                "per_page": 100,
            })
        );

        let second_page = send_request(
            app.clone(),
            Method::GET,
            &format!("{relations_path}?page=2&per_page=2"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(second_page.status(), StatusCode::OK);
        let second_page_body = to_bytes(second_page.into_body(), 4096)
            .await
            .expect("read relation page response");
        let second_page_json: serde_json::Value =
            serde_json::from_slice(&second_page_body).expect("decode relation page response");
        assert_eq!(
            second_page_json["items"],
            serde_json::json!([
                {"name": "c_view", "kind": "view"},
                {"name": "d_matview", "kind": "materialized_view"},
            ])
        );

        for (path, expected_status) in [
            (
                format!("{relations_path}?per_page=101"),
                StatusCode::BAD_REQUEST,
            ),
            (format!("{relations_path}?page=0"), StatusCode::BAD_REQUEST),
            (
                format!(
                    "/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas/{}/relations",
                    "s".repeat(64)
                ),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = send_request(
                app.clone(),
                Method::GET,
                &path,
                Some(owner_token.as_str()),
                None,
            )
            .await;
            assert_eq!(response.status(), expected_status, "{path}");
        }

        let missing_schema = send_request(
            app.clone(),
            Method::GET,
            &format!(
                "/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas/cat003_missing/relations"
            ),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(missing_schema.status(), StatusCode::OK);
        let missing_schema_body = to_bytes(missing_schema.into_body(), 4096)
            .await
            .expect("read missing schema response");
        let missing_schema_json: serde_json::Value =
            serde_json::from_slice(&missing_schema_body).expect("decode missing schema response");
        assert_eq!(missing_schema_json["items"], serde_json::json!([]));

        let a_table_path = format!(
            "/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas/{catalog_schema}/relations/a_table"
        );
        let columns = send_request(
            app.clone(),
            Method::GET,
            &format!("{a_table_path}/columns"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(columns.status(), StatusCode::OK);
        let columns_body = to_bytes(columns.into_body(), 4096)
            .await
            .expect("read column catalog response");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&columns_body)
                .expect("decode column catalog response"),
            serde_json::json!({"items": [
                {"name": "id", "position": 1, "data_type": "integer", "nullable": false,
                 "default": null, "primary_key": true, "identity": "always", "generated": null},
                {"name": "email", "position": 2, "data_type": "character varying(255)",
                 "nullable": false, "default": null, "primary_key": false, "identity": null,
                 "generated": null},
                {"name": "created_at", "position": 3, "data_type": "timestamp with time zone",
                 "nullable": true, "default": "now()", "primary_key": false, "identity": null,
                 "generated": null},
                {"name": "email_lower", "position": 4, "data_type": "text", "nullable": true,
                 "default": null, "primary_key": false, "identity": null, "generated": "stored"},
            ]})
        );

        let indexes = send_request(
            app.clone(),
            Method::GET,
            &format!("{a_table_path}/indexes"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(indexes.status(), StatusCode::OK);
        let indexes_body = to_bytes(indexes.into_body(), 4096)
            .await
            .expect("read index catalog response");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&indexes_body)
                .expect("decode index catalog response"),
            serde_json::json!({"items": [
                {"name": "a_table_created_at_idx", "method": "btree", "unique": false,
                 "primary": false, "columns": ["created_at DESC", "email"]},
                {"name": "a_table_email_lower_idx", "method": "btree", "unique": true,
                 "primary": false, "columns": ["lower(email::text)"]},
                {"name": "a_table_pkey", "method": "btree", "unique": true, "primary": true,
                 "columns": ["id"]},
            ]})
        );

        for detail in ["columns", "indexes"] {
            let missing_relation = send_request(
                app.clone(),
                Method::GET,
                &format!(
                    "/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas/{catalog_schema}/relations/missing/{detail}"
                ),
                Some(owner_token.as_str()),
                None,
            )
            .await;
            assert_eq!(missing_relation.status(), StatusCode::OK, "{detail}");
            let missing_relation_body = to_bytes(missing_relation.into_body(), 4096)
                .await
                .expect("read missing relation response");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&missing_relation_body)
                    .expect("decode missing relation response"),
                serde_json::json!({"items": []})
            );

            let invalid_relation = send_request(
                app.clone(),
                Method::GET,
                &format!(
                    "/api/v1/connections/{connection_id}/databases/pgrustadmin_test/schemas/{catalog_schema}/relations/{}/{detail}",
                    "r".repeat(64)
                ),
                Some(owner_token.as_str()),
                None,
            )
            .await;
            assert_eq!(
                invalid_relation.status(),
                StatusCode::BAD_REQUEST,
                "{detail}"
            );

            let other_owner_detail = send_request(
                app.clone(),
                Method::GET,
                &format!("{a_table_path}/{detail}"),
                Some(other_owner_token.as_str()),
                None,
            )
            .await;
            assert_eq!(
                other_owner_detail.status(),
                StatusCode::NOT_FOUND,
                "{detail}"
            );
        }

        let other_owner_relations = send_request(
            app.clone(),
            Method::GET,
            &relations_path,
            Some(other_owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(other_owner_relations.status(), StatusCode::NOT_FOUND);

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {catalog_schema} CASCADE"
        )))
        .execute(&pool)
        .await
        .expect("drop catalog fixture");

        let mut invalid_credentials = payload.clone();
        invalid_credentials["password"] = serde_json::json!("incorrect-test-password");
        let updated = send_request(
            app.clone(),
            Method::PUT,
            &format!("/api/v1/connections/{connection_id}"),
            Some(owner_token.as_str()),
            Some(invalid_credentials),
        )
        .await;
        assert_eq!(updated.status(), StatusCode::OK);
        let failed_test = send_request(
            app.clone(),
            Method::POST,
            &format!("/api/v1/connections/{connection_id}/test"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(failed_test.status(), StatusCode::OK);
        let failed_body = to_bytes(failed_test.into_body(), 4096)
            .await
            .expect("read failed connection test response");
        let failed_json: serde_json::Value =
            serde_json::from_slice(&failed_body).expect("decode failed connection test response");
        assert_eq!(failed_json, serde_json::json!({"success": false}));
        assert!(!String::from_utf8_lossy(&failed_body).contains("incorrect-test-password"));

        let failed_catalog = send_request(
            app.clone(),
            Method::GET,
            &format!("/api/v1/connections/{connection_id}/databases"),
            Some(owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(failed_catalog.status(), StatusCode::BAD_GATEWAY);
        let failed_catalog_body = to_bytes(failed_catalog.into_body(), 4096)
            .await
            .expect("read failed catalog response");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&failed_catalog_body)
                .expect("decode failed catalog response"),
            serde_json::json!({"error": "catalog_unavailable"})
        );
        assert!(!String::from_utf8_lossy(&failed_catalog_body).contains("incorrect-test-password"));

        let other_owner_test = send_request(
            app.clone(),
            Method::POST,
            &format!("/api/v1/connections/{connection_id}/test"),
            Some(other_owner_token.as_str()),
            None,
        )
        .await;
        assert_eq!(other_owner_test.status(), StatusCode::NOT_FOUND);

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
