//! Integração PostgreSQL do PGRustAdmin.

use futures_util::TryStreamExt;
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

const RELATION_ROWS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// One page of a relation's rows, requested by the data grid.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RelationRowsQuery<'a> {
    pub sort: Option<(&'a str, SortDirection)>,
    /// Case-insensitive text matched anywhere in the row; empty means none.
    pub search: &'a str,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RelationRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_rows: i64,
}

#[derive(Debug)]
pub enum RelationRowsError {
    /// Missing relation, kind outside the catalog list or schema without `USAGE`.
    NotFound,
    /// The sort column is not a column of the relation.
    InvalidSort,
    /// The saved credential cannot read the relation.
    Forbidden,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for RelationRowsError {
    fn from(error: sqlx::Error) -> Self {
        let denied = error
            .as_database_error()
            .and_then(|database_error| database_error.code())
            .is_some_and(|code| code == "42501");
        if denied {
            Self::Forbidden
        } else {
            Self::Database(error)
        }
    }
}

/// Quotes an identifier taken from the catalog for use in SQL text.
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `ILIKE` pattern matching `search` literally anywhere, escaped with `\\`.
fn contains_pattern(search: &str) -> String {
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// Reads one page of a catalog relation as text values, sorted and searched
/// in PostgreSQL. Identifiers come from the catalog and are always quoted;
/// search and paging values are bound parameters.
pub async fn list_relation_rows(
    connection: &StoredConnectionCredentials,
    database: &str,
    schema: &str,
    relation: &str,
    password: &str,
    query: RelationRowsQuery<'_>,
) -> Result<RelationRows, RelationRowsError> {
    let options = connection_options(connection, password)?.database(database);
    tokio::time::timeout(RELATION_ROWS_TIMEOUT, async {
        let mut connection = sqlx::PgConnection::connect_with(&options).await?;
        let relation_oid: Option<sqlx::postgres::types::Oid> = sqlx::query_scalar(
            "SELECT c.oid FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 \
               AND has_schema_privilege(n.oid, 'USAGE') \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition",
        )
        .bind(schema)
        .bind(relation)
        .fetch_optional(&mut connection)
        .await?;
        let Some(relation_oid) = relation_oid else {
            return Err(RelationRowsError::NotFound);
        };
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT attname::text FROM pg_attribute \
             WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped ORDER BY attnum",
        )
        .bind(relation_oid)
        .fetch_all(&mut connection)
        .await?;
        if let Some((column, _)) = query.sort
            && !columns.iter().any(|name| name == column)
        {
            return Err(RelationRowsError::InvalidSort);
        }

        let source = format!(
            "{}.{} AS t",
            quote_identifier(schema),
            quote_identifier(relation)
        );
        let filter = if query.search.is_empty() {
            ""
        } else {
            " WHERE t::text ILIKE $1 ESCAPE '\\'"
        };
        let pattern = contains_pattern(query.search);

        let count_sql = format!("SELECT count(*) FROM {source}{filter}");
        let mut count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
        if !query.search.is_empty() {
            count = count.bind(&pattern);
        }
        let total_rows = count.fetch_one(&mut connection).await?;

        let select_list = columns
            .iter()
            .map(|column| format!("{}::text", quote_identifier(column)))
            .collect::<Vec<_>>()
            .join(", ");
        let order = query
            .sort
            .map(|(column, direction)| {
                let direction = match direction {
                    SortDirection::Ascending => "ASC",
                    SortDirection::Descending => "DESC",
                };
                format!(
                    " ORDER BY {} {direction} NULLS LAST",
                    quote_identifier(column)
                )
            })
            .unwrap_or_default();
        let (limit_parameter, offset_parameter) = if query.search.is_empty() {
            ("$1", "$2")
        } else {
            ("$2", "$3")
        };
        let rows_sql = format!(
            "SELECT {select_list} FROM {source}{filter}{order} \
             LIMIT {limit_parameter} OFFSET {offset_parameter}"
        );
        // Identifiers are quoted catalog names; every value is a parameter.
        let mut rows_query = sqlx::query(sqlx::AssertSqlSafe(rows_sql));
        if !query.search.is_empty() {
            rows_query = rows_query.bind(&pattern);
        }
        let fetched = rows_query
            .bind(query.limit)
            .bind(query.offset)
            .fetch_all(&mut connection)
            .await?;
        let rows = fetched
            .iter()
            .map(|row| {
                use sqlx::Row as _;
                (0..columns.len())
                    .map(|index| row.try_get::<Option<String>, _>(index))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        connection.close().await?;
        Ok(RelationRows {
            columns,
            rows,
            total_rows,
        })
    })
    .await
    .map_err(|_| RelationRowsError::Database(sqlx::Error::PoolTimedOut))?
}

#[cfg(test)]
mod relation_rows_tests {
    use super::{contains_pattern, quote_identifier};

    #[test]
    fn identifiers_are_quoted_and_search_wildcards_escaped() {
        assert_eq!(quote_identifier("users"), "\"users\"");
        assert_eq!(
            quote_identifier("we\"ird \"\" name"),
            "\"we\"\"ird \"\"\"\" name\""
        );
        assert_eq!(contains_pattern("ana"), "%ana%");
        assert_eq!(contains_pattern("50%_off\\"), "%50\\%\\_off\\\\%");
    }
}

/// Rows kept per result; further rows are counted but not stored.
pub const MAX_RESULT_ROWS: usize = 1000;
/// UTF-8 bytes of values kept per result.
pub const MAX_RESULT_BYTES: usize = 5 * 1024 * 1024;

/// Columns and text values of one statement's rows, bounded by the limits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_rows: u64,
    pub truncated: bool,
    stored_bytes: usize,
}

impl ResultSet {
    fn push(&mut self, row: &sqlx::postgres::PgRow) -> Result<(), sqlx::Error> {
        use sqlx::{Column as _, Row as _, ValueRef as _};

        if self.total_rows == 0 {
            self.columns = row
                .columns()
                .iter()
                .map(|column| column.name().to_owned())
                .collect();
        }
        self.total_rows += 1;
        if self.truncated {
            return Ok(());
        }
        let mut values = Vec::with_capacity(row.len());
        let mut row_bytes = 0;
        for index in 0..row.len() {
            let raw = row.try_get_raw(index)?;
            if raw.is_null() {
                values.push(None);
            } else {
                // The simple query protocol returns every value in text format.
                let text = raw.as_str().map_err(sqlx::Error::Decode)?;
                row_bytes += text.len();
                values.push(Some(text.to_owned()));
            }
        }
        if self.rows.len() >= MAX_RESULT_ROWS || self.stored_bytes + row_bytes > MAX_RESULT_BYTES {
            self.truncated = true;
        } else {
            self.stored_bytes += row_bytes;
            self.rows.push(values);
        }
        Ok(())
    }
}

/// What running a user script produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptOutcome {
    pub statement_count: u64,
    /// Rows returned by the last completed statement.
    pub rows_returned: u64,
    /// Rows affected as reported for the last completed statement.
    pub rows_affected: u64,
    /// Rows of the last completed statement, within the result limits.
    pub result: ResultSet,
}

/// How a user script ended.
#[derive(Debug, PartialEq, Eq)]
pub enum ScriptEnd<T> {
    Completed(ScriptOutcome),
    /// `stop` resolved first with this value; the running statement was
    /// cancelled on a best-effort basis and the connection dropped.
    Stopped(T),
}

/// Runs arbitrary user SQL on a fresh autocommit connection to `database`
/// with the simple query protocol, keeping only the last statement's rows.
/// When `stop` resolves first, the backend is asked to cancel the running
/// statement through a second connection with the same credentials.
pub async fn execute_script<T>(
    connection: &StoredConnectionCredentials,
    database: &str,
    password: &str,
    sql: String,
    stop: impl std::future::Future<Output = T>,
) -> Result<ScriptEnd<T>, sqlx::Error> {
    let options = connection_options(connection, password)?.database(database);
    tokio::pin!(stop);
    let connect = tokio::time::timeout(
        CONNECTION_TEST_TIMEOUT,
        sqlx::PgConnection::connect_with(&options),
    );
    let mut connection = tokio::select! {
        connected = connect => connected.map_err(|_| sqlx::Error::PoolTimedOut)??,
        reason = &mut stop => return Ok(ScriptEnd::Stopped(reason)),
    };
    let backend_pid: i32 = tokio::select! {
        pid = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut connection) => pid?,
        reason = &mut stop => return Ok(ScriptEnd::Stopped(reason)),
    };
    let finished = tokio::select! {
        outcome = run_script(&mut connection, sql) => Ok(outcome),
        reason = &mut stop => Err(reason),
    };
    match finished {
        Ok(outcome) => {
            let outcome = outcome?;
            connection.close().await?;
            Ok(ScriptEnd::Completed(outcome))
        }
        Err(reason) => {
            // The protocol state of an interrupted stream is unknown, so the
            // connection is dropped instead of being closed gracefully.
            cancel_backend(&options, backend_pid).await;
            drop(connection);
            Ok(ScriptEnd::Stopped(reason))
        }
    }
}

