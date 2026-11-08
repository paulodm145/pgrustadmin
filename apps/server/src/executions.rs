//! Asynchronous execution of user SQL, tracked in server memory.

use std::{
    collections::HashMap,
    ffi::OsString,
    io,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
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
use pgrustadmin_postgres_driver::{PgPool, PostgresError, ResultSet, ScriptEnd, execute_script};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::connections::{
    authenticated_owner, authentication_error_response, is_valid_identifier, load_saved_connection,
};

const MAX_SQL_BYTES: usize = 1_048_576;
/// JSON escaping can expand SQL well beyond its raw size.
const MAX_REQUEST_BYTES: usize = 8 * MAX_SQL_BYTES;
const MAX_RUNNING_PER_OWNER: usize = 5;
const MAX_RETAINED_PER_OWNER: usize = 50;
const MAX_RESULTS_WITH_ROWS_PER_OWNER: usize = 10;
const CONNECTION_FAILED_MESSAGE: &str = "Could not connect to the database or read its response.";
const STOPPED_MESSAGE: &str = "The execution stopped unexpectedly.";
pub const EXECUTION_TIMEOUT_ENV: &str = "PGRUSTADMIN_EXECUTION_TIMEOUT_SECS";
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 300;
const MAX_EXECUTION_TIMEOUT_SECS: u64 = 3600;
pub const MAX_CONCURRENT_EXECUTIONS_ENV: &str = "PGRUSTADMIN_MAX_CONCURRENT_EXECUTIONS";
const DEFAULT_MAX_CONCURRENT_EXECUTIONS: u64 = 20;
const MAX_MAX_CONCURRENT_EXECUTIONS: u64 = 1000;

/// Parses the execution timeout from its environment value, if set.
pub fn execution_timeout(value: Option<OsString>) -> io::Result<Duration> {
    bounded_setting(
        value,
        DEFAULT_EXECUTION_TIMEOUT_SECS,
        MAX_EXECUTION_TIMEOUT_SECS,
        || {
            format!(
                "{EXECUTION_TIMEOUT_ENV} must be a whole number of seconds from 1 to \
                 {MAX_EXECUTION_TIMEOUT_SECS}"
            )
        },
    )
    .map(Duration::from_secs)
}

/// Parses the server-wide limit of running executions, if set.
pub fn max_concurrent_executions(value: Option<OsString>) -> io::Result<usize> {
    bounded_setting(
        value,
        DEFAULT_MAX_CONCURRENT_EXECUTIONS,
        MAX_MAX_CONCURRENT_EXECUTIONS,
        || {
            format!(
                "{MAX_CONCURRENT_EXECUTIONS_ENV} must be a whole number from 1 to \
                 {MAX_MAX_CONCURRENT_EXECUTIONS}"
            )
        },
    )
    .and_then(|limit| usize::try_from(limit).map_err(io::Error::other))
}

/// A whole number from 1 to `max`, or `default` when the variable is unset.
fn bounded_setting(
    value: Option<OsString>,
    default: u64,
    max: u64,
    message: impl FnOnce() -> String,
) -> io::Result<u64> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .into_string()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|number| (1..=max).contains(number))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, message()))
}

/// Why an execution stopped before the script finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopReason {
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    statement_count: u64,
    rows_returned: u64,
    rows_affected: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum ExecutionState {
    Running,
    Succeeded {
        counts: Counts,
        /// Rows of the last statement; `None` once retention discarded them.
        rows: Option<ResultSet>,
    },
    Failed {
        code: Option<String>,
        message: String,
    },
    Cancelled,
    TimedOut,
}

struct ExecutionEntry {
    owner_id: Uuid,
    connection_id: Uuid,
    database: String,
    /// Start order, used to evict the oldest finished executions.
    sequence: u64,
    /// Finish order, used to keep rows only for the most recent results.
    finish_sequence: Option<u64>,
    duration_ms: Option<u64>,
    state: ExecutionState,
    /// Signals the running task to cancel; taken by the first cancel request.
    cancel: Option<oneshot::Sender<()>>,
}

