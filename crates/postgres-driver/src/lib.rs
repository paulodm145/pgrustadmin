//! Integração PostgreSQL do PGRustAdmin.

use sqlx::{
    Connection,
    migrate::MigrateError,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};
pub use sqlx::{Error as PostgresError, PgPool};
use std::time::Duration;
use uuid::Uuid;

const CONNECTION_TEST_TIMEOUT: Duration = Duration::from_secs(5);

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

#[derive(sqlx::FromRow)]
pub struct StoredConnectionCredentials {
    pub host: String,
    pub port: i32,
    pub database_name: String,
    pub username: String,
    pub tls_enabled: bool,
    pub credential_envelope: Vec<u8>,
}

pub async fn find_connection_credentials_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    connection_id: Uuid,
) -> Result<Option<StoredConnectionCredentials>, sqlx::Error> {
    sqlx::query_as(
        "SELECT host, port, database_name, username, tls_enabled, credential_envelope \
         FROM connections WHERE owner_id = $1 AND id = $2",
    )
    .bind(owner_id)
    .bind(connection_id)
    .fetch_optional(pool)
    .await
}

pub async fn test_database_connection(
    connection: &StoredConnectionCredentials,
    password: &str,
) -> Result<(), sqlx::Error> {
    let options = connection_options(connection, password)?;
    tokio::time::timeout(CONNECTION_TEST_TIMEOUT, async {
        let connection = sqlx::PgConnection::connect_with(&options).await?;
        connection.close().await
    })
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)?
}

pub async fn list_databases(
    connection: &StoredConnectionCredentials,
    password: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let options = connection_options(connection, password)?;
    tokio::time::timeout(CONNECTION_TEST_TIMEOUT, async {
        let mut connection = sqlx::PgConnection::connect_with(&options).await?;
        let databases = sqlx::query_scalar::<_, String>(
            "SELECT datname FROM pg_database \
             WHERE datallowconn AND NOT datistemplate \
               AND has_database_privilege(datname, 'CONNECT') \
             ORDER BY datname",
        )
        .fetch_all(&mut connection)
        .await?;
        connection.close().await?;
        Ok(databases)
    })
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)?
}

/// Lists schemas of `database` visible to the saved credential, excluding
/// internal TOAST and temporary schemas.
pub async fn list_schemas(
    connection: &StoredConnectionCredentials,
    database: &str,
    password: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let options = connection_options(connection, password)?.database(database);
    tokio::time::timeout(CONNECTION_TEST_TIMEOUT, async {
        let mut connection = sqlx::PgConnection::connect_with(&options).await?;
        let schemas = sqlx::query_scalar::<_, String>(
            "SELECT nspname::text FROM pg_namespace \
             WHERE has_schema_privilege(oid, 'USAGE') \
               AND nspname NOT LIKE 'pg\\_toast%' \
               AND nspname NOT LIKE 'pg\\_temp\\_%' \
             ORDER BY nspname",
        )
        .fetch_all(&mut connection)
        .await?;
        connection.close().await?;
        Ok(schemas)
    })
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)?
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    PartitionedTable,
    View,
    MaterializedView,
    ForeignTable,
}

impl RelationKind {
    fn from_relkind(relkind: &str) -> Option<Self> {
        match relkind {
            "r" => Some(Self::Table),
            "p" => Some(Self::PartitionedTable),
            "v" => Some(Self::View),
            "m" => Some(Self::MaterializedView),
            "f" => Some(Self::ForeignTable),
            _ => None,
        }
    }
}

pub struct Relation {
    pub name: String,
    pub kind: RelationKind,
}

