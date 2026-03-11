use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmbedQueueRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub chunks_artifact_id: Uuid,
    pub source_artifact_id: Option<Uuid>,
    pub tool_name: String,
    pub status: String,
    pub chunk_count: Option<i32>,
    pub chunks_embedded: i32,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Enqueue a chunks.json artifact for background embedding.
/// Uses ON CONFLICT DO NOTHING to silently skip if already enqueued
/// (the partial unique index prevents duplicate active entries).
pub async fn enqueue<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
    chunks_artifact_id: Uuid,
    source_artifact_id: Option<Uuid>,
    tool_name: &str,
) -> Result<Option<EmbedQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, EmbedQueueRow>(
        "INSERT INTO embed_queue (project_id, chunks_artifact_id, source_artifact_id, tool_name)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (chunks_artifact_id) WHERE status NOT IN ('failed', 'cancelled')
         DO NOTHING
         RETURNING *",
    )
    .bind(project_id)
    .bind(chunks_artifact_id)
    .bind(source_artifact_id)
    .bind(tool_name)
    .fetch_optional(db)
    .await
}

/// Fetch pending items ordered by creation time (oldest first).
pub async fn list_pending<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    limit: i64,
) -> Result<Vec<EmbedQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, EmbedQueueRow>(
        "SELECT * FROM embed_queue WHERE status = 'pending' ORDER BY created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Atomically claim a pending item for processing.
/// Returns true if successfully claimed, false if already claimed by another worker.
pub async fn claim<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE embed_queue SET status = 'processing', updated_at = NOW()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update progress counters after processing a batch of chunks.
pub async fn update_progress<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    chunks_embedded: i32,
    chunk_count: Option<i32>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE embed_queue SET chunks_embedded = $2,
         chunk_count = COALESCE($3, chunk_count), updated_at = NOW()
         WHERE id = $1",
    )
    .bind(id)
    .bind(chunks_embedded)
    .bind(chunk_count)
    .execute(db)
    .await?;
    Ok(())
}

/// Mark item as completed.
pub async fn complete<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE embed_queue SET status = 'completed', completed_at = NOW(), updated_at = NOW(),
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
        "UPDATE embed_queue SET
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

/// Cancel a pending item.
pub async fn cancel<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE embed_queue SET status = 'cancelled', updated_at = NOW()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// List queue items with optional filters (for CLI).
pub async fn list_queue<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<EmbedQueueRow>, sqlx::Error> {
    match (project_id, status) {
        (Some(pid), Some(st)) => {
            sqlx::query_as::<_, EmbedQueueRow>(
                "SELECT * FROM embed_queue WHERE project_id = $1 AND status = $2
                 ORDER BY created_at DESC LIMIT $3",
            )
            .bind(pid)
            .bind(st)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (Some(pid), None) => {
            sqlx::query_as::<_, EmbedQueueRow>(
                "SELECT * FROM embed_queue WHERE project_id = $1
                 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(pid)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, Some(st)) => {
            sqlx::query_as::<_, EmbedQueueRow>(
                "SELECT * FROM embed_queue WHERE status = $1
                 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(st)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, None) => {
            sqlx::query_as::<_, EmbedQueueRow>(
                "SELECT * FROM embed_queue ORDER BY created_at DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(db)
            .await
        }
    }
}

/// Recover items stuck in 'processing' state (from crashed tick workers).
/// Items with `updated_at` older than `stale_minutes` are incremented and
/// either reset to 'pending' (if under max_attempts) or marked 'failed'.
/// Returns the updated rows so the caller can count recovered vs permanently failed.
pub async fn recover_stale<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    stale_minutes: i32,
) -> Result<Vec<EmbedQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, EmbedQueueRow>(
        "UPDATE embed_queue SET
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

/// Reset a failed item back to pending for retry.
/// Resets attempt_count so it gets a fresh set of retries.
pub async fn retry<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE embed_queue SET status = 'pending', error_message = NULL,
         attempt_count = 0, updated_at = NOW()
         WHERE id = $1 AND status = 'failed'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
