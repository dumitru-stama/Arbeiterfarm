use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct WorkflowRow {
    pub name: String,
    pub description: Option<String>,
    pub steps: serde_json::Value,
    pub is_builtin: bool,
    pub source_plugin: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub agent: String,
    pub group: u32,
    pub prompt: String,
    #[serde(default = "default_can_repivot")]
    pub can_repivot: bool,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
    /// When true, this step may run concurrently with other parallel-flagged
    /// steps in the same group. Steps without this flag run sequentially
    /// after the parallel batch completes. Default: false (backward-compatible).
    #[serde(default)]
    pub parallel: bool,
}

fn default_can_repivot() -> bool {
    true
}

pub async fn upsert(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
    description: Option<&str>,
    steps: &serde_json::Value,
    is_builtin: bool,
    source_plugin: Option<&str>,
) -> Result<WorkflowRow, sqlx::Error> {
    sqlx::query_as::<_, WorkflowRow>(
        "INSERT INTO workflows (name, description, steps, is_builtin, source_plugin) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (name) DO UPDATE SET \
           description = EXCLUDED.description, \
           steps = EXCLUDED.steps, \
           is_builtin = EXCLUDED.is_builtin, \
           source_plugin = EXCLUDED.source_plugin, \
           updated_at = now() \
         RETURNING name, description, steps, is_builtin, source_plugin, created_at, updated_at",
    )
    .bind(name)
    .bind(description)
    .bind(steps)
    .bind(is_builtin)
    .bind(source_plugin)
    .fetch_one(db)
    .await
}

pub async fn get(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
) -> Result<Option<WorkflowRow>, sqlx::Error> {
    sqlx::query_as::<_, WorkflowRow>(
        "SELECT name, description, steps, is_builtin, source_plugin, created_at, updated_at \
         FROM workflows WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(db)
    .await
}

pub async fn list(
    db: impl sqlx::Executor<'_, Database = Postgres>,
) -> Result<Vec<WorkflowRow>, sqlx::Error> {
    sqlx::query_as::<_, WorkflowRow>(
        "SELECT name, description, steps, is_builtin, source_plugin, created_at, updated_at \
         FROM workflows ORDER BY name",
    )
    .fetch_all(db)
    .await
}

pub async fn delete(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM workflows WHERE name = $1 AND is_builtin = false",
    )
    .bind(name)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