/// Status of an execution without its rows, cheap to copy for polling.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusView {
    connection_id: Uuid,
    database: String,
    duration_ms: Option<u64>,
    state: ViewState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ViewState {
    Running,
    Succeeded(Counts),
    Failed {
        code: Option<String>,
        message: String,
    },
    Cancelled,
    TimedOut,
}

/// Which limit of running executions refused a new one.
#[derive(Debug, PartialEq, Eq)]
enum LimitReached {
    Owner,
    Server,
}

#[derive(Debug, PartialEq, Eq)]
enum CancelError {
    NotFound,
    NotRunning,
}

#[derive(Debug, PartialEq, Eq)]
enum ResultError {
    NotFound,
    Unavailable,
    Expired,
}

/// Executions shared between request handlers and the tasks running SQL.
///
/// A `Mutex` is the simplest fit: every critical section is a short map
/// update that never awaits, and the alternatives (a dedicated actor task or
/// a concurrent-map crate) add code or a dependency without a concrete gain.
struct ExecutionRegistry {
    inner: Mutex<RegistryState>,
    /// Running executions allowed across all users.
    max_running: usize,
}

impl Default for ExecutionRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT_EXECUTIONS as usize)
    }
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<Uuid, ExecutionEntry>,
    next_sequence: u64,
}

impl RegistryState {
    /// Drops stored rows of all but the most recent finished results of `owner_id`.
    fn expire_old_rows(&mut self, owner_id: Uuid) {
        let mut with_rows: Vec<(u64, &mut ExecutionEntry)> = self
            .entries
            .values_mut()
            .filter(|entry| {
                entry.owner_id == owner_id
                    && matches!(entry.state, ExecutionState::Succeeded { rows: Some(_), .. })
            })
            .filter_map(|entry| entry.finish_sequence.map(|sequence| (sequence, entry)))
            .collect();
        with_rows.sort_by_key(|(sequence, _)| std::cmp::Reverse(*sequence));
        for (_, entry) in with_rows.into_iter().skip(MAX_RESULTS_WITH_ROWS_PER_OWNER) {
            if let ExecutionState::Succeeded { rows, .. } = &mut entry.state {
                *rows = None;
            }
        }
    }
}

impl ExecutionRegistry {
    fn new(max_running: usize) -> Self {
        Self {
            inner: Mutex::default(),
            max_running,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        // Entries stay consistent even if a holder panicked: each update is a
        // single insert/remove/assignment.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(
        &self,
        owner_id: Uuid,
        connection_id: Uuid,
        database: &str,
        cancel: oneshot::Sender<()>,
    ) -> Result<Uuid, LimitReached> {
        let mut state = self.lock();
        let running = state
            .entries
            .values()
            .filter(|entry| entry.state == ExecutionState::Running)
            .count();
        let owned: Vec<(Uuid, u64, bool)> = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.owner_id == owner_id)
            .map(|(id, entry)| (*id, entry.sequence, entry.state == ExecutionState::Running))
            .collect();
        if owned.iter().filter(|(_, _, running)| *running).count() >= MAX_RUNNING_PER_OWNER {
            return Err(LimitReached::Owner);
        }
        if running >= self.max_running {
            return Err(LimitReached::Server);
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
                finish_sequence: None,
                duration_ms: None,
                state: ExecutionState::Running,
                cancel: Some(cancel),
            },
        );
        Ok(id)
    }

    fn finish(&self, id: Uuid, result: ExecutionState, duration_ms: u64) {
        let mut state = self.lock();
        let finish_sequence = state.next_sequence;
        state.next_sequence += 1;
        let Some(entry) = state.entries.get_mut(&id) else {
            return;
        };
        entry.state = result;
        entry.cancel = None;
        entry.duration_ms = Some(duration_ms);
        entry.finish_sequence = Some(finish_sequence);
        let owner_id = entry.owner_id;
        state.expire_old_rows(owner_id);
    }

    fn status(&self, owner_id: Uuid, id: Uuid) -> Option<StatusView> {
        let state = self.lock();
        let entry = state
            .entries
            .get(&id)
            .filter(|entry| entry.owner_id == owner_id)?;
        let view = match &entry.state {
            ExecutionState::Running => ViewState::Running,
            ExecutionState::Succeeded { counts, .. } => ViewState::Succeeded(*counts),
            ExecutionState::Failed { code, message } => ViewState::Failed {
                code: code.clone(),
                message: message.clone(),
            },
            ExecutionState::Cancelled => ViewState::Cancelled,
            ExecutionState::TimedOut => ViewState::TimedOut,
        };
        Some(StatusView {
            connection_id: entry.connection_id,
            database: entry.database.clone(),
            duration_ms: entry.duration_ms,
            state: view,
        })
    }

    fn result(&self, owner_id: Uuid, id: Uuid) -> Result<ResultSet, ResultError> {
        let state = self.lock();
        let entry = state
            .entries
            .get(&id)
            .filter(|entry| entry.owner_id == owner_id)
            .ok_or(ResultError::NotFound)?;
        match &entry.state {
            ExecutionState::Succeeded {
                rows: Some(rows), ..
            } => Ok(rows.clone()),
            ExecutionState::Succeeded { rows: None, .. } => Err(ResultError::Expired),
            ExecutionState::Running
            | ExecutionState::Failed { .. }
            | ExecutionState::Cancelled
            | ExecutionState::TimedOut => Err(ResultError::Unavailable),
        }
    }

    /// Signals a running execution to stop. Repeated requests while it is
    /// still stopping succeed without effect.
    fn cancel(&self, owner_id: Uuid, id: Uuid) -> Result<(), CancelError> {
        let mut state = self.lock();
        let entry = state
            .entries
            .get_mut(&id)
            .filter(|entry| entry.owner_id == owner_id)
            .ok_or(CancelError::NotFound)?;
        if entry.state != ExecutionState::Running {
            return Err(CancelError::NotRunning);
        }
        if let Some(cancel) = entry.cancel.take() {
            // The receiver is gone only if the task already ended; its guard
            // then records the final state.
            let _ = cancel.send(());
        }
        Ok(())
    }
}

