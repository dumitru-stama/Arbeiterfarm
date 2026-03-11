use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationChannelRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub channel_type: String,
    pub config_json: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationQueueRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub channel_id: Uuid,
    pub subject: String,
    pub body: String,
    pub attachment_artifact_id: Option<Uuid>,
    pub status: String,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub submitted_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Channel operations
// ---------------------------------------------------------------------------

pub async fn create_channel<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
    name: &str,
    channel_type: &str,
    config_json: &serde_json::Value,
) -> Result<NotificationChannelRow, sqlx::Error> {
    sqlx::query_as::<_, NotificationChannelRow>(
        "INSERT INTO notification_channels (project_id, name, channel_type, config_json)
         VALUES ($1, $2, $3, $4)
         RETURNING *",
    )
    .bind(project_id)
    .bind(name)
    .bind(channel_type)
    .bind(config_json)
    .fetch_one(db)
    .await
}

pub async fn get_channel<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<Option<NotificationChannelRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationChannelRow>(
        "SELECT * FROM notification_channels WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn get_channel_by_name<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
    name: &str,
) -> Result<Option<NotificationChannelRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationChannelRow>(
        "SELECT * FROM notification_channels WHERE project_id = $1 AND name = $2",
    )
    .bind(project_id)
    .bind(name)
    .fetch_optional(db)
    .await
}

pub async fn list_channels<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
) -> Result<Vec<NotificationChannelRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationChannelRow>(
        "SELECT * FROM notification_channels WHERE project_id = $1 ORDER BY name",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}

pub async fn update_channel<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    config_json: &serde_json::Value,
    enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_channels SET config_json = $2, enabled = $3, updated_at = NOW()
         WHERE id = $1",
    )
    .bind(id)
    .bind(config_json)
    .bind(enabled)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update a channel scoped to a specific project (for API auth).
pub async fn update_channel_for_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    project_id: Uuid,
    config_json: &serde_json::Value,
    enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_channels SET config_json = $3, enabled = $4, updated_at = NOW()
         WHERE id = $1 AND project_id = $2",
    )
    .bind(id)
    .bind(project_id)
    .bind(config_json)
    .bind(enabled)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_channel<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM notification_channels WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a channel scoped to a specific project (for API auth).
pub async fn delete_channel_for_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM notification_channels WHERE id = $1 AND project_id = $2",
    )
    .bind(id)
    .bind(project_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Queue operations
// ---------------------------------------------------------------------------

pub async fn enqueue<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
    channel_id: Uuid,
    subject: &str,
    body: &str,
    attachment_artifact_id: Option<Uuid>,
    submitted_by: Option<Uuid>,
) -> Result<NotificationQueueRow, sqlx::Error> {
    sqlx::query_as::<_, NotificationQueueRow>(
        "INSERT INTO notification_queue
            (project_id, channel_id, subject, body, attachment_artifact_id, submitted_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING *",
    )
    .bind(project_id)
    .bind(channel_id)
    .bind(subject)
    .bind(body)
    .bind(attachment_artifact_id)
    .bind(submitted_by)
    .fetch_one(db)
    .await
}

/// Get a single queue item by ID.
pub async fn get_queue_item<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<Option<NotificationQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationQueueRow>(
        "SELECT * FROM notification_queue WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Fetch pending items ordered by creation time (oldest first).
pub async fn list_pending<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    limit: i64,
) -> Result<Vec<NotificationQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationQueueRow>(
        "SELECT * FROM notification_queue WHERE status = 'pending' ORDER BY created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Atomically claim a pending item for processing.
pub async fn claim<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_queue SET status = 'processing', updated_at = NOW()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark item as completed.
pub async fn complete<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notification_queue SET status = 'completed', completed_at = NOW(), updated_at = NOW(),
         error_message = NULL WHERE id = $1 AND status = 'processing'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Record failure. Resets to 'pending' if under max_attempts, else 'failed'.
pub async fn fail<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    error_msg: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notification_queue SET
           attempt_count = attempt_count + 1,
           error_message = $2,
           status = CASE WHEN attempt_count + 1 >= max_attempts THEN 'failed' ELSE 'pending' END,
           updated_at = NOW()
         WHERE id = $1 AND status = 'processing'",
    )
    .bind(id)
    .bind(error_msg)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a permanent failure — immediately sets status to 'failed' regardless of attempt count.
/// Use for non-transient errors (e.g. channel type not supported).
pub async fn fail_permanent<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    error_msg: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notification_queue SET
           status = 'failed',
           error_message = $2,
           attempt_count = max_attempts,
           updated_at = NOW()
         WHERE id = $1 AND status = 'processing'",
    )
    .bind(id)
    .bind(error_msg)
    .execute(db)
    .await?;
    Ok(())
}

/// Cancel a pending item.
pub async fn cancel<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_queue SET status = 'cancelled', updated_at = NOW()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Cancel a pending item scoped to project (for API auth).
pub async fn cancel_for_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_queue SET status = 'cancelled', updated_at = NOW()
         WHERE id = $1 AND project_id = $2 AND status = 'pending'",
    )
    .bind(id)
    .bind(project_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Recover items stuck in 'processing' state (from crashed tick workers).
pub async fn recover_stale<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    stale_minutes: i32,
) -> Result<Vec<NotificationQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationQueueRow>(
        "UPDATE notification_queue SET
            attempt_count = attempt_count + 1,
            status = CASE
                WHEN attempt_count + 1 >= max_attempts THEN 'failed'
                ELSE 'pending'
            END,
            error_message = CASE
                WHEN attempt_count + 1 >= max_attempts
                THEN 'permanently failed: exceeded max retries after stale recovery'
                ELSE 'recovered from stale processing state'
            END,
            updated_at = NOW()
         WHERE status = 'processing'
           AND updated_at < NOW() - make_interval(mins := $1)
         RETURNING *",
    )
    .bind(stale_minutes)
    .fetch_all(db)
    .await
}

/// List queue items with optional filters (for CLI and API).
pub async fn list_queue<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<NotificationQueueRow>, sqlx::Error> {
    match (project_id, status) {
        (Some(pid), Some(st)) => {
            sqlx::query_as::<_, NotificationQueueRow>(
                "SELECT * FROM notification_queue WHERE project_id = $1 AND status = $2
                 ORDER BY created_at DESC LIMIT $3",
            )
            .bind(pid)
            .bind(st)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (Some(pid), None) => {
            sqlx::query_as::<_, NotificationQueueRow>(
                "SELECT * FROM notification_queue WHERE project_id = $1
                 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(pid)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, Some(st)) => {
            sqlx::query_as::<_, NotificationQueueRow>(
                "SELECT * FROM notification_queue WHERE status = $1
                 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(st)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, None) => {
            sqlx::query_as::<_, NotificationQueueRow>(
                "SELECT * FROM notification_queue ORDER BY created_at DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(db)
            .await
        }
    }
}

/// Reset a failed item back to pending for retry.
pub async fn retry<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_queue SET status = 'pending', error_message = NULL,
         attempt_count = 0, updated_at = NOW()
         WHERE id = $1 AND status = 'failed'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Retry a failed item scoped to project (for API auth).
pub async fn retry_for_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE notification_queue SET status = 'pending', error_message = NULL,
         attempt_count = 0, updated_at = NOW()
         WHERE id = $1 AND project_id = $2 AND status = 'failed'",
    )
    .bind(id)
    .bind(project_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
