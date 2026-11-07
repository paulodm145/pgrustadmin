//! Asynchronous execution of user SQL, tracked in server memory.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pgrustadmin_postgres_driver::{PgPool, PostgresError, ScriptOutcome, execute_script};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::connections::{
    authenticated_owner, authentication_error_response, is_valid_identifier, load_saved_connection,
};

const MAX_SQL_BYTES: usize = 1_048_576;
/// JSON escaping can expand SQL well beyond its raw size.
const MAX_REQUEST_BYTES: usize = 8 * MAX_SQL_BYTES;
const MAX_RUNNING_PER_OWNER: usize = 5;
const MAX_RETAINED_PER_OWNER: usize = 50;
const CONNECTION_FAILED_MESSAGE: &str = "Could not connect to the database or read its response.";

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExecutionState {
    Running,
    Succeeded(ScriptOutcome),
    Failed {
        code: Option<String>,
        message: String,
    },
}

#[derive(Clone)]
struct ExecutionEntry {
    owner_id: Uuid,
    connection_id: Uuid,
    database: String,
    /// Start order, used to evict the oldest finished executions.
    sequence: u64,
    state: ExecutionState,
}

#[derive(Debug, PartialEq, Eq)]
struct RunningLimitReached;

/// Executions shared between request handlers and the tasks running SQL.
///
/// A `Mutex` is the simplest fit: every critical section is a short map
/// update that never awaits, and the alternatives (a dedicated actor task or
/// a concurrent-map crate) add code or a dependency without a concrete gain.
#[derive(Default)]
struct ExecutionRegistry {
    inner: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<Uuid, ExecutionEntry>,
    next_sequence: u64,
}

impl ExecutionRegistry {
    fn start(
        &self,
        owner_id: Uuid,
        connection_id: Uuid,
        database: &str,
    ) -> Result<Uuid, RunningLimitReached> {
        // Entries stay consistent even if a holder panicked: each update is a
        // single insert/remove/assignment.
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let owned: Vec<(Uuid, u64, bool)> = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.owner_id == owner_id)
            .map(|(id, entry)| (*id, entry.sequence, entry.state == ExecutionState::Running))
            .collect();
        if owned.iter().filter(|(_, _, running)| *running).count() >= MAX_RUNNING_PER_OWNER {
            return Err(RunningLimitReached);
        }
        let mut finished: Vec<(Uuid, u64)> = owned
            .iter()
            .filter(|(_, _, running)| !*running)
            .map(|(id, sequence, _)| (*id, *sequence))
            .collect();
        finished.sort_by_key(|(_, sequence)| *sequence);
        let excess = (owned.len() + 1).saturating_sub(MAX_RETAINED_PER_OWNER);
        for (id, _) in finished.into_iter().take(excess) {
            state.entries.remove(&id);
        }

        let id = Uuid::new_v4();
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        state.entries.insert(
            id,
            ExecutionEntry {
                owner_id,
                connection_id,
                database: database.to_owned(),
                sequence,
                state: ExecutionState::Running,
            },
        );
        Ok(id)
    }

    fn finish(&self, id: Uuid, result: ExecutionState) {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = state.entries.get_mut(&id) {
            entry.state = result;
        }
    }

    fn get(&self, owner_id: Uuid, id: Uuid) -> Option<ExecutionEntry> {
        let state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .entries
            .get(&id)
            .filter(|entry| entry.owner_id == owner_id)
            .cloned()
    }
}

#[derive(Clone)]
struct ExecutionsState {
    pool: PgPool,
    registry: Arc<ExecutionRegistry>,
}

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route(
            "/api/v1/connections/{id}/databases/{database}/executions",
            post(start),
        )
        .route("/api/v1/executions/{id}", get(status))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(ExecutionsState {
            pool,
            registry: Arc::default(),
        })
}

#[derive(Deserialize)]
struct ExecutionRequest {
    sql: String,
}

#[derive(Serialize)]
struct StartedResponse {
    id: Uuid,
    status: &'static str,
}

#[derive(Serialize)]
struct ExecutionResponse {
    id: Uuid,
    connection_id: Uuid,
    database: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ResultResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ExecutionErrorResponse>,
}

#[derive(Serialize)]
struct ResultResponse {
    statement_count: u64,
    rows_returned: u64,
    rows_affected: u64,
}

