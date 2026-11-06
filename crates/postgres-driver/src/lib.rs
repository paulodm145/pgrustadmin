//! Integração PostgreSQL do PGRustAdmin.

pub use sqlx::{Error as PostgresError, PgPool};
use sqlx::{migrate::MigrateError, postgres::PgPoolOptions};
use uuid::Uuid;

static METADATA_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(sqlx::FromRow)]
pub struct ConnectionSummary {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: i32,
    pub database_name: String,
    pub username: String,
    pub tls_enabled: bool,
}

pub struct ConnectionData<'a> {
    pub name: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub database_name: &'a str,
    pub username: &'a str,
    pub tls_enabled: bool,
}

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

pub async fn find_user_for_login(
    pool: &PgPool,
    normalized_email: &str,
) -> Result<Option<(Uuid, String, bool)>, sqlx::Error> {
    sqlx::query_as("SELECT id, password_hash, is_active FROM users WHERE email = $1")
        .bind(normalized_email)
        .fetch_optional(pool)
        .await
}

pub async fn list_connections_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<ConnectionSummary>, PostgresError> {
    sqlx::query_as(
        "SELECT id, name, host, port, database_name, username, tls_enabled \
         FROM connections WHERE owner_id = $1 \
         ORDER BY name, id LIMIT $2 OFFSET $3",
    )
    .bind(owner_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn find_connection_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    connection_id: Uuid,
) -> Result<Option<ConnectionSummary>, PostgresError> {
    sqlx::query_as(
        "SELECT id, name, host, port, database_name, username, tls_enabled \
         FROM connections WHERE owner_id = $1 AND id = $2",
    )
    .bind(owner_id)
    .bind(connection_id)
    .fetch_optional(pool)
    .await
}

pub async fn create_connection(
    pool: &PgPool,
    owner_id: Uuid,
    connection_id: Uuid,
    connection: &ConnectionData<'_>,
    credential_envelope: &[u8],
) -> Result<ConnectionSummary, PostgresError> {
    sqlx::query_as(
        "INSERT INTO connections \
         (id, owner_id, name, host, port, database_name, username, tls_enabled, credential_envelope) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         RETURNING id, name, host, port, database_name, username, tls_enabled",
    )
    .bind(connection_id)
    .bind(owner_id)
    .bind(connection.name)
    .bind(connection.host)
    .bind(i32::from(connection.port))
    .bind(connection.database_name)
    .bind(connection.username)
    .bind(connection.tls_enabled)
    .bind(credential_envelope)
    .fetch_one(pool)
    .await
}

pub async fn update_connection_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    connection_id: Uuid,
    connection: &ConnectionData<'_>,
    credential_envelope: Option<&[u8]>,
) -> Result<Option<ConnectionSummary>, PostgresError> {
    sqlx::query_as(
        "UPDATE connections SET name = $3, host = $4, port = $5, \
         database_name = $6, username = $7, tls_enabled = $8, \
         credential_envelope = COALESCE($9, credential_envelope), updated_at = now() \
         WHERE owner_id = $1 AND id = $2 \
         RETURNING id, name, host, port, database_name, username, tls_enabled",
    )
    .bind(owner_id)
    .bind(connection_id)
    .bind(connection.name)
    .bind(connection.host)
    .bind(i32::from(connection.port))
    .bind(connection.database_name)
    .bind(connection.username)
    .bind(connection.tls_enabled)
    .bind(credential_envelope)
    .fetch_optional(pool)
    .await
}

pub async fn delete_connection_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    connection_id: Uuid,
) -> Result<bool, PostgresError> {
    let result = sqlx::query("DELETE FROM connections WHERE owner_id = $1 AND id = $2")
        .bind(owner_id)
        .bind(connection_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}
