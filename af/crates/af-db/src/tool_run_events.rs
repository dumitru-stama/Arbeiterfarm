use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ToolRunEventRow {
    pub id: Uuid,
    pub tool_run_id: Uuid,
    pub event_type: String,
    pub payload: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

pub async fn insert_event(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    tool_run_id: Uuid,
    event_type: &str,
    payload: Option<&serde_json::Value>,
) -> Result<ToolRunEventRow, sqlx::Error> {
    sqlx::query_as::<_, ToolRunEventRow>(
        "INSERT INTO tool_run_events (tool_run_id, event_type, payload)
         VALUES ($1, $2, $3)
         RETURNING id, tool_run_id, event_type, payload, created_at",
    )
    .bind(tool_run_id)
    .bind(event_type)
    .bind(payload)
    .fetch_one(db)
    .await
}

/// List events for a tool run, optionally filtered by event type.
pub async fn list_events(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    tool_run_id: Uuid,
    event_type: Option<&str>,
) -> Result<Vec<ToolRunEventRow>, sqlx::Error> {
    match event_type {
        Some(et) => {
            sqlx::query_as::<_, ToolRunEventRow>(
                "SELECT id, tool_run_id, event_type, payload, created_at \
                 FROM tool_run_events \
                 WHERE tool_run_id = $1 AND event_type = $2 \
                 ORDER BY created_at ASC",
            )
            .bind(tool_run_id)
            .bind(et)
            .fetch_all(db)
            .await
        }
        None => {
            sqlx::query_as::<_, ToolRunEventRow>(
                "SELECT id, tool_run_id, event_type, payload, created_at \
                 FROM tool_run_events \
                 WHERE tool_run_id = $1 \
                 ORDER BY created_at ASC",
            )
            .bind(tool_run_id)
            .fetch_all(db)
            .await
        }
    }
}
