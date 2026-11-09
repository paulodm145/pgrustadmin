use std::{ffi::OsStr, io, net::SocketAddr, path::PathBuf};

use axum::Router;
pub mod api;
pub mod auth;
pub mod connections;
pub mod credentials;
pub mod executions;
pub mod sql_files;
pub mod users_cli;

use api::router as api_router;
use auth::handlers::router as auth_router;
use connections::router as connections_router;
use executions::{
    EXECUTION_TIMEOUT_ENV, MAX_CONCURRENT_EXECUTIONS_ENV, execution_timeout,
    max_concurrent_executions, router as executions_router,
};
use pgrustadmin_postgres_driver::{
    PgPool, connect_metadata_pool, pending_metadata_migrations, run_metadata_migrations,
};
use sql_files::{SQL_FILES_DIR_ENV, router as sql_files_router};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("pgrustadmin-server: {error}");
        std::process::exit(1);
    }
}

async fn run() -> io::Result<()> {
    initialize_logging()?;
    let mut arguments = std::env::args_os();
    let _program = arguments.next();

    match arguments.next() {
        None => run_server().await,
        Some(command) if command == OsStr::new("migrate") && arguments.next().is_none() => {
            migrate_metadata().await
        }
        Some(command) if command == OsStr::new("user") => users_cli::run(arguments).await,
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            users_cli::USAGE,
        )),
    }
}

async fn run_server() -> io::Result<()> {
    let bind_addr = configured_bind_addr()?;
    let execution_timeout = execution_timeout(std::env::var_os(EXECUTION_TIMEOUT_ENV))?;
    let max_executions =
        max_concurrent_executions(std::env::var_os(MAX_CONCURRENT_EXECUTIONS_ENV))?;
    let database_url = std::env::var("PGRUSTADMIN_METADATA_DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "PGRUSTADMIN_METADATA_DATABASE_URL is required to run the server",
        )
    })?;
    let metadata_pool = connect_metadata_pool(&database_url)
        .await
        .map_err(|_| io::Error::other("could not connect to metadata database"))?;
    ensure_metadata_migrated(&metadata_pool).await?;
    let listener = TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;

    let sql_files_dir = std::env::var_os(SQL_FILES_DIR_ENV).map(PathBuf::from);
    if sql_files_dir.is_none() {
        tracing::warn!("{SQL_FILES_DIR_ENV} is not set; SQL file routes respond 503");
    }

    tracing::info!(%local_addr, "server listening");
    let app = Router::new()
        .merge(api_router())
        .merge(auth_router(metadata_pool.clone()))
        .merge(connections_router(metadata_pool.clone()))
        .merge(executions_router(
            metadata_pool.clone(),
            execution_timeout,
            max_executions,
        ))
        .merge(sql_files_router(metadata_pool, sql_files_dir));
    axum::serve(listener, app).await
}

async fn migrate_metadata() -> io::Result<()> {
    let database_url = std::env::var("PGRUSTADMIN_METADATA_DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "PGRUSTADMIN_METADATA_DATABASE_URL is required for migrations",
        )
    })?;

    let pool = connect_metadata_pool(&database_url)
        .await
        .map_err(|_| io::Error::other("could not connect to metadata database"))?;
    run_metadata_migrations(&pool)
        .await
        .map_err(|_| io::Error::other("could not apply metadata migrations"))?;
    pool.close().await;

    tracing::info!("metadata migrations applied");
    Ok(())
}

/// Refuses to serve a metadata database that lacks migrations this build
/// expects; queries would otherwise fail later with unclear errors.
async fn ensure_metadata_migrated(pool: &PgPool) -> io::Result<()> {
    let pending = pending_metadata_migrations(pool)
        .await
        .map_err(|_| io::Error::other("could not read the metadata migration state"))?;
    if pending.is_empty() {
        return Ok(());
    }
    let names = pending
        .iter()
        .map(|(version, description)| format!("{version:04} {description}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(io::Error::other(format!(
        "the metadata database has {} pending migration(s) ({names}); \
         run `pgrustadmin-server migrate` and start the server again",
        pending.len()
    )))
}

fn configured_bind_addr() -> io::Result<SocketAddr> {
    let Some(value) = std::env::var_os("PGRUSTADMIN_BIND_ADDR") else {
        return Ok(SocketAddr::from(([127, 0, 0, 1], 3000)));
    };

    let value = value.into_string().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "PGRUSTADMIN_BIND_ADDR must contain a valid socket address",
        )
    })?;

    let bind_addr = value.parse::<SocketAddr>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "PGRUSTADMIN_BIND_ADDR must contain a valid socket address",
        )
    })?;

    if !bind_addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote bind is disabled until TLS and authentication are configured",
        ));
    }

    Ok(bind_addr)
}

fn initialize_logging() -> io::Result<()> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => EnvFilter::try_new(value).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "RUST_LOG contains an invalid filter",
            )
        })?,
        Err(std::env::VarError::NotPresent) => EnvFilter::new("info"),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RUST_LOG must contain a valid filter",
            ));
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .map_err(|_| io::Error::other("could not initialize logging"))
}

#[cfg(test)]
mod tests {
    use std::env;

    use pgrustadmin_postgres_driver::{connect_metadata_pool, run_metadata_migrations};
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use uuid::Uuid;

    use super::ensure_metadata_migrated;

    #[tokio::test]
    async fn the_server_refuses_a_metadata_database_with_pending_migrations() {
        let database_url = env::var("PGRUSTADMIN_TEST_DATABASE_URL")
            .expect("PGRUSTADMIN_TEST_DATABASE_URL is required for database tests");
        let pool = connect_metadata_pool(&database_url)
            .await
            .expect("connect to the test metadata database");
        run_metadata_migrations(&pool)
            .await
            .expect("apply metadata migrations");
        ensure_metadata_migrated(&pool)
            .await
            .expect("a migrated database is accepted");

        // An empty schema first in the search path hides the migration table.
        let schema = format!("unmigrated_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&pool)
            .await
            .expect("create empty schema");
        let options: PgConnectOptions = database_url.parse().expect("parse test URL");
        let empty = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.options([("search_path", schema.as_str())]))
            .await
            .expect("connect with the empty schema");
        let error = ensure_metadata_migrated(&empty)
            .await
            .expect_err("pending migrations stop the server");
        let message = error.to_string();
        assert!(
            message.contains("pending migration(s) (0001 create users"),
            "{message}"
        );
        assert!(
            message.contains("0006 add connection read only"),
            "{message}"
        );
        assert!(message.contains("pgrustadmin-server migrate"), "{message}");
        empty.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema}")))
            .execute(&pool)
            .await
            .expect("drop empty schema");
    }
}