/// Lists one page of tables, views, materialized views and foreign tables in
/// `schema`, excluding partitions. A schema without `USAGE` yields no rows.
pub async fn list_relations(
    connection: &StoredConnectionCredentials,
    database: &str,
    schema: &str,
    password: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Relation>, sqlx::Error> {
    let options = connection_options(connection, password)?.database(database);
    tokio::time::timeout(CONNECTION_TEST_TIMEOUT, async {
        let mut connection = sqlx::PgConnection::connect_with(&options).await?;
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT c.relname::text, c.relkind::text \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 \
               AND has_schema_privilege(n.oid, 'USAGE') \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
               AND NOT c.relispartition \
             ORDER BY c.relname LIMIT $2 OFFSET $3",
        )
        .bind(schema)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut connection)
        .await?;
        connection.close().await?;
        rows.into_iter()
            .map(|(name, relkind)| {
                RelationKind::from_relkind(&relkind)
                    .map(|kind| Relation { name, kind })
                    .ok_or_else(|| sqlx::Error::Protocol("unexpected relation kind".into()))
            })
            .collect()
    })
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)?
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnIdentity {
    Always,
    ByDefault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnGenerated {
    Stored,
    Virtual,
}

pub struct Column {
    pub name: String,
    pub position: i16,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    pub identity: Option<ColumnIdentity>,
    pub generated: Option<ColumnGenerated>,
}

#[derive(sqlx::FromRow)]
struct ColumnRow {
    name: String,
    position: i16,
    data_type: String,
    nullable: bool,
    default_expression: Option<String>,
    primary_key: bool,
    identity: String,
    generated: String,
}

impl TryFrom<ColumnRow> for Column {
    type Error = sqlx::Error;

    fn try_from(row: ColumnRow) -> Result<Self, Self::Error> {
        let identity = match row.identity.as_str() {
            "" => None,
            "a" => Some(ColumnIdentity::Always),
            "d" => Some(ColumnIdentity::ByDefault),
            _ => return Err(sqlx::Error::Protocol("unexpected column identity".into())),
        };
        let generated = match row.generated.as_str() {
            "" => None,
            "s" => Some(ColumnGenerated::Stored),
            "v" => Some(ColumnGenerated::Virtual),
            _ => return Err(sqlx::Error::Protocol("unexpected generated column".into())),
        };
        Ok(Self {
            name: row.name,
            position: row.position,
            data_type: row.data_type,
            nullable: row.nullable,
            default: row.default_expression,
            primary_key: row.primary_key,
            identity,
            generated,
        })
    }
}

#[derive(sqlx::FromRow)]
pub struct Index {
    pub name: String,
    pub method: String,
    pub unique: bool,
    pub primary: bool,
    pub columns: Vec<String>,
}

/// Lists the live columns of a relation listed by [`list_relations`], in
/// position order. A missing relation or schema without `USAGE` yields no rows.
pub async fn list_columns(
    connection: &StoredConnectionCredentials,
    database: &str,
    schema: &str,
    relation: &str,
    password: &str,
) -> Result<Vec<Column>, sqlx::Error> {
    let options = connection_options(connection, password)?.database(database);
    tokio::time::timeout(CONNECTION_TEST_TIMEOUT, async {
        let mut connection = sqlx::PgConnection::connect_with(&options).await?;
        let rows = sqlx::query_as::<_, ColumnRow>(
            "SELECT a.attname::text AS name, \
                    a.attnum AS position, \
                    format_type(a.atttypid, a.atttypmod) AS data_type, \
                    NOT a.attnotnull AS nullable, \
                    CASE WHEN a.attgenerated = '' THEN pg_get_expr(d.adbin, d.adrelid) END \
                        AS default_expression, \
                    EXISTS (SELECT 1 FROM pg_index i \
                            WHERE i.indrelid = c.oid AND i.indisprimary \
                              AND a.attnum = ANY (i.indkey)) AS primary_key, \
                    a.attidentity::text AS identity, \
                    a.attgenerated::text AS generated \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE n.nspname = $1 AND c.relname = $2 \
               AND has_schema_privilege(n.oid, 'USAGE') \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
               AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
        )
        .bind(schema)
        .bind(relation)
        .fetch_all(&mut connection)
        .await?;
        connection.close().await?;
        rows.into_iter().map(Column::try_from).collect()
    })
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)?
}

/// Lists indexes of a relation listed by [`list_relations`], ordered by name,
/// with key columns or expressions in index order (no `INCLUDE` columns).
pub async fn list_indexes(
    connection: &StoredConnectionCredentials,
    database: &str,
    schema: &str,
    relation: &str,
    password: &str,
) -> Result<Vec<Index>, sqlx::Error> {
    let options = connection_options(connection, password)?.database(database);
    tokio::time::timeout(CONNECTION_TEST_TIMEOUT, async {
        let mut connection = sqlx::PgConnection::connect_with(&options).await?;
        let indexes = sqlx::query_as::<_, Index>(
            "SELECT ic.relname::text AS name, \
                    am.amname::text AS method, \
                    i.indisunique AS unique, \
                    i.indisprimary AS primary, \
                    ARRAY(SELECT pg_get_indexdef(i.indexrelid, k, true) \
                                 || CASE WHEN i.indoption[k - 1] & 1 <> 0 \
                                         THEN ' DESC' ELSE '' END \
                                 || CASE i.indoption[k - 1] & 3 \
                                         WHEN 1 THEN ' NULLS LAST' \
                                         WHEN 2 THEN ' NULLS FIRST' ELSE '' END \
                          FROM generate_series(1, i.indnkeyatts::int) AS k \
                          ORDER BY k) AS columns \
             FROM pg_index i \
             JOIN pg_class ic ON ic.oid = i.indexrelid \
             JOIN pg_am am ON am.oid = ic.relam \
             JOIN pg_class c ON c.oid = i.indrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 \
               AND has_schema_privilege(n.oid, 'USAGE') \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
             ORDER BY ic.relname",
        )
        .bind(schema)
        .bind(relation)
        .fetch_all(&mut connection)
        .await?;
        connection.close().await?;
        Ok(indexes)
    })
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)?
}

