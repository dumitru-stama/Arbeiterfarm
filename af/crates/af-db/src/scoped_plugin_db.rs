use async_trait::async_trait;
use af_core::{Migration, PluginDb, PluginDbError};
use serde_json::Value;
use sqlx::postgres::{PgConnection, PgRow};
use sqlx::{Column, Executor, PgPool, Row};

/// Concrete PluginDb backed by Postgres with schema-scoped search_path.
///
/// Security model: each plugin gets its own Postgres schema. Queries run with
/// `SET search_path TO <plugin_schema>,pg_temp` to prevent cross-schema access.
/// SQL statements are validated against a forbidden-pattern blocklist before
/// execution.
///
/// When `user_id` is passed to query/execute methods, queries run inside a
/// scoped transaction that sets `ROLE af_api` and `af.current_user_id`
/// for RLS policy evaluation. This ensures plugin tables with RLS (e.g.
/// re.iocs) enforce tenant isolation without shared mutable state.
pub struct ScopedPluginDb {
    pool: PgPool,
    schema: String,
}

/// Validate a plugin schema name: must be non-empty, lowercase alphanumeric + underscores.
fn validate_schema_name(schema: &str) -> Result<(), String> {
    if schema.is_empty() {
        return Err("schema name must not be empty".into());
    }
    if !schema
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "invalid schema name: {schema:?} — must be [a-z0-9_]+"
        ));
    }
    Ok(())
}

impl ScopedPluginDb {
    pub fn new(pool: PgPool, schema: &str) -> Self {
        validate_schema_name(schema).expect("invalid schema name");
        Self {
            pool,
            schema: schema.to_string(),
        }
    }
}

/// Forbidden SQL patterns that could escape the plugin's schema sandbox.
const FORBIDDEN_SQL_PATTERNS: &[&str] = &[
    "SET SEARCH_PATH",
    "SET SESSION",
    "SET LOCAL",
    "GRANT",
    "REVOKE",
    "DROP SCHEMA",
    "CREATE SCHEMA",
    "ALTER SCHEMA",
    "PUBLIC.",
    "PG_CATALOG.",
    "INFORMATION_SCHEMA.",
    "SET ROLE",
    "RESET ROLE",
    "COPY ",
    "\\COPY",
];

/// Validate SQL statement against forbidden patterns.
fn validate_sql(sql: &str) -> Result<(), PluginDbError> {
    let upper = sql.to_uppercase();
    for forbidden in FORBIDDEN_SQL_PATTERNS {
        if upper.contains(forbidden) {
            return Err(PluginDbError::Query(format!(
                "forbidden SQL pattern: {forbidden}"
            )));
        }
    }
    Ok(())
}

/// Convert a single column value from a PgRow to a serde_json::Value.
/// Handles: String, i64, i32, i16, f32, f64, bool, Uuid, chrono::DateTime<Utc>,
/// chrono::NaiveDate, serde_json::Value (JSONB/JSON), Vec<String> (TEXT[]),
/// Vec<u8> (BYTEA → base64).
fn row_column_to_json(row: &PgRow, name: &str) -> Value {
    // Text types
    if let Ok(v) = row.try_get::<String, _>(name) {
        return Value::String(v);
    }
    // Integer types
    if let Ok(v) = row.try_get::<i64, _>(name) {
        return Value::Number(v.into());
    }
    if let Ok(v) = row.try_get::<i32, _>(name) {
        return Value::Number(v.into());
    }
    if let Ok(v) = row.try_get::<i16, _>(name) {
        return Value::Number(v.into());
    }
    // Float types
    if let Ok(v) = row.try_get::<f64, _>(name) {
        if let Some(n) = serde_json::Number::from_f64(v) {
            return Value::Number(n);
        }
        return Value::Null; // NaN/Inf
    }
    if let Ok(v) = row.try_get::<f32, _>(name) {
        if let Some(n) = serde_json::Number::from_f64(v as f64) {
            return Value::Number(n);
        }
        return Value::Null;
    }
    // Boolean
    if let Ok(v) = row.try_get::<bool, _>(name) {
        return Value::Bool(v);
    }
    // UUID
    if let Ok(v) = row.try_get::<uuid::Uuid, _>(name) {
        return Value::String(v.to_string());
    }
    // TIMESTAMPTZ → ISO 8601
    if let Ok(v) = row.try_get::<chrono::DateTime<chrono::Utc>, _>(name) {
        return Value::String(v.to_rfc3339());
    }
    // DATE
    if let Ok(v) = row.try_get::<chrono::NaiveDate, _>(name) {
        return Value::String(v.to_string());
    }
    // JSONB / JSON → nested Value
    if let Ok(v) = row.try_get::<Value, _>(name) {
        return v;
    }
    // TEXT[] → JSON array of strings
    if let Ok(v) = row.try_get::<Vec<String>, _>(name) {
        return Value::Array(v.into_iter().map(Value::String).collect());
    }
    // BYTEA → base64-encoded string
    if let Ok(v) = row.try_get::<Vec<u8>, _>(name) {
        use base64::Engine;
        return Value::String(base64::engine::general_purpose::STANDARD.encode(&v));
    }
    Value::Null
}

