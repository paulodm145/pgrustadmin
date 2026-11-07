//! Owner-scoped `.sql` files: metadata in PostgreSQL, content on disk.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use pgrustadmin_postgres_driver::{
    NewSqlFile, PgPool, SqlFileRecord, create_sql_file, delete_sql_file_for_owner,
    find_sql_file_for_owner, list_sql_files_for_owner, update_sql_file_content_for_owner,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::connections::{
    Page, PaginationQuery, authenticated_owner, authentication_error_response, is_unique_violation,
};

pub const SQL_FILES_DIR_ENV: &str = "PGRUSTADMIN_SQL_FILES_DIR";
const MAX_CONTENT_BYTES: usize = 1_048_576;
/// JSON escaping can expand content well beyond its raw size.
const MAX_REQUEST_BYTES: usize = 8 * MAX_CONTENT_BYTES;
const MIN_NAME_BYTES: usize = 5;
const MAX_NAME_BYTES: usize = 255;

#[derive(Clone)]
struct SqlFilesState {
    pool: PgPool,
    storage_root: Option<PathBuf>,
}

pub fn router(pool: PgPool, storage_root: Option<PathBuf>) -> Router {
    Router::new()
        .route("/api/v1/sql-files", get(list).post(create))
        .route(
            "/api/v1/sql-files/{id}",
            get(read).put(update).delete(remove),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(SqlFilesState { pool, storage_root })
}

#[derive(Deserialize)]
struct CreateFileRequest {
    name: String,
    content: String,
}

#[derive(Deserialize)]
struct UpdateFileRequest {
    content: String,
    expected_sha256: String,
}

#[derive(Serialize)]
struct FileResponse {
    id: Uuid,
    name: String,
    size_bytes: i32,
    sha256: String,
}

impl From<SqlFileRecord> for FileResponse {
    fn from(file: SqlFileRecord) -> Self {
        Self {
            id: file.id,
            name: file.name,
            size_bytes: file.size_bytes,
            sha256: to_hex(&file.content_sha256),
        }
    }
}

#[derive(Serialize)]
struct FileContentResponse {
    #[serde(flatten)]
    file: FileResponse,
    content: String,
}

#[derive(Serialize)]
struct FileListResponse {
    items: Vec<FileResponse>,
    page: u64,
    per_page: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

/// Content directory of one deployment; paths derive only from UUIDs.
struct FileStorage {
    root: PathBuf,
}

impl FileStorage {
    fn path(&self, owner_id: Uuid, storage_key: Uuid) -> PathBuf {
        self.root
            .join(owner_id.to_string())
            .join(format!("{storage_key}.sql"))
    }

    async fn write_new(
        &self,
        owner_id: Uuid,
        storage_key: Uuid,
        content: Vec<u8>,
    ) -> io::Result<()> {
        let path = self.path(owner_id, storage_key);
        run_blocking(move || {
            if let Some(directory) = path.parent() {
                fs::create_dir_all(directory)?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(&content)?;
            file.sync_all()
        })
        .await
    }

    async fn read(&self, owner_id: Uuid, storage_key: Uuid) -> io::Result<Vec<u8>> {
        let path = self.path(owner_id, storage_key);
        run_blocking(move || fs::read(path)).await
    }

    async fn remove(&self, owner_id: Uuid, storage_key: Uuid) -> io::Result<()> {
        let path = self.path(owner_id, storage_key);
        run_blocking(move || match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        })
        .await
    }

    /// Removes content that is no longer referenced; failure only leaves an
    /// orphaned file behind, so it is logged instead of failing the request.
    async fn remove_unreferenced(&self, owner_id: Uuid, storage_key: Uuid) {
        if self.remove(owner_id, storage_key).await.is_err() {
            tracing::warn!("could not remove unreferenced SQL file content");
        }
    }
}

async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(io::Error::other)?
}

/// The configured content directory, when it exists.
async fn storage(state: &SqlFilesState) -> Option<FileStorage> {
    let root = state.storage_root.clone()?;
    let probe = root.clone();
    match run_blocking(move || fs::metadata(probe)).await {
        Ok(metadata) if metadata.is_dir() => Some(FileStorage { root }),
        _ => None,
    }
}

async fn list(
    State(state): State<SqlFilesState>,
    headers: HeaderMap,
    query: Result<Query<PaginationQuery>, QueryRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    if storage(&state).await.is_none() {
        return storage_unavailable();
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
    match list_sql_files_for_owner(&state.pool, owner_id, limit, offset).await {
        Ok(files) => Json(FileListResponse {
            items: files.into_iter().map(FileResponse::from).collect(),
            page,
            per_page,
        })
        .into_response(),
        Err(_) => internal_error(),
    }
}

async fn create(
    State(state): State<SqlFilesState>,
    headers: HeaderMap,
    request: Result<Json<CreateFileRequest>, JsonRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Some(storage) = storage(&state).await else {
        return storage_unavailable();
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return json_rejection(&rejection),
    };
    if !is_valid_file_name(&request.name) {
        return bad_request();
    }
    let Some(size_bytes) = content_size(&request.content) else {
        return file_too_large();
    };
    let content = request.content.into_bytes();
    let content_sha256 = Sha256::digest(&content);
    let storage_key = Uuid::new_v4();
    if storage
        .write_new(owner_id, storage_key, content)
        .await
        .is_err()
    {
        return internal_error();
    }
    let new_file = NewSqlFile {
        id: Uuid::new_v4(),
        owner_id,
        name: &request.name,
        storage_key,
        size_bytes,
        content_sha256: &content_sha256,
    };
    match create_sql_file(&state.pool, &new_file).await {
        Ok(file) => (StatusCode::CREATED, Json(FileResponse::from(file))).into_response(),
        Err(error) => {
            storage.remove_unreferenced(owner_id, storage_key).await;
            if is_unique_violation(&error) {
                error_response(StatusCode::CONFLICT, "file_name_conflict")
            } else {
                internal_error()
            }
        }
    }
}

async fn read(
    State(state): State<SqlFilesState>,
    headers: HeaderMap,
    file_path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Some(storage) = storage(&state).await else {
        return storage_unavailable();
    };
    let Ok(Path(file_id)) = file_path else {
        return bad_request();
    };
    let file = match find_sql_file_for_owner(&state.pool, owner_id, file_id).await {
        Ok(Some(file)) => file,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let Ok(content) = storage.read(owner_id, file.storage_key).await else {
        return internal_error();
    };
    if Sha256::digest(&content)[..] != file.content_sha256[..] {
        return internal_error();
    }
    let Ok(content) = String::from_utf8(content) else {
        return internal_error();
    };
    Json(FileContentResponse {
        file: FileResponse::from(file),
        content,
    })
    .into_response()
}

async fn update(
    State(state): State<SqlFilesState>,
    headers: HeaderMap,
    file_path: Result<Path<Uuid>, PathRejection>,
    request: Result<Json<UpdateFileRequest>, JsonRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Some(storage) = storage(&state).await else {
        return storage_unavailable();
    };
    let Ok(Path(file_id)) = file_path else {
        return bad_request();
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return json_rejection(&rejection),
    };
    let Some(expected_sha256) = parse_sha256_hex(&request.expected_sha256) else {
        return bad_request();
    };
    let Some(size_bytes) = content_size(&request.content) else {
        return file_too_large();
    };
    let content = request.content.into_bytes();
    let content_sha256 = Sha256::digest(&content);
    let storage_key = Uuid::new_v4();
    if storage
        .write_new(owner_id, storage_key, content)
        .await
        .is_err()
    {
        return internal_error();
    }
    let updated = update_sql_file_content_for_owner(
        &state.pool,
        owner_id,
        file_id,
        &expected_sha256,
        storage_key,
        size_bytes,
        &content_sha256,
    )
    .await;
    match updated {
        Ok(Some(updated)) => {
            storage
                .remove_unreferenced(owner_id, updated.previous_storage_key)
                .await;
            Json(FileResponse::from(updated.file)).into_response()
        }
        Ok(None) => {
            storage.remove_unreferenced(owner_id, storage_key).await;
            match find_sql_file_for_owner(&state.pool, owner_id, file_id).await {
                Ok(Some(_)) => error_response(StatusCode::CONFLICT, "file_conflict"),
                Ok(None) => not_found(),
                Err(_) => internal_error(),
            }
        }
        Err(_) => {
            storage.remove_unreferenced(owner_id, storage_key).await;
            internal_error()
        }
    }
}

async fn remove(
    State(state): State<SqlFilesState>,
    headers: HeaderMap,
    file_path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Some(storage) = storage(&state).await else {
        return storage_unavailable();
    };
    let Ok(Path(file_id)) = file_path else {
        return bad_request();
    };
    match delete_sql_file_for_owner(&state.pool, owner_id, file_id).await {
        Ok(Some(storage_key)) => {
            storage.remove_unreferenced(owner_id, storage_key).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found(),
        Err(_) => internal_error(),
    }
}

fn is_valid_file_name(name: &str) -> bool {
    (MIN_NAME_BYTES..=MAX_NAME_BYTES).contains(&name.len())
        && name.ends_with(".sql")
        && !name.starts_with('.')
        && name.trim() == name
        && !name
            .chars()
            .any(|character| matches!(character, '/' | '\\') || character.is_control())
}

/// Returns the content size when it is within the limit.
fn content_size(content: &str) -> Option<i32> {
    if content.len() > MAX_CONTENT_BYTES {
        return None;
    }
    i32::try_from(content.len()).ok()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_sha256_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut hash = [0_u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        let pair = value.get(index * 2..index * 2 + 2)?;
        if !pair
            .bytes()
            .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(hash)
}

fn json_rejection(rejection: &JsonRejection) -> Response {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        file_too_large()
    } else {
        bad_request()
    }
}

fn error_response(status: StatusCode, error: &'static str) -> Response {
    (status, Json(ErrorResponse { error })).into_response()
}

fn bad_request() -> Response {
    error_response(StatusCode::BAD_REQUEST, "invalid_request")
}

fn not_found() -> Response {
    error_response(StatusCode::NOT_FOUND, "not_found")
}

fn internal_error() -> Response {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

fn file_too_large() -> Response {
    error_response(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large")
}

fn storage_unavailable() -> Response {
    error_response(StatusCode::SERVICE_UNAVAILABLE, "file_storage_unavailable")
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header::AUTHORIZATION},
        response::Response,
    };
    use pgrustadmin_postgres_driver::{
        connect_metadata_pool, create_session, run_metadata_migrations,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{MAX_CONTENT_BYTES, is_valid_file_name, parse_sha256_hex, to_hex};

    #[test]
    fn file_names_must_be_plain_sql_names() {
        for valid in ["a.sql", "report 2026.sql", "relatório.sql"] {
            assert!(is_valid_file_name(valid), "{valid}");
        }
        let long_name = format!("{}.sql", "a".repeat(252));
        for invalid in [
            ".sql",
            "a.SQL",
            "a.txt",
            "../a.sql",
            "dir/a.sql",
            "dir\\a.sql",
            ".hidden.sql",
            " a.sql",
            "a.sql ",
            "a\n.sql",
            long_name.as_str(),
        ] {
            assert!(!is_valid_file_name(invalid), "{invalid:?}");
        }
        assert!(is_valid_file_name(&format!("{}.sql", "a".repeat(251))));
    }

    #[test]
    fn sha256_hex_round_trips_and_rejects_malformed_values() {
        let bytes: Vec<u8> = (0..32).collect();
        let hex = to_hex(&bytes);
        assert_eq!(hex.len(), 64);
        assert_eq!(parse_sha256_hex(&hex).map(Vec::from), Some(bytes));
        assert!(parse_sha256_hex(&hex.to_uppercase()).is_none());
        assert!(parse_sha256_hex(&hex[..62]).is_none());
        assert!(parse_sha256_hex(&format!("{}zz", &hex[..62])).is_none());
        assert!(parse_sha256_hex(&format!("{}é", &hex[..62])).is_none());
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn sql_file_endpoints_store_content_and_reject_conflicts() {
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
        for user_id in [owner_id, other_owner_id] {
            sqlx::query(
                "INSERT INTO users (id, email, password_hash, is_active) \
                 VALUES ($1, $2, 'test-only-hash', TRUE)",
            )
            .bind(user_id)
            .bind(format!("{user_id}@example.test"))
            .execute(&pool)
            .await
            .expect("insert isolated test user");
        }
        let owner_token = crate::auth::session::SessionToken::generate();
        let other_token = crate::auth::session::SessionToken::generate();
        create_session(&pool, owner_id, &owner_token.token_hash())
            .await
            .expect("create owner session");
        create_session(&pool, other_owner_id, &other_token.token_hash())
            .await
            .expect("create other owner session");
        let owner = Some(owner_token.as_str());
        let other = Some(other_token.as_str());

        let storage_root = TempDir(
            env::temp_dir().join(format!("pgrustadmin-sql-files-{}", Uuid::new_v4().simple())),
        );
        std::fs::create_dir(&storage_root.0).expect("create storage directory");
        let app = super::router(pool.clone(), Some(storage_root.0.clone()));

        let unavailable = super::router(pool.clone(), None);
        let (status, body) = send(unavailable, Method::GET, "/api/v1/sql-files", owner, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            serde_json::json!({"error": "file_storage_unavailable"})
        );
        let (status, _) = send(app.clone(), Method::GET, "/api/v1/sql-files", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, created) = send(
            app.clone(),
            Method::POST,
            "/api/v1/sql-files",
            owner,
            Some(serde_json::json!({"name": "report.sql", "content": "SELECT 'ção';\n"})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let file_id = created["id"].as_str().expect("file id").to_owned();
        let first_hash = created["sha256"].as_str().expect("file hash").to_owned();
        assert_eq!(created["name"], "report.sql");
        assert_eq!(created["size_bytes"], 16);
        assert_eq!(first_hash.len(), 64);
        let owner_dir = storage_root.0.join(owner_id.to_string());
        assert_eq!(
            stored_contents(&owner_dir),
            vec!["SELECT 'ção';\n".to_owned()]
        );

        for (payload, expected) in [
            (
                serde_json::json!({"name": "report.sql", "content": ""}),
                StatusCode::CONFLICT,
            ),
            (
                serde_json::json!({"name": "../escape.sql", "content": ""}),
                StatusCode::BAD_REQUEST,
            ),
            (
                serde_json::json!({"name": "big.sql", "content": "x".repeat(MAX_CONTENT_BYTES + 1)}),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            let (status, _) = send(
                app.clone(),
                Method::POST,
                "/api/v1/sql-files",
                owner,
                Some(payload),
            )
            .await;
            assert_eq!(status, expected);
        }
        assert_eq!(
            stored_contents(&owner_dir).len(),
            1,
            "failed creates leave no content"
        );

        let (status, other_created) = send(
            app.clone(),
            Method::POST,
            "/api/v1/sql-files",
            other,
            Some(serde_json::json!({"name": "report.sql", "content": "SELECT 2;"})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "names are unique per owner");

        let (status, listed) = send(
            app.clone(),
            Method::GET,
            "/api/v1/sql-files?per_page=100",
            owner,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"], serde_json::json!([created.clone()]));
        assert_eq!(listed["per_page"], 100);

        let file_path = format!("/api/v1/sql-files/{file_id}");
        let (status, read) = send(app.clone(), Method::GET, &file_path, owner, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["content"], "SELECT 'ção';\n");
        assert_eq!(read["sha256"], first_hash.as_str());

        let (status, updated) = send(
            app.clone(),
            Method::PUT,
            &file_path,
            owner,
            Some(serde_json::json!({"content": "SELECT 2;", "expected_sha256": first_hash})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let second_hash = updated["sha256"].as_str().expect("updated hash").to_owned();
        assert_ne!(second_hash, first_hash);
        assert_eq!(updated["size_bytes"], 9);
        assert_eq!(stored_contents(&owner_dir), vec!["SELECT 2;".to_owned()]);

        let (status, conflict) = send(
            app.clone(),
            Method::PUT,
            &file_path,
            owner,
            Some(serde_json::json!({"content": "SELECT 3;", "expected_sha256": first_hash})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(conflict, serde_json::json!({"error": "file_conflict"}));
        assert_eq!(stored_contents(&owner_dir), vec!["SELECT 2;".to_owned()]);

        let (status, _) = send(
            app.clone(),
            Method::PUT,
            &file_path,
            owner,
            Some(serde_json::json!({"content": "", "expected_sha256": "not-a-hash"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        for method in [Method::GET, Method::DELETE] {
            let (status, _) = send(app.clone(), method, &file_path, other, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
        let (status, _) = send(
            app.clone(),
            Method::PUT,
            &file_path,
            other,
            Some(serde_json::json!({"content": "", "expected_sha256": second_hash})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = send(app.clone(), Method::DELETE, &file_path, owner, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(stored_contents(&owner_dir).is_empty());
        let (status, _) = send(app.clone(), Method::GET, &file_path, owner, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let other_path = format!(
            "/api/v1/sql-files/{}",
            other_created["id"].as_str().expect("other file id")
        );
        let (status, _) = send(app, Method::DELETE, &other_path, other, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    fn stored_contents(directory: &std::path::Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        entries
            .map(|entry| {
                std::fs::read_to_string(entry.expect("storage entry").path())
                    .expect("read stored content")
            })
            .collect()
    }

    async fn send(
        app: Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
        payload: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if payload.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let body = payload.map_or_else(Body::empty, |value| Body::from(value.to_string()));
        let response: Response = app
            .oneshot(builder.body(body).expect("build test request"))
            .await
            .expect("sql file router responds");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("read response body");
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("decode JSON response")
        };
        (status, json)
    }
}