#[derive(Serialize)]
struct ExecutionErrorResponse {
    code: Option<String>,
    message: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

async fn start(
    State(state): State<ExecutionsState>,
    headers: HeaderMap,
    path: Result<Path<(Uuid, String)>, PathRejection>,
    request: Result<Json<ExecutionRequest>, JsonRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path((connection_id, database))) = path else {
        return bad_request();
    };
    let Ok(Json(ExecutionRequest { sql })) = request else {
        return bad_request();
    };
    if !is_valid_identifier(&database) || sql.trim().is_empty() || sql.len() > MAX_SQL_BYTES {
        return bad_request();
    }
    let (connection, password) =
        match load_saved_connection(&state.pool, owner_id, connection_id).await {
            Ok(saved) => saved,
            Err(error) => return error.into_response(),
        };
    let Ok(id) = state.registry.start(owner_id, connection_id, &database) else {
        return error_response(StatusCode::TOO_MANY_REQUESTS, "execution_limit_reached");
    };

    let registry = Arc::clone(&state.registry);
    tokio::spawn(async move {
        let result = execute_script(&connection, &database, &password, sql).await;
        registry.finish(id, execution_state(result));
    });
    (
        StatusCode::ACCEPTED,
        Json(StartedResponse {
            id,
            status: "running",
        }),
    )
        .into_response()
}

