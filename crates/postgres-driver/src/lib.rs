//! Integração PostgreSQL do PGRustAdmin.

use sqlx::{PgPool, migrate::MigrateError, postgres::PgPoolOptions};
use uuid::Uuid;

static METADATA_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect_metadata_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().connect(database_url).await
}

pub async fn run_metadata_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    METADATA_MIGRATOR.run(pool).await
}

pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    token_hash: &[u8; 32],
) -> Result<Uuid, sqlx::Error> {
    let session_id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO sessions (id, token_hash, user_id, expires_at) \
         VALUES ($1, $2, $3, now() + interval '12 hours')",
    )
    .bind(session_id)
    .bind(token_hash.as_slice())
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(session_id)
}

pub async fn session_user_id(
    pool: &PgPool,
    token_hash: &[u8; 32],
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT sessions.user_id FROM sessions \
         JOIN users ON users.id = sessions.user_id \
         WHERE sessions.token_hash = $1 \
           AND sessions.revoked_at IS NULL \
           AND sessions.expires_at > now() \
           AND users.is_active",
    )
    .bind(token_hash.as_slice())
    .fetch_optional(pool)
    .await
}

pub async fn revoke_session(pool: &PgPool, token_hash: &[u8; 32]) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = now() \
         WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token_hash.as_slice())
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}
