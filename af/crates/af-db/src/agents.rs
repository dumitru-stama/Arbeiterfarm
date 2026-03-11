use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AgentRow {
    pub name: String,
    pub system_prompt: String,
    pub allowed_tools: serde_json::Value,
    pub default_route: String,
    pub metadata: serde_json::Value,
    pub is_builtin: bool,
    pub source_plugin: Option<String>,
    pub timeout_secs: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn upsert(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
    system_prompt: &str,
    allowed_tools: &serde_json::Value,
    default_route: &str,
    metadata: &serde_json::Value,
    is_builtin: bool,
    source_plugin: Option<&str>,
    timeout_secs: Option<i32>,
) -> Result<AgentRow, sqlx::Error> {
    sqlx::query_as::<_, AgentRow>(
        "INSERT INTO agents (name, system_prompt, allowed_tools, default_route, metadata, is_builtin, source_plugin, timeout_secs) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         ON CONFLICT (name) DO UPDATE SET \
           system_prompt = EXCLUDED.system_prompt, \
           allowed_tools = EXCLUDED.allowed_tools, \
           default_route = EXCLUDED.default_route, \
           metadata = EXCLUDED.metadata, \
           is_builtin = EXCLUDED.is_builtin, \
           source_plugin = EXCLUDED.source_plugin, \
           timeout_secs = EXCLUDED.timeout_secs, \
           updated_at = now() \
         RETURNING name, system_prompt, allowed_tools, default_route, metadata, is_builtin, source_plugin, timeout_secs, created_at, updated_at",
    )
    .bind(name)
    .bind(system_prompt)
    .bind(allowed_tools)
    .bind(default_route)
    .bind(metadata)
    .bind(is_builtin)
    .bind(source_plugin)
    .bind(timeout_secs)
    .fetch_one(db)
    .await
}

pub async fn get(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
) -> Result<Option<AgentRow>, sqlx::Error> {
    sqlx::query_as::<_, AgentRow>(
        "SELECT name, system_prompt, allowed_tools, default_route, metadata, is_builtin, source_plugin, timeout_secs, created_at, updated_at \
         FROM agents WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(db)
    .await
}

pub async fn list(
    db: impl sqlx::Executor<'_, Database = Postgres>,
) -> Result<Vec<AgentRow>, sqlx::Error> {
    sqlx::query_as::<_, AgentRow>(
        "SELECT name, system_prompt, allowed_tools, default_route, metadata, is_builtin, source_plugin, timeout_secs, created_at, updated_at \
         FROM agents ORDER BY name",
    )
    .fetch_all(db)
    .await
}

pub async fn delete(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM agents WHERE name = $1 AND is_builtin = false",
    )
    .bind(name)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