async fn status(
    State(state): State<ExecutionsState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Response {
    let owner_id = match authenticated_owner(&state.pool, &headers).await {
        Ok(owner_id) => owner_id,
        Err(error) => return authentication_error_response(error),
    };
    let Ok(Path(id)) = path else {
        return bad_request();
    };
    let Some(entry) = state.registry.get(owner_id, id) else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let (status, result, error) = match entry.state {
        ExecutionState::Running => ("running", None, None),
        ExecutionState::Succeeded(outcome) => (
            "succeeded",
            Some(ResultResponse {
                statement_count: outcome.statement_count,
                rows_returned: outcome.rows_returned,
                rows_affected: outcome.rows_affected,
            }),
            None,
        ),
        ExecutionState::Failed { code, message } => (
            "failed",
            None,
            Some(ExecutionErrorResponse { code, message }),
        ),
    };
    Json(ExecutionResponse {
        id,
        connection_id: entry.connection_id,
        database: entry.database,
        status,
        result,
        error,
    })
    .into_response()
}

/// Keeps PostgreSQL's SQLSTATE and primary message for errors in the user's
/// SQL; any other failure gets a generic message without driver text.
fn execution_state(result: Result<ScriptOutcome, PostgresError>) -> ExecutionState {
    match result {
        Ok(outcome) => ExecutionState::Succeeded(outcome),
        Err(error) => match error.as_database_error() {
            Some(database_error) => ExecutionState::Failed {
                code: database_error.code().map(|code| code.into_owned()),
                message: database_error.message().to_owned(),
            },
            None => ExecutionState::Failed {
                code: None,
                message: CONNECTION_FAILED_MESSAGE.to_owned(),
            },
        },
    }
}

fn error_response(status: StatusCode, error: &'static str) -> Response {
    (status, Json(ErrorResponse { error })).into_response()
}

fn bad_request() -> Response {
    error_response(StatusCode::BAD_REQUEST, "invalid_request")
}

#[cfg(test)]
mod tests {
    use std::{env, time::Duration};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header::AUTHORIZATION},
    };
    use pgrustadmin_postgres_driver::{
        PostgresError, ScriptOutcome, connect_metadata_pool, create_session,
        run_metadata_migrations,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{
        CONNECTION_FAILED_MESSAGE, ExecutionRegistry, ExecutionState, MAX_RETAINED_PER_OWNER,
        MAX_RUNNING_PER_OWNER, RunningLimitReached, execution_state,
    };

    #[test]
    fn registry_limits_running_executions_per_owner() {
        let registry = ExecutionRegistry::default();
        let owner = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..MAX_RUNNING_PER_OWNER)
            .map(|_| registry.start(owner, Uuid::new_v4(), "app").expect("start"))
            .collect();
        assert_eq!(
            registry.start(owner, Uuid::new_v4(), "app"),
            Err(RunningLimitReached)
        );
        assert!(
            registry
                .start(Uuid::new_v4(), Uuid::new_v4(), "app")
                .is_ok()
        );

        registry.finish(ids[0], ExecutionState::Succeeded(ScriptOutcome::default()));
        assert!(registry.start(owner, Uuid::new_v4(), "app").is_ok());
        assert!(
            registry.get(Uuid::new_v4(), ids[0]).is_none(),
            "owner scoped"
        );
        assert_eq!(
            registry.get(owner, ids[0]).map(|entry| entry.state),
            Some(ExecutionState::Succeeded(ScriptOutcome::default()))
        );
    }

    #[test]
    fn registry_evicts_oldest_finished_executions_beyond_retention() {
        let registry = ExecutionRegistry::default();
        let owner = Uuid::new_v4();
        let mut ids = Vec::new();
        for _ in 0..MAX_RETAINED_PER_OWNER {
            let id = registry.start(owner, Uuid::new_v4(), "app").expect("start");
            registry.finish(id, ExecutionState::Succeeded(ScriptOutcome::default()));
            ids.push(id);
        }
        let running = registry.start(owner, Uuid::new_v4(), "app").expect("start");
        assert!(
            registry.get(owner, ids[0]).is_none(),
            "oldest finished evicted"
        );
        assert!(registry.get(owner, ids[1]).is_some());
        assert!(registry.get(owner, running).is_some());
        registry.start(owner, Uuid::new_v4(), "app").expect("start");
        assert!(registry.get(owner, ids[1]).is_none());
        assert!(
            registry.get(owner, running).is_some(),
            "running never evicted"
        );
    }

    #[test]
    fn non_database_errors_get_a_generic_message() {
        assert_eq!(
            execution_state(Err(PostgresError::PoolTimedOut)),
            ExecutionState::Failed {
                code: None,
                message: CONNECTION_FAILED_MESSAGE.to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn executions_run_user_sql_and_report_outcomes() {
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

        let connections = crate::connections::router(pool.clone());
        let test_port = env::var("PGRUSTADMIN_TEST_CONNECTION_PORT")
            .expect("test connection port must be configured")
            .parse::<u16>()
            .expect("test connection port must be valid");
        let (status, created) = send(
            connections,
            Method::POST,
            "/api/v1/connections",
            owner,
            Some(serde_json::json!({
                "name": "execution test",
                "host": "127.0.0.1",
                "port": test_port,
                "database": "pgrustadmin_test",
                "username": "pgrustadmin",
                "tls_enabled": false,
                "password": env::var("PGRUSTADMIN_TEST_CONNECTION_PASSWORD")
                    .expect("test connection password must be configured"),
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let connection_id = created["id"].as_str().expect("connection id").to_owned();
        let start_path =
            format!("/api/v1/connections/{connection_id}/databases/pgrustadmin_test/executions");
        let app = super::router(pool.clone());

        let (status, started) = send(
            app.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({
                "sql": "CREATE TEMP TABLE t (x int);\n\
                        INSERT INTO t VALUES (1), (2), (3);\n\
                        SELECT x FROM t WHERE x > 1;"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(started["status"], "running");
        let execution_id = started["id"].as_str().expect("execution id").to_owned();
        let finished = wait_for(&app, &execution_id, owner).await;
        assert_eq!(
            finished,
            serde_json::json!({
                "id": execution_id,
                "connection_id": connection_id,
                "database": "pgrustadmin_test",
                "status": "succeeded",
                "result": {"statement_count": 3, "rows_returned": 2, "rows_affected": 2},
            })
        );

        let (status, _) = send(
            app.clone(),
            Method::GET,
            &format!("/api/v1/executions/{execution_id}"),
            Some(other_token.as_str()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (_, started) = send(
            app.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({"sql": "SELECT 1; SELEC 2; SELECT 3;"})),
        )
        .await;
        let failed = wait_for(&app, started["id"].as_str().expect("id"), owner).await;
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["error"]["code"], "42601");
        assert!(
            failed["error"]["message"]
                .as_str()
                .expect("error message")
                .contains("SELEC")
        );
        assert!(failed.get("result").is_none());

        for payload in [
            serde_json::json!({"sql": "  \n "}),
            serde_json::json!({"sql": "x".repeat(super::MAX_SQL_BYTES + 1)}),
            serde_json::json!({"query": "SELECT 1"}),
        ] {
            let (status, _) =
                send(app.clone(), Method::POST, &start_path, owner, Some(payload)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (status, _) = send(
            app.clone(),
            Method::POST,
            &start_path,
            Some(other_token.as_str()),
            Some(serde_json::json!({"sql": "SELECT 1"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "foreign connection");

        let mut sleeping = Vec::new();
        for _ in 0..MAX_RUNNING_PER_OWNER {
            let (status, started) = send(
                app.clone(),
                Method::POST,
                &start_path,
                owner,
                Some(serde_json::json!({"sql": "SELECT pg_sleep(1)"})),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED);
            sleeping.push(started["id"].as_str().expect("id").to_owned());
        }
        let (status, limited) = send(
            app.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({"sql": "SELECT 1"})),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited,
            serde_json::json!({"error": "execution_limit_reached"})
        );
        for id in &sleeping {
            assert_eq!(wait_for(&app, id, owner).await["status"], "succeeded");
        }
    }

    async fn wait_for(app: &Router, id: &str, token: Option<&str>) -> serde_json::Value {
        for _ in 0..100 {
            let (status, body) = send(
                app.clone(),
                Method::GET,
                &format!("/api/v1/executions/{id}"),
                token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            if body["status"] != "running" {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("execution {id} did not finish");
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
        let response = app
            .oneshot(builder.body(body).expect("build test request"))
            .await
            .expect("execution router responds");
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
