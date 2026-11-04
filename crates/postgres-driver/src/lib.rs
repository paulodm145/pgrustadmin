//! Integração PostgreSQL do PGRustAdmin.

use sqlx::{PgPool, migrate::MigrateError, postgres::PgPoolOptions};

static METADATA_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect_metadata_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().connect(database_url).await
}

pub async fn run_metadata_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    METADATA_MIGRATOR.run(pool).await
}