/// Records a failure if the execution task ends without reporting, for
/// example when it panics or is aborted, so it never stays `running`.
struct FinishGuard {
    registry: Arc<ExecutionRegistry>,
    id: Uuid,
    started: Instant,
    finished: bool,
}

impl FinishGuard {
    fn new(registry: Arc<ExecutionRegistry>, id: Uuid) -> Self {
        Self {
            registry,
            id,
            started: Instant::now(),
            finished: false,
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn finish(mut self, result: ExecutionState) {
        self.finished = true;
        self.registry.finish(self.id, result, self.elapsed_ms());
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.finish(
                self.id,
                ExecutionState::Failed {
                    code: None,
                    message: STOPPED_MESSAGE.to_owned(),
                },
                self.elapsed_ms(),
            );
        }
    }
}

#[derive(Clone)]
struct ExecutionsState {
    pool: PgPool,
    registry: Arc<ExecutionRegistry>,
    timeout: Duration,
}

pub fn router(pool: PgPool, timeout: Duration, max_running: usize) -> Router {
    Router::new()
        .route(
            "/api/v1/connections/{id}/databases/{database}/executions",
            post(start),
        )
        .route("/api/v1/executions/{id}", get(status))
        .route("/api/v1/executions/{id}/result", get(result))
        .route("/api/v1/executions/{id}/cancel", post(cancel))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(ExecutionsState {
            pool,
            registry: Arc::new(ExecutionRegistry::new(max_running)),
            timeout,
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
    duration_ms: Option<u64>,
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
struct ResultRowsResponse {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    stored_rows: usize,
    total_rows: u64,
    truncated: bool,
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
    let (cancel_sender, cancel_receiver) = oneshot::channel();
    let id = match state
        .registry
        .start(owner_id, connection_id, &database, cancel_sender)
    {
        Ok(id) => id,
        Err(LimitReached::Owner) => {
            return error_response(StatusCode::TOO_MANY_REQUESTS, "execution_limit_reached");
        }
        Err(LimitReached::Server) => {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "server_execution_limit_reached",
            );
        }
    };

    let guard = FinishGuard::new(Arc::clone(&state.registry), id);
    let timeout = state.timeout;
    tokio::spawn(async move {
        let stop = async {
            tokio::select! {
                () = tokio::time::sleep(timeout) => StopReason::TimedOut,
                requested = cancel_receiver => match requested {
                    Ok(()) => StopReason::Cancelled,
                    // The sender only disappears once the execution finished.
                    Err(_) => std::future::pending().await,
                },
            }
        };
        let result = execute_script(&connection, &database, &password, sql, stop).await;
        guard.finish(execution_state(result));
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
    let Some(view) = state.registry.status(owner_id, id) else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let (status, result, error) = match view.state {
        ViewState::Running => ("running", None, None),
        ViewState::Succeeded(counts) => (
            "succeeded",
            Some(ResultResponse {
                statement_count: counts.statement_count,
                rows_returned: counts.rows_returned,
                rows_affected: counts.rows_affected,
            }),
            None,
        ),
        ViewState::Failed { code, message } => (
            "failed",
            None,
            Some(ExecutionErrorResponse { code, message }),
        ),
        ViewState::Cancelled => ("cancelled", None, None),
        ViewState::TimedOut => ("timed_out", None, None),
    };
    Json(ExecutionResponse {
        id,
        connection_id: view.connection_id,
        database: view.database,
        status,
        duration_ms: view.duration_ms,
        result,
        error,
    })
    .into_response()
}

async fn result(
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
    match state.registry.result(owner_id, id) {
        Ok(rows) => Json(ResultRowsResponse {
            stored_rows: rows.rows.len(),
            columns: rows.columns,
            rows: rows.rows,
            total_rows: rows.total_rows,
            truncated: rows.truncated,
        })
        .into_response(),
        Err(ResultError::NotFound) => error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(ResultError::Unavailable) => error_response(StatusCode::CONFLICT, "result_unavailable"),
        Err(ResultError::Expired) => error_response(StatusCode::GONE, "result_expired"),
    }
}

async fn cancel(
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
    match state.registry.cancel(owner_id, id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(CancelError::NotFound) => error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(CancelError::NotRunning) => {
            error_response(StatusCode::CONFLICT, "execution_not_running")
        }
    }
}

/// Keeps PostgreSQL's SQLSTATE and primary message for errors in the user's
/// SQL; any other failure gets a generic message without driver text.
fn execution_state(result: Result<ScriptEnd<StopReason>, PostgresError>) -> ExecutionState {
    match result {
        Ok(ScriptEnd::Stopped(StopReason::Cancelled)) => ExecutionState::Cancelled,
        Ok(ScriptEnd::Stopped(StopReason::TimedOut)) => ExecutionState::TimedOut,
        Ok(ScriptEnd::Completed(outcome)) => ExecutionState::Succeeded {
            counts: Counts {
                statement_count: outcome.statement_count,
                rows_returned: outcome.rows_returned,
                rows_affected: outcome.rows_affected,
            },
            rows: Some(outcome.result),
        },
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
        PostgresError, ResultSet, connect_metadata_pool, create_session, run_metadata_migrations,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{
        CONNECTION_FAILED_MESSAGE, CancelError, Counts, ExecutionRegistry, ExecutionState,
        FinishGuard, LimitReached, MAX_RESULTS_WITH_ROWS_PER_OWNER, MAX_RETAINED_PER_OWNER,
        MAX_RUNNING_PER_OWNER, ResultError, STOPPED_MESSAGE, ViewState, execution_state,
        execution_timeout, max_concurrent_executions,
    };

    fn channel() -> tokio::sync::oneshot::Sender<()> {
        tokio::sync::oneshot::channel().0
    }

    fn succeeded() -> ExecutionState {
        ExecutionState::Succeeded {
            counts: Counts::default(),
            rows: Some(ResultSet::default()),
        }
    }

    #[test]
    fn registry_limits_running_executions_per_owner() {
        let registry = ExecutionRegistry::default();
        let owner = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..MAX_RUNNING_PER_OWNER)
            .map(|_| {
                registry
                    .start(owner, Uuid::new_v4(), "app", channel())
                    .expect("start")
            })
            .collect();
        assert_eq!(
            registry.start(owner, Uuid::new_v4(), "app", channel()),
            Err(LimitReached::Owner)
        );
        assert!(
            registry
                .start(Uuid::new_v4(), Uuid::new_v4(), "app", channel())
                .is_ok()
        );

        registry.finish(ids[0], succeeded(), 7);
        assert!(
            registry
                .start(owner, Uuid::new_v4(), "app", channel())
                .is_ok()
        );
        assert!(
            registry.status(Uuid::new_v4(), ids[0]).is_none(),
            "owner scoped"
        );
        let view = registry.status(owner, ids[0]).expect("status");
        assert_eq!(view.state, ViewState::Succeeded(Counts::default()));
        assert_eq!(view.duration_ms, Some(7));
        assert_eq!(
            registry.status(owner, ids[1]).expect("status").duration_ms,
            None
        );
    }

    #[test]
    fn registry_evicts_oldest_finished_executions_beyond_retention() {
        let registry = ExecutionRegistry::default();
        let owner = Uuid::new_v4();
        let mut ids = Vec::new();
        for _ in 0..MAX_RETAINED_PER_OWNER {
            let id = registry
                .start(owner, Uuid::new_v4(), "app", channel())
                .expect("start");
            registry.finish(id, succeeded(), 1);
            ids.push(id);
        }
        let running = registry
            .start(owner, Uuid::new_v4(), "app", channel())
            .expect("start");
        assert!(
            registry.status(owner, ids[0]).is_none(),
            "oldest finished evicted"
        );
        assert!(registry.status(owner, ids[1]).is_some());
        assert!(registry.status(owner, running).is_some());
        registry
            .start(owner, Uuid::new_v4(), "app", channel())
            .expect("start");
        assert!(registry.status(owner, ids[1]).is_none());
        assert!(
            registry.status(owner, running).is_some(),
            "running never evicted"
        );
    }

    #[test]
    fn registry_keeps_rows_only_for_recent_results() {
        let registry = ExecutionRegistry::default();
        let owner = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..=MAX_RESULTS_WITH_ROWS_PER_OWNER)
            .map(|_| {
                let id = registry
                    .start(owner, Uuid::new_v4(), "app", channel())
                    .expect("start");
                registry.finish(id, succeeded(), 1);
                id
            })
            .collect();
        assert_eq!(registry.result(owner, ids[0]), Err(ResultError::Expired));
        assert!(registry.status(owner, ids[0]).is_some(), "status survives");
        assert_eq!(registry.result(owner, ids[1]), Ok(ResultSet::default()));
        assert_eq!(
            registry.result(Uuid::new_v4(), ids[1]),
            Err(ResultError::NotFound)
        );

        let running = registry
            .start(owner, Uuid::new_v4(), "app", channel())
            .expect("start");
        assert_eq!(
            registry.result(owner, running),
            Err(ResultError::Unavailable)
        );
        registry.finish(
            running,
            ExecutionState::Failed {
                code: None,
                message: "x".to_owned(),
            },
            1,
        );
        assert_eq!(
            registry.result(owner, running),
            Err(ResultError::Unavailable)
        );
    }

    #[test]
    fn dropped_execution_task_is_recorded_as_failed() {
        let registry = std::sync::Arc::new(ExecutionRegistry::default());
        let owner = Uuid::new_v4();
        let id = registry
            .start(owner, Uuid::new_v4(), "app", channel())
            .expect("start");
        drop(FinishGuard::new(std::sync::Arc::clone(&registry), id));
        let view = registry.status(owner, id).expect("status");
        assert_eq!(
            view.state,
            ViewState::Failed {
                code: None,
                message: STOPPED_MESSAGE.to_owned(),
            }
        );
        assert!(view.duration_ms.is_some());

        let finished = registry
            .start(owner, Uuid::new_v4(), "app", channel())
            .expect("start");
        FinishGuard::new(std::sync::Arc::clone(&registry), finished).finish(succeeded());
        assert_eq!(
            registry.status(owner, finished).expect("status").state,
            ViewState::Succeeded(Counts::default())
        );
    }

    #[test]
    fn execution_timeout_defaults_and_rejects_out_of_range_values() {
        assert_eq!(
            execution_timeout(None).expect("default"),
            Duration::from_secs(300)
        );
        assert_eq!(
            execution_timeout(Some("3600".into())).expect("maximum"),
            Duration::from_secs(3600)
        );
        assert_eq!(
            execution_timeout(Some("1".into())).expect("minimum"),
            Duration::from_secs(1)
        );
        for invalid in ["0", "3601", "-1", "5m", ""] {
            assert!(
                execution_timeout(Some(invalid.into())).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn max_concurrent_executions_defaults_and_rejects_out_of_range_values() {
        assert_eq!(max_concurrent_executions(None).expect("default"), 20);
        assert_eq!(
            max_concurrent_executions(Some("1000".into())).expect("maximum"),
            1000
        );
        assert_eq!(
            max_concurrent_executions(Some("1".into())).expect("minimum"),
            1
        );
        for invalid in ["0", "1001", "-1", "ten", ""] {
            assert!(
                max_concurrent_executions(Some(invalid.into())).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn registry_limits_running_executions_across_owners() {
        let registry = ExecutionRegistry::new(3);
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let ids = [
            registry.start(first, Uuid::new_v4(), "app", channel()),
            registry.start(first, Uuid::new_v4(), "app", channel()),
            registry.start(second, Uuid::new_v4(), "app", channel()),
        ]
        .map(|started| started.expect("start below the server limit"));
        let third = Uuid::new_v4();
        assert_eq!(
            registry.start(third, Uuid::new_v4(), "app", channel()),
            Err(LimitReached::Server)
        );

        registry.finish(ids[0], succeeded(), 7);
        assert!(
            registry
                .start(third, Uuid::new_v4(), "app", channel())
                .is_ok(),
            "finished executions free a server slot"
        );
        assert_eq!(
            registry.start(second, Uuid::new_v4(), "app", channel()),
            Err(LimitReached::Server)
        );
    }

    #[tokio::test]
    async fn cancelling_signals_only_running_executions_of_the_owner() {
        let registry = ExecutionRegistry::default();
        let owner = Uuid::new_v4();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let id = registry
            .start(owner, Uuid::new_v4(), "app", sender)
            .expect("start");
        assert_eq!(
            registry.cancel(Uuid::new_v4(), id),
            Err(CancelError::NotFound)
        );
        assert_eq!(registry.cancel(owner, id), Ok(()));
        assert_eq!(
            registry.cancel(owner, id),
            Ok(()),
            "repeated while stopping"
        );
        assert_eq!(receiver.await, Ok(()));
        registry.finish(id, ExecutionState::Cancelled, 5);
        assert_eq!(registry.cancel(owner, id), Err(CancelError::NotRunning));
        assert_eq!(
            registry.status(owner, id).expect("status").state,
            ViewState::Cancelled
        );
        assert!(
            registry
                .start(owner, Uuid::new_v4(), "app", channel())
                .is_ok(),
            "cancelled executions free their slot"
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
        let app = super::router(pool.clone(), Duration::from_secs(300), 20);

        let (status, started) = send(
            app.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({
                "sql": "CREATE TEMP TABLE t (x int, label text);\n\
                        INSERT INTO t VALUES (1, 'a'), (2, NULL), (3, 'ção');\n\
                        SELECT x, label FROM t WHERE x > 1 ORDER BY x;"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(started["status"], "running");
        let execution_id = started["id"].as_str().expect("execution id").to_owned();
        let mut finished = wait_for(&app, &execution_id, owner).await;
        assert!(finished["duration_ms"].is_u64(), "{finished}");
        finished
            .as_object_mut()
            .expect("status object")
            .remove("duration_ms");
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

        let result_path = format!("/api/v1/executions/{execution_id}/result");
        let (status, rows) = send(app.clone(), Method::GET, &result_path, owner, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            rows,
            serde_json::json!({
                "columns": ["x", "label"],
                "rows": [["2", null], ["3", "ção"]],
                "stored_rows": 2,
                "total_rows": 2,
                "truncated": false,
            })
        );

        for path in [
            format!("/api/v1/executions/{execution_id}"),
            result_path.clone(),
        ] {
            let (status, _) = send(
                app.clone(),
                Method::GET,
                &path,
                Some(other_token.as_str()),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }

        for (sql, stored, total) in [
            ("SELECT g FROM generate_series(1, 1001) AS g", 1000, 1001),
            (
                "SELECT repeat('x', 1048576) FROM generate_series(1, 6)",
                5,
                6,
            ),
            ("SELECT 1; CREATE TEMP TABLE e (x int)", 0, 0),
        ] {
            let (_, started) = send(
                app.clone(),
                Method::POST,
                &start_path,
                owner,
                Some(serde_json::json!({ "sql": sql })),
            )
            .await;
            let id = started["id"].as_str().expect("id").to_owned();
            assert_eq!(wait_for(&app, &id, owner).await["status"], "succeeded");
            let (status, rows) = send(
                app.clone(),
                Method::GET,
                &format!("/api/v1/executions/{id}/result"),
                owner,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(rows["stored_rows"], stored, "{sql}");
            assert_eq!(rows["total_rows"], total, "{sql}");
            assert_eq!(rows["truncated"], stored != total, "{sql}");
            assert_eq!(rows["rows"].as_array().map(Vec::len), Some(stored), "{sql}");
        }

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
        assert!(failed["duration_ms"].is_u64());
        let (status, unavailable) = send(
            app.clone(),
            Method::GET,
            &format!(
                "/api/v1/executions/{}/result",
                started["id"].as_str().expect("id")
            ),
            owner,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            unavailable,
            serde_json::json!({"error": "result_unavailable"})
        );

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

        let single = super::router(pool.clone(), Duration::from_secs(300), 1);
        let (status, busy) = send(
            single.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({"sql": "SELECT pg_sleep(1)"})),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (status, limited) = send(
            single.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({"sql": "SELECT 1"})),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited,
            serde_json::json!({"error": "server_execution_limit_reached"})
        );
        let busy = busy["id"].as_str().expect("id");
        assert_eq!(wait_for(&single, busy, owner).await["status"], "succeeded");

        let marker = Uuid::new_v4().simple().to_string();
        let sleep_sql = format!("SELECT pg_sleep(30) /* cancel {marker} */");
        let (_, started) = send(
            app.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({ "sql": sleep_sql })),
        )
        .await;
        let id = started["id"].as_str().expect("id").to_owned();
        wait_for_backend(&pool, &marker, 1).await;
        let cancel_path = format!("/api/v1/executions/{id}/cancel");
        let (status, _) = send(
            app.clone(),
            Method::POST,
            &cancel_path,
            Some(other_token.as_str()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(app.clone(), Method::POST, &cancel_path, owner, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let cancelled = wait_for(&app, &id, owner).await;
        assert_eq!(cancelled["status"], "cancelled");
        assert!(cancelled["duration_ms"].as_u64().expect("duration") < 30_000);
        assert!(cancelled.get("result").is_none() && cancelled.get("error").is_none());
        wait_for_backend(&pool, &marker, 0).await;
        let (status, body) = send(app.clone(), Method::POST, &cancel_path, owner, None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, serde_json::json!({"error": "execution_not_running"}));
        let (status, _) = send(
            app.clone(),
            Method::GET,
            &format!("/api/v1/executions/{id}/result"),
            owner,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let short = super::router(pool.clone(), Duration::from_secs(1), 20);
        let marker = Uuid::new_v4().simple().to_string();
        let (_, started) = send(
            short.clone(),
            Method::POST,
            &start_path,
            owner,
            Some(serde_json::json!({
                "sql": format!("SELECT 1; SELECT pg_sleep(30) /* timeout {marker} */")
            })),
        )
        .await;
        let timed_out = wait_for(&short, started["id"].as_str().expect("id"), owner).await;
        assert_eq!(timed_out["status"], "timed_out");
        let duration = timed_out["duration_ms"].as_u64().expect("duration");
        assert!((1_000..30_000).contains(&duration), "{duration}");
        wait_for_backend(&pool, &marker, 0).await;
    }

    /// Waits until `expected` backends are running the query tagged `marker`.
    async fn wait_for_backend(pool: &sqlx::PgPool, marker: &str, expected: i64) {
        for _ in 0..100 {
            let running: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE state = 'active' AND query LIKE '%' || $1 || '%' \
                   AND pid <> pg_backend_pid()",
            )
            .bind(marker)
            .fetch_one(pool)
            .await
            .expect("query pg_stat_activity");
            if running == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("expected {expected} backend(s) running {marker}");
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
        let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
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
