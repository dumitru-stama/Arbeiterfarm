use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UrlIngestRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub url: String,
    pub status: String,
    pub title: Option<String>,
    pub content_length: Option<i32>,
    pub text_artifact_id: Option<Uuid>,
    pub chunks_artifact_id: Option<Uuid>,
    pub chunk_count: Option<i32>,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub submitted_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Bulk enqueue URLs. Uses ON CONFLICT DO NOTHING to skip duplicates
/// (the partial unique index prevents duplicate active entries per project+url).
pub async fn enqueue_urls<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
    urls: &[String],
    submitted_by: Option<Uuid>,
) -> Result<Vec<UrlIngestRow>, sqlx::Error> {
    // Build a multi-row INSERT with unnest for efficiency
    sqlx::query_as::<_, UrlIngestRow>(
        "INSERT INTO url_ingest_queue (project_id, url, submitted_by)
         SELECT $1, u, $3
         FROM unnest($2::text[]) AS u
         ON CONFLICT (project_id, url) WHERE status NOT IN ('failed', 'cancelled')
         DO NOTHING
         RETURNING *",
    )
    .bind(project_id)
    .bind(urls)
    .bind(submitted_by)
    .fetch_all(db)
    .await
}

/// Fetch pending items oldest-first.
pub async fn list_pending<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    limit: i64,
) -> Result<Vec<UrlIngestRow>, sqlx::Error> {
    sqlx::query_as::<_, UrlIngestRow>(
        "SELECT * FROM url_ingest_queue WHERE status = 'pending' ORDER BY created_at LIMIT $1",
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
        "UPDATE url_ingest_queue SET status = 'processing', updated_at = NOW()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark item as completed with result metadata.
#[allow(clippy::too_many_arguments)]
pub async fn complete<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    title: Option<&str>,
    content_length: i32,
    text_artifact_id: Uuid,
    chunks_artifact_id: Uuid,
    chunk_count: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE url_ingest_queue SET
           status = 'completed',
           title = $2,
           content_length = $3,
           text_artifact_id = $4,
           chunks_artifact_id = $5,
           chunk_count = $6,
           error_message = NULL,
           completed_at = NOW(),
           updated_at = NOW()
         WHERE id = $1 AND status = 'processing'",
    )
    .bind(id)
    .bind(title)
    .bind(content_length)
    .bind(text_artifact_id)
    .bind(chunks_artifact_id)
    .bind(chunk_count)
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
        "UPDATE url_ingest_queue SET
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
        "UPDATE url_ingest_queue SET status = 'cancelled', updated_at = NOW()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Cancel a pending item, scoped to a specific project (for API auth).
pub async fn cancel_for_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE url_ingest_queue SET status = 'cancelled', updated_at = NOW()
         WHERE id = $1 AND project_id = $2 AND status = 'pending'",
    )
    .bind(id)
    .bind(project_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Recover items stuck in 'processing' state from crashed workers.
pub async fn recover_stale<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    stale_minutes: i32,
) -> Result<Vec<UrlIngestRow>, sqlx::Error> {
    sqlx::query_as::<_, UrlIngestRow>(
        "UPDATE url_ingest_queue SET
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

/// List queue items with optional filters.
pub async fn list_queue<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<UrlIngestRow>, sqlx::Error> {
    match (project_id, status) {
        (Some(pid), Some(st)) => {
            sqlx::query_as::<_, UrlIngestRow>(
                "SELECT * FROM url_ingest_queue WHERE project_id = $1 AND status = $2
                 ORDER BY created_at DESC LIMIT $3",
            )
            .bind(pid)
            .bind(st)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (Some(pid), None) => {
            sqlx::query_as::<_, UrlIngestRow>(
                "SELECT * FROM url_ingest_queue WHERE project_id = $1
                 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(pid)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, Some(st)) => {
            sqlx::query_as::<_, UrlIngestRow>(
                "SELECT * FROM url_ingest_queue WHERE status = $1
                 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(st)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, None) => {
            sqlx::query_as::<_, UrlIngestRow>(
                "SELECT * FROM url_ingest_queue ORDER BY created_at DESC LIMIT $1",
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
        "UPDATE url_ingest_queue SET status = 'pending', error_message = NULL,
         attempt_count = 0, updated_at = NOW()
         WHERE id = $1 AND status = 'failed'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Reset a failed item back to pending, scoped to a specific project (for API auth).
pub async fn retry_for_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE url_ingest_queue SET status = 'pending', error_message = NULL,
         attempt_count = 0, updated_at = NOW()
         WHERE id = $1 AND project_id = $2 AND status = 'failed'",
    )
    .bind(id)
    .bind(project_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