async fn run_script(
    connection: &mut sqlx::PgConnection,
    sql: String,
) -> Result<ScriptOutcome, sqlx::Error> {
    // Arbitrary SQL is the feature: it is deliberately neither parsed nor filtered.
    let mut results = sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).fetch_many(connection);
    let mut outcome = ScriptOutcome::default();
    let mut current = ResultSet::default();
    while let Some(item) = results.try_next().await? {
        match item {
            sqlx::Either::Left(result) => {
                let finished = std::mem::take(&mut current);
                outcome = ScriptOutcome {
                    statement_count: outcome.statement_count + 1,
                    rows_returned: finished.total_rows,
                    rows_affected: result.rows_affected(),
                    result: finished,
                };
            }
            sqlx::Either::Right(row) => current.push(&row)?,
        }
    }
    Ok(outcome)
}

/// Asks PostgreSQL to cancel the statement running in `backend_pid`. Failure
/// only means the backend may finish its current statement; it is ignored.
async fn cancel_backend(options: &PgConnectOptions, backend_pid: i32) {
    let cancel = async {
        let mut connection = sqlx::PgConnection::connect_with(options).await?;
        sqlx::query("SELECT pg_cancel_backend($1)")
            .bind(backend_pid)
            .execute(&mut connection)
            .await?;
        connection.close().await
    };
    let _ = tokio::time::timeout(CONNECTION_TEST_TIMEOUT, cancel).await;
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

/// Inserts an active user; a duplicate email is a unique violation.
pub async fn create_user(
    pool: &PgPool,
    user_id: Uuid,
    email: &str,
    password_hash: &str,
) -> Result<(), PostgresError> {
    sqlx::query("INSERT INTO users (id, email, password_hash, is_active) VALUES ($1, $2, $3, TRUE)")
        .bind(user_id)
        .bind(email)
        .bind(password_hash)
        .execute(pool)
        .await
        .map(|_| ())
}
