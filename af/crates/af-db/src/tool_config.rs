use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct ToolConfigRow {
    pub tool_name: String,
    pub enabled: bool,
    pub policy_override: serde_json::Value,
}

pub async fn get(pool: &PgPool, tool_name: &str) -> Result<Option<ToolConfigRow>, sqlx::Error> {
    sqlx::query_as::<_, ToolConfigRow>(
        "SELECT tool_name, enabled, policy_override FROM tool_config WHERE tool_name = $1",
    )
    .bind(tool_name)
    .fetch_optional(pool)
    .await
}

pub async fn set_enabled(pool: &PgPool, tool_name: &str, enabled: bool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tool_config (tool_name, enabled) VALUES ($1, $2) \
         ON CONFLICT (tool_name) DO UPDATE SET enabled = EXCLUDED.enabled",
    )
    .bind(tool_name)
    .bind(enabled)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn is_enabled(pool: &PgPool, tool_name: &str) -> Result<bool, sqlx::Error> {
    match get(pool, tool_name).await? {
        Some(row) => Ok(row.enabled),
        None => Ok(true), // default: enabled
    }
}