/// Bind JSON params to a sqlx query and fetch all rows.
async fn scoped_query(
    conn: &mut PgConnection,
    schema: &str,
    sql: &str,
    params: Vec<Value>,
    user_id: Option<uuid::Uuid>,
) -> Result<Vec<Value>, PluginDbError> {
    // Set RLS context if user_id is provided
    if let Some(uid) = user_id {
        conn.execute("SET LOCAL ROLE af_api")
            .await
            .map_err(|e| PluginDbError::Query(e.to_string()))?;
        sqlx::query("SELECT set_config('af.current_user_id', $1, true)")
            .bind(uid.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| PluginDbError::Query(e.to_string()))?;
    }

    // Set search_path to plugin schema
    let set_path = format!("SET search_path TO {schema},pg_temp");
    conn.execute(set_path.as_str())
        .await
        .map_err(|e| PluginDbError::Query(e.to_string()))?;

    // Build and execute the query with JSON params
    let mut query = sqlx::query(sql);
    for param in &params {
        match param {
            Value::String(s) => query = query.bind(s.clone()),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    query = query.bind(i);
                } else if let Some(f) = n.as_f64() {
                    query = query.bind(f);
                }
            }
            Value::Bool(b) => query = query.bind(*b),
            Value::Null => query = query.bind(Option::<String>::None),
            other => query = query.bind(other.to_string()),
        }
    }

    let rows_result: Result<Vec<PgRow>, _> = query
        .fetch_all(&mut *conn)
        .await;

    // Always reset search_path and role, even on query error
    conn.execute("RESET search_path")
        .await
        .map_err(|e| PluginDbError::Query(e.to_string()))?;
    if user_id.is_some() {
        let _ = conn.execute("RESET ROLE").await;
    }

    let rows = rows_result.map_err(|e| PluginDbError::Query(e.to_string()))?;

    // Convert rows to JSON values with comprehensive type mapping
    let mut results = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut obj = serde_json::Map::new();
        for col in row.columns() {
            let name = col.name();
            let val: Value = row_column_to_json(row, name);
            obj.insert(name.to_string(), val);
        }
        results.push(Value::Object(obj));
    }

    Ok(results)
}

/// Bind JSON params to a sqlx query and execute (return rows_affected).
async fn scoped_execute(
    conn: &mut PgConnection,
    schema: &str,
    sql: &str,
    params: Vec<Value>,
    user_id: Option<uuid::Uuid>,
) -> Result<u64, PluginDbError> {
    // Set RLS context if user_id is provided
    if let Some(uid) = user_id {
        conn.execute("SET LOCAL ROLE af_api")
            .await
            .map_err(|e| PluginDbError::Query(e.to_string()))?;
        sqlx::query("SELECT set_config('af.current_user_id', $1, true)")
            .bind(uid.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| PluginDbError::Query(e.to_string()))?;
    }

    let set_path = format!("SET search_path TO {schema},pg_temp");
    conn.execute(set_path.as_str())
        .await
        .map_err(|e| PluginDbError::Query(e.to_string()))?;

    let mut query = sqlx::query(sql);
    for param in &params {
        match param {
            Value::String(s) => query = query.bind(s.clone()),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    query = query.bind(i);
                } else if let Some(f) = n.as_f64() {
                    query = query.bind(f);
                }
            }
            Value::Bool(b) => query = query.bind(*b),
            Value::Null => query = query.bind(Option::<String>::None),
            other => query = query.bind(other.to_string()),
        }
    }

    let exec_result = query
        .execute(&mut *conn)
        .await;

    // Always reset search_path and role, even on execute error
    conn.execute("RESET search_path")
        .await
        .map_err(|e| PluginDbError::Query(e.to_string()))?;
    if user_id.is_some() {
        let _ = conn.execute("RESET ROLE").await;
    }

    let result = exec_result.map_err(|e| PluginDbError::Query(e.to_string()))?;

    Ok(result.rows_affected())
}