fn connection_options(
    connection: &StoredConnectionCredentials,
    password: &str,
) -> Result<PgConnectOptions, sqlx::Error> {
    let port = u16::try_from(connection.port)
        .map_err(|_| sqlx::Error::Protocol("stored database port is invalid".into()))?;
    let ssl_mode = if connection.tls_enabled {
        PgSslMode::Require
    } else {
        PgSslMode::Disable
    };
    let options = PgConnectOptions::new()
        .host(&connection.host)
        .port(port)
        .database(&connection.database_name)
        .username(&connection.username)
        .password(password)
        .ssl_mode(ssl_mode);
    Ok(options)
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

#[cfg(test)]
mod connection_options_tests {
    use super::{PgSslMode, StoredConnectionCredentials, connection_options};

    fn connection(tls_enabled: bool) -> StoredConnectionCredentials {
        StoredConnectionCredentials {
            host: "db.example.test".to_owned(),
            port: 5432,
            database_name: "app".to_owned(),
            username: "app_user".to_owned(),
            tls_enabled,
            credential_envelope: Vec::new(),
        }
    }

    #[test]
    fn requires_tls_when_enabled_and_disables_it_when_explicitly_disabled() {
        let secure = connection_options(&connection(true), "test-password")
            .expect("valid connection options");
        assert!(matches!(secure.get_ssl_mode(), PgSslMode::Require));

        let local = connection_options(&connection(false), "test-password")
            .expect("valid connection options");
        assert!(matches!(local.get_ssl_mode(), PgSslMode::Disable));
    }
}

#[derive(sqlx::FromRow)]
pub struct SqlFileRecord {
    pub id: Uuid,
    pub name: String,
    pub storage_key: Uuid,
    pub size_bytes: i32,
    pub content_sha256: Vec<u8>,
}

pub struct NewSqlFile<'a> {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: &'a str,
    pub storage_key: Uuid,
    pub size_bytes: i32,
    pub content_sha256: &'a [u8],
}

/// A successful conditional content update and the storage key it replaced.
#[derive(sqlx::FromRow)]
pub struct UpdatedSqlFile {
    #[sqlx(flatten)]
    pub file: SqlFileRecord,
    pub previous_storage_key: Uuid,
}

pub async fn list_sql_files_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<SqlFileRecord>, PostgresError> {
    sqlx::query_as(
        "SELECT id, name, storage_key, size_bytes, content_sha256 \
         FROM sql_files WHERE owner_id = $1 \
         ORDER BY name, id LIMIT $2 OFFSET $3",
    )
    .bind(owner_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn find_sql_file_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    file_id: Uuid,
) -> Result<Option<SqlFileRecord>, PostgresError> {
    sqlx::query_as(
        "SELECT id, name, storage_key, size_bytes, content_sha256 \
         FROM sql_files WHERE owner_id = $1 AND id = $2",
    )
    .bind(owner_id)
    .bind(file_id)
    .fetch_optional(pool)
    .await
}

pub async fn create_sql_file(
    pool: &PgPool,
    file: &NewSqlFile<'_>,
) -> Result<SqlFileRecord, PostgresError> {
    sqlx::query_as(
        "INSERT INTO sql_files (id, owner_id, name, storage_key, size_bytes, content_sha256) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, name, storage_key, size_bytes, content_sha256",
    )
    .bind(file.id)
    .bind(file.owner_id)
    .bind(file.name)
    .bind(file.storage_key)
    .bind(file.size_bytes)
    .bind(file.content_sha256)
    .fetch_one(pool)
    .await
}

/// Points the file at new content only while its current hash equals
/// `expected_sha256`. Returns `None` when the file is missing, owned by
/// someone else or its content changed.
pub async fn update_sql_file_content_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    file_id: Uuid,
    expected_sha256: &[u8],
    storage_key: Uuid,
    size_bytes: i32,
    content_sha256: &[u8],
) -> Result<Option<UpdatedSqlFile>, PostgresError> {
    sqlx::query_as(
        "UPDATE sql_files AS file \
         SET storage_key = $4, size_bytes = $5, content_sha256 = $6, updated_at = now() \
         FROM sql_files AS previous \
         WHERE previous.id = file.id \
           AND file.owner_id = $1 AND file.id = $2 AND file.content_sha256 = $3 \
         RETURNING file.id, file.name, file.storage_key, file.size_bytes, \
                   file.content_sha256, previous.storage_key AS previous_storage_key",
    )
    .bind(owner_id)
    .bind(file_id)
    .bind(expected_sha256)
    .bind(storage_key)
    .bind(size_bytes)
    .bind(content_sha256)
    .fetch_optional(pool)
    .await
}

/// Deletes the file record and returns the storage key of its content.
pub async fn delete_sql_file_for_owner(
    pool: &PgPool,
    owner_id: Uuid,
    file_id: Uuid,
) -> Result<Option<Uuid>, PostgresError> {
    sqlx::query_scalar(
        "DELETE FROM sql_files WHERE owner_id = $1 AND id = $2 RETURNING storage_key",
    )
    .bind(owner_id)
    .bind(file_id)
    .fetch_optional(pool)
    .await
}
