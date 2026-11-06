use std::{ffi::OsStr, io, net::SocketAddr};

pub mod auth;
pub mod credentials;

use auth::handlers::router as auth_router;
use pgrustadmin_postgres_driver::{connect_metadata_pool, run_metadata_migrations};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("server failed to start or stopped unexpectedly: {error}");
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
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: pgrustadmin-server [migrate]",
        )),
    }
}

async fn run_server() -> io::Result<()> {
    let bind_addr = configured_bind_addr()?;
    let database_url = std::env::var("PGRUSTADMIN_METADATA_DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "PGRUSTADMIN_METADATA_DATABASE_URL is required to run the server",
        )
    })?;
    let metadata_pool = connect_metadata_pool(&database_url)
        .await
        .map_err(|_| io::Error::other("could not connect to metadata database"))?;
    let listener = TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;

    tracing::info!(%local_addr, "server listening");
    axum::serve(listener, auth_router(metadata_pool)).await
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
