use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ToolRunRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub tool_name: String,
    pub tool_version: i32,
    pub input_json: serde_json::Value,
    pub status: String,
    pub output_json: Option<serde_json::Value>,
    pub output_kind: Option<String>,
    pub error_json: Option<serde_json::Value>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub thread_id: Option<Uuid>,
    pub parent_message_id: Option<Uuid>,
    pub actor_subject: Option<String>,
    pub actor_user_id: Option<Uuid>,
    pub attempt: i32,
    pub lease_expires: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Enqueue a new tool run.
pub async fn enqueue(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    tool_name: &str,
    tool_version: i32,
    input_json: &serde_json::Value,
    thread_id: Option<Uuid>,
    parent_message_id: Option<Uuid>,
    actor_subject: Option<&str>,
    actor_user_id: Option<Uuid>,
) -> Result<ToolRunRow, sqlx::Error> {
    sqlx::query_as::<_, ToolRunRow>(
        "INSERT INTO tool_runs (project_id, tool_name, tool_version, input_json, thread_id, parent_message_id, actor_subject, actor_user_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, project_id, tool_name, tool_version, input_json, status,
                   output_json, output_kind, error_json, stdout, stderr,
                   thread_id, parent_message_id, actor_subject, actor_user_id,
                   attempt, lease_expires, created_at, started_at, completed_at",
    )
    .bind(project_id)
    .bind(tool_name)
    .bind(tool_version)
    .bind(input_json)
    .bind(thread_id)
    .bind(parent_message_id)
    .bind(actor_subject)
    .bind(actor_user_id)
    .fetch_one(db)
    .await
}

/// Claim a queued tool run (FOR UPDATE SKIP LOCKED).
pub async fn claim(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    lease_duration_secs: i64,
) -> Result<Option<ToolRunRow>, sqlx::Error> {
    sqlx::query_as::<_, ToolRunRow>(
        "UPDATE tool_runs SET
            status = 'running',
            attempt = attempt + 1,
            started_at = now(),
            lease_expires = now() + make_interval(secs => $1)
         WHERE id = (
            SELECT id FROM tool_runs
            WHERE status = 'queued'
            ORDER BY created_at
            FOR UPDATE SKIP LOCKED
            LIMIT 1
         )
         RETURNING id, project_id, tool_name, tool_version, input_json, status,
                   output_json, output_kind, error_json, stdout, stderr,
                   thread_id, parent_message_id, actor_subject, actor_user_id,
                   attempt, lease_expires, created_at, started_at, completed_at",
    )
    .bind(lease_duration_secs as f64)
    .fetch_optional(db)
    .await
}

/// Mark a tool run as completed.
pub async fn complete(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    output_json: &serde_json::Value,
    output_kind: &str,
    stdout: Option<&str>,
    stderr: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tool_runs SET
            status = 'completed',
            output_json = $2,
            output_kind = $3,
            stdout = $4,
            stderr = $5,
            completed_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(output_json)
    .bind(output_kind)
    .bind(stdout)
    .bind(stderr)
    .execute(db)
    .await?;
    Ok(())
}

/// Mark a tool run as failed.
pub async fn fail(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    error_json: &serde_json::Value,
    stderr: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tool_runs SET
            status = 'failed',
            error_json = $2,
            stderr = $3,
            completed_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(error_json)
    .bind(stderr)
    .execute(db)
    .await?;
    Ok(())
}

/// Heartbeat — extend the lease.
pub async fn heartbeat(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    lease_duration_secs: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tool_runs SET lease_expires = now() + make_interval(secs => $2) WHERE id = $1",
    )
    .bind(id)
    .bind(lease_duration_secs as f64)
    .execute(db)
    .await?;
    Ok(())
}

/// Get a tool run by ID.
pub async fn get(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<Option<ToolRunRow>, sqlx::Error> {
    sqlx::query_as::<_, ToolRunRow>(
        "SELECT id, project_id, tool_name, tool_version, input_json, status,
                output_json, output_kind, error_json, stdout, stderr,
                thread_id, parent_message_id, actor_subject, actor_user_id,
                attempt, lease_expires, created_at, started_at, completed_at
         FROM tool_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Reclaim expired leases — reset to queued for retry.
pub async fn reclaim_expired(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    max_attempts: i32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE tool_runs SET status = 'queued', lease_expires = NULL
         WHERE status = 'running' AND lease_expires < now() AND attempt < $1",
    )
    .bind(max_attempts)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Permanently fail runs that exceeded max attempts and have expired leases.
pub async fn fail_exhausted(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    max_attempts: i32,
) -> Result<u64, sqlx::Error> {
    let err_json = serde_json::json!({
        "code": "max_attempts_exceeded",
        "message": "tool run exceeded maximum retry attempts",
        "retryable": false,
        "details": null
    });
    let result = sqlx::query(
        "UPDATE tool_runs SET
            status = 'failed',
            error_json = $2,
            completed_at = now()
         WHERE status = 'running' AND lease_expires < now() AND attempt >= $1",
    )
    .bind(max_attempts)
    .bind(&err_json)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
