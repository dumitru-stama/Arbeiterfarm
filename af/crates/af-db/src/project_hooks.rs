use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::Postgres;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectHookRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub event_type: String,
    pub workflow_name: Option<String>,
    pub agent_name: Option<String>,
    pub prompt_template: String,
    pub route_override: Option<String>,
    pub tick_interval_minutes: Option<i32>,
    pub last_tick_at: Option<DateTime<Utc>>,
    pub tick_generation: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn create(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    name: &str,
    event_type: &str,
    workflow_name: Option<&str>,
    agent_name: Option<&str>,
    prompt_template: &str,
    route_override: Option<&str>,
    tick_interval_minutes: Option<i32>,
) -> Result<ProjectHookRow, sqlx::Error> {
    sqlx::query_as::<_, ProjectHookRow>(
        "INSERT INTO project_hooks \
         (project_id, name, event_type, workflow_name, agent_name, prompt_template, \
          route_override, tick_interval_minutes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, project_id, name, enabled, event_type, workflow_name, agent_name, \
                   prompt_template, route_override, tick_interval_minutes, last_tick_at, \
                   tick_generation, created_at, updated_at",
    )
    .bind(project_id)
    .bind(name)
    .bind(event_type)
    .bind(workflow_name)
    .bind(agent_name)
    .bind(prompt_template)
    .bind(route_override)
    .bind(tick_interval_minutes)
    .fetch_one(db)
    .await
}

pub async fn get(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<Option<ProjectHookRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectHookRow>(
        "SELECT id, project_id, name, enabled, event_type, workflow_name, agent_name, \
                prompt_template, route_override, tick_interval_minutes, last_tick_at, \
                tick_generation, created_at, updated_at \
         FROM project_hooks WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn list_by_project(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
) -> Result<Vec<ProjectHookRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectHookRow>(
        "SELECT id, project_id, name, enabled, event_type, workflow_name, agent_name, \
                prompt_template, route_override, tick_interval_minutes, last_tick_at, \
                tick_generation, created_at, updated_at \
         FROM project_hooks WHERE project_id = $1 ORDER BY created_at",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}

pub async fn list_enabled_by_event(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    event_type: &str,
) -> Result<Vec<ProjectHookRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectHookRow>(
        "SELECT id, project_id, name, enabled, event_type, workflow_name, agent_name, \
                prompt_template, route_override, tick_interval_minutes, last_tick_at, \
                tick_generation, created_at, updated_at \
         FROM project_hooks \
         WHERE project_id = $1 AND event_type = $2 AND enabled = true \
         ORDER BY created_at",
    )
    .bind(project_id)
    .bind(event_type)
    .fetch_all(db)
    .await
}

/// List all tick hooks that are due (cross-project).
/// A hook is due when it has never fired or when enough time has elapsed since last_tick_at.
pub async fn list_due_tick_hooks(
    db: impl sqlx::Executor<'_, Database = Postgres>,
) -> Result<Vec<ProjectHookRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectHookRow>(
        "SELECT id, project_id, name, enabled, event_type, workflow_name, agent_name, \
                prompt_template, route_override, tick_interval_minutes, last_tick_at, \
                tick_generation, created_at, updated_at \
         FROM project_hooks \
         WHERE event_type = 'tick' AND enabled = true \
           AND (last_tick_at IS NULL \
                OR last_tick_at + (tick_interval_minutes || ' minutes')::interval <= now()) \
         ORDER BY last_tick_at NULLS FIRST",
    )
    .fetch_all(db)
    .await
}

/// Atomically claim a tick hook for execution using CAS on tick_generation.
/// Returns true if the claim succeeded (generation matched and row was updated).
pub async fn claim_tick(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    hook_id: Uuid,
    expected_generation: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE project_hooks \
         SET last_tick_at = now(), tick_generation = tick_generation + 1, updated_at = now() \
         WHERE id = $1 AND tick_generation = $2",
    )
    .bind(hook_id)
    .bind(expected_generation)
    .execute(db)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn update(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    enabled: Option<bool>,
    prompt_template: Option<&str>,
    route_override: Option<Option<&str>>,
    tick_interval_minutes: Option<i32>,
) -> Result<Option<ProjectHookRow>, sqlx::Error> {
    // Build dynamic SET clause
    let mut sets = vec!["updated_at = now()".to_string()];
    let mut bind_idx = 2u32; // $1 is id

    if enabled.is_some() {
        sets.push(format!("enabled = ${bind_idx}"));
        bind_idx += 1;
    }
    if prompt_template.is_some() {
        sets.push(format!("prompt_template = ${bind_idx}"));
        bind_idx += 1;
    }
    if route_override.is_some() {
        sets.push(format!("route_override = ${bind_idx}"));
        bind_idx += 1;
    }
    if tick_interval_minutes.is_some() {
        sets.push(format!("tick_interval_minutes = ${bind_idx}"));
        // bind_idx not needed further
    }

    let sql = format!(
        "UPDATE project_hooks SET {} WHERE id = $1 \
         RETURNING id, project_id, name, enabled, event_type, workflow_name, agent_name, \
                   prompt_template, route_override, tick_interval_minutes, last_tick_at, \
                   tick_generation, created_at, updated_at",
        sets.join(", ")
    );

    let mut query = sqlx::query_as::<_, ProjectHookRow>(&sql).bind(id);

    if let Some(v) = enabled {
        query = query.bind(v);
    }
    if let Some(v) = prompt_template {
        query = query.bind(v);
    }
    if let Some(v) = route_override {
        query = query.bind(v);
    }
    if let Some(v) = tick_interval_minutes {
        query = query.bind(v);
    }

    query.fetch_optional(db).await
}

pub async fn delete(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM project_hooks WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