/// Run plugin migrations on a connection.
async fn run_migrations(
    conn: &mut PgConnection,
    schema: &str,
    migrations: &[Migration],
) -> Result<(), PluginDbError> {
    // Create the plugin schema if it doesn't exist
    let create_schema = format!("CREATE SCHEMA IF NOT EXISTS {schema}");
    conn.execute(create_schema.as_str())
        .await
        .map_err(|e| PluginDbError::Migration(e.to_string()))?;

    for migration in migrations {
        // Check if this migration was already applied
        let already_applied: bool =
            sqlx::query("SELECT EXISTS(SELECT 1 FROM plugin_migrations WHERE plugin = $1 AND version = $2)")
                .bind(schema)
                .bind(migration.version as i32)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| PluginDbError::Migration(e.to_string()))
                .and_then(|row| {
                    row.try_get::<bool, _>(0)
                        .map_err(|e| PluginDbError::Migration(e.to_string()))
                })?;

        if already_applied {
            continue;
        }

        // Set search_path to include both plugin schema and public (for references)
        let set_path = format!("SET search_path TO {schema},public,pg_temp");
        conn.execute(set_path.as_str())
            .await
            .map_err(|e| PluginDbError::Migration(e.to_string()))?;

        conn.execute(migration.sql.as_str())
            .await
            .map_err(|e| PluginDbError::Migration(e.to_string()))?;

        conn.execute("RESET search_path")
            .await
            .map_err(|e| PluginDbError::Migration(e.to_string()))?;

        // Record migration
        sqlx::query("INSERT INTO plugin_migrations (plugin, version) VALUES ($1, $2)")
            .bind(schema)
            .bind(migration.version as i32)
            .execute(&mut *conn)
            .await
            .map_err(|e| PluginDbError::Migration(e.to_string()))?;
    }

    Ok(())
}

#[async_trait]
impl PluginDb for ScopedPluginDb {
    async fn query_json(
        &self,
        sql: &str,
        params: Vec<Value>,
        user_id: Option<uuid::Uuid>,
    ) -> Result<Vec<Value>, PluginDbError> {
        validate_sql(sql)?;
        if user_id.is_some() {
            // SET LOCAL ROLE requires an explicit transaction to persist across statements
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| PluginDbError::Query(e.to_string()))?;
            let result = scoped_query(&mut *tx, &self.schema, sql, params, user_id).await?;
            tx.commit()
                .await
                .map_err(|e| PluginDbError::Query(e.to_string()))?;
            Ok(result)
        } else {
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|e| PluginDbError::Query(e.to_string()))?;
            scoped_query(&mut conn, &self.schema, sql, params, user_id).await
        }
    }

    async fn execute_json(
        &self,
        sql: &str,
        params: Vec<Value>,
        user_id: Option<uuid::Uuid>,
    ) -> Result<u64, PluginDbError> {
        validate_sql(sql)?;
        if user_id.is_some() {
            // SET LOCAL ROLE requires an explicit transaction to persist across statements
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| PluginDbError::Query(e.to_string()))?;
            let result = scoped_execute(&mut *tx, &self.schema, sql, params, user_id).await?;
            tx.commit()
                .await
                .map_err(|e| PluginDbError::Query(e.to_string()))?;
            Ok(result)
        } else {
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|e| PluginDbError::Query(e.to_string()))?;
            scoped_execute(&mut conn, &self.schema, sql, params, user_id).await
        }
    }

    async fn migrate(&self, migrations: &[Migration]) -> Result<(), PluginDbError> {
        // Use a transaction so that a failure mid-migration rolls back all changes,
        // keeping the schema and plugin_migrations table consistent.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PluginDbError::Migration(e.to_string()))?;
        run_migrations(&mut *tx, &self.schema, migrations).await?;
        tx.commit()
            .await
            .map_err(|e| PluginDbError::Migration(e.to_string()))?;
        Ok(())
    }

    fn schema(&self) -> &str {
        &self.schema
    }

    async fn audit_log(
        &self,
        event_type: &str,
        actor_user_id: Option<uuid::Uuid>,
        detail: Option<&serde_json::Value>,
    ) -> Result<(), PluginDbError> {
        let prefixed = format!("{}:{}", self.schema, event_type);
        sqlx::query(
            "INSERT INTO public.audit_log (event_type, actor_user_id, detail) \
             VALUES ($1, $2, $3)",
        )
        .bind(&prefixed)
        .bind(actor_user_id)
        .bind(detail)
        .execute(&self.pool)
        .await
        .map_err(|e| PluginDbError::Query(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_sql_allows_normal_queries() {
        assert!(validate_sql("SELECT * FROM iocs WHERE type = $1").is_ok());
        assert!(validate_sql("INSERT INTO iocs (type, value) VALUES ($1, $2)").is_ok());
        assert!(validate_sql("UPDATE iocs SET value = $1 WHERE id = $2").is_ok());
        assert!(validate_sql("DELETE FROM iocs WHERE id = $1").is_ok());
        assert!(validate_sql("CREATE TABLE IF NOT EXISTS iocs (id SERIAL PRIMARY KEY)").is_ok());
    }

    #[test]
    fn test_validate_sql_blocks_search_path() {
        assert!(validate_sql("SET SEARCH_PATH TO public").is_err());
        assert!(validate_sql("set search_path to public").is_err());
        assert!(validate_sql("SET SESSION authorization 'admin'").is_err());
        assert!(validate_sql("SET LOCAL search_path TO public").is_err());
    }

    #[test]
    fn test_validate_sql_blocks_schema_escape() {
        assert!(validate_sql("SELECT * FROM public.users").is_err());
        assert!(validate_sql("SELECT * FROM pg_catalog.pg_roles").is_err());
        assert!(validate_sql("SELECT * FROM information_schema.tables").is_err());
    }

    #[test]
    fn test_validate_sql_blocks_privilege_escalation() {
        assert!(validate_sql("GRANT ALL ON SCHEMA public TO af").is_err());
        assert!(validate_sql("REVOKE SELECT ON users FROM af").is_err());
        assert!(validate_sql("SET ROLE postgres").is_err());
        assert!(validate_sql("RESET ROLE").is_err());
    }

    #[test]
    fn test_validate_sql_blocks_schema_ddl() {
        assert!(validate_sql("DROP SCHEMA re CASCADE").is_err());
        assert!(validate_sql("CREATE SCHEMA evil").is_err());
        assert!(validate_sql("ALTER SCHEMA re RENAME TO evil").is_err());
    }

    #[test]
    fn test_validate_sql_blocks_copy() {
        assert!(validate_sql("COPY users TO '/tmp/dump.csv'").is_err());
        assert!(validate_sql("\\COPY users TO '/tmp/dump.csv'").is_err());
    }

    #[test]
    fn test_schema_name_valid() {
        assert!(validate_schema_name("re").is_ok());
        assert!(validate_schema_name("my_plugin_123").is_ok());
        assert!(validate_schema_name("a").is_ok());
    }

    #[test]
    fn test_schema_name_rejects_uppercase() {
        assert!(validate_schema_name("RE").is_err());
        assert!(validate_schema_name("mixedCase").is_err());
    }

    #[test]
    fn test_schema_name_rejects_special_chars() {
        assert!(validate_schema_name("public.evil").is_err());
        assert!(validate_schema_name("schema;drop").is_err());
        assert!(validate_schema_name("with space").is_err());
    }

    #[test]
    fn test_schema_name_rejects_empty() {
        assert!(validate_schema_name("").is_err());
    }
}
