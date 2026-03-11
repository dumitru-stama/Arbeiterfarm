use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ThreadRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub agent_name: String,
    pub title: Option<String>,
    pub parent_thread_id: Option<Uuid>,
    pub thread_type: String,
    pub target_artifact_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

const THREAD_COLUMNS: &str =
    "id, project_id, agent_name, title, parent_thread_id, thread_type, target_artifact_id, created_at, updated_at";

pub async fn create_thread(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    agent_name: &str,
    title: Option<&str>,
) -> Result<ThreadRow, sqlx::Error> {
    create_thread_typed(db, project_id, agent_name, title, "agent").await
}

pub async fn create_thread_typed(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    agent_name: &str,
    title: Option<&str>,
    thread_type: &str,
) -> Result<ThreadRow, sqlx::Error> {
    create_thread_full(db, project_id, agent_name, title, thread_type, None).await
}

pub async fn create_thread_full(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    agent_name: &str,
    title: Option<&str>,
    thread_type: &str,
    target_artifact_id: Option<Uuid>,
) -> Result<ThreadRow, sqlx::Error> {
    let query = format!(
        "INSERT INTO threads (project_id, agent_name, title, thread_type, target_artifact_id) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING {THREAD_COLUMNS}"
    );
    let row = sqlx::query_as::<_, ThreadRow>(&query)
        .bind(project_id)
        .bind(agent_name)
        .bind(title)
        .bind(thread_type)
        .bind(target_artifact_id)
        .fetch_one(db)
        .await?;
    Ok(row)
}

pub async fn create_child_thread(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    agent_name: &str,
    title: Option<&str>,
    parent_thread_id: Uuid,
) -> Result<ThreadRow, sqlx::Error> {
    create_child_thread_typed(db, project_id, agent_name, title, parent_thread_id, "agent").await
}

pub async fn create_child_thread_typed(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    agent_name: &str,
    title: Option<&str>,
    parent_thread_id: Uuid,
    thread_type: &str,
) -> Result<ThreadRow, sqlx::Error> {
    let query = format!(
        "INSERT INTO threads (project_id, agent_name, title, parent_thread_id, thread_type) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING {THREAD_COLUMNS}"
    );
    let row = sqlx::query_as::<_, ThreadRow>(&query)
        .bind(project_id)
        .bind(agent_name)
        .bind(title)
        .bind(parent_thread_id)
        .bind(thread_type)
        .fetch_one(db)
        .await?;
    Ok(row)
}

pub async fn get_thread(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<Option<ThreadRow>, sqlx::Error> {
    let query = format!(
        "SELECT {THREAD_COLUMNS} FROM threads WHERE id = $1"
    );
    let row = sqlx::query_as::<_, ThreadRow>(&query)
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

pub async fn list_threads(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
) -> Result<Vec<ThreadRow>, sqlx::Error> {
    let query = format!(
        "SELECT {THREAD_COLUMNS} FROM threads WHERE project_id = $1 ORDER BY created_at DESC"
    );
    let rows = sqlx::query_as::<_, ThreadRow>(&query)
        .bind(project_id)
        .fetch_all(db)
        .await?;
    Ok(rows)
}

pub async fn list_child_threads(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    parent_thread_id: Uuid,
) -> Result<Vec<ThreadRow>, sqlx::Error> {
    let query = format!(
        "SELECT {THREAD_COLUMNS} FROM threads WHERE parent_thread_id = $1 ORDER BY created_at ASC"
    );
    let rows = sqlx::query_as::<_, ThreadRow>(&query)
        .bind(parent_thread_id)
        .fetch_all(db)
        .await?;
    Ok(rows)
}

/// Delete a single thread and all its child threads recursively.
/// Cleans up FK dependencies in correct order.
pub async fn delete_thread(
    db: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    // Use a recursive CTE to find this thread and all descendants
    let result = sqlx::query(
        "WITH RECURSIVE tree AS ( \
            SELECT id FROM threads WHERE id = $1 \
            UNION ALL \
            SELECT t.id FROM threads t JOIN tree ON t.parent_thread_id = tree.id \
        ), \
        del_memory AS ( \
            DELETE FROM thread_memory WHERE thread_id IN (SELECT id FROM tree) \
        ), \
        del_evidence AS ( \
            DELETE FROM message_evidence \
            WHERE message_id IN (SELECT id FROM messages WHERE thread_id IN (SELECT id FROM tree)) \
        ), \
        del_msgs AS ( \
            DELETE FROM messages WHERE thread_id IN (SELECT id FROM tree) \
        ), \
        del_email_log AS ( \
            DELETE FROM email_log WHERE thread_id IN (SELECT id FROM tree) \
        ), \
        del_email_sched AS ( \
            DELETE FROM email_scheduled WHERE thread_id IN (SELECT id FROM tree) \
        ) \
        DELETE FROM threads WHERE id IN (SELECT id FROM tree)",
    )
    .bind(thread_id)
    .execute(&mut *db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete threads (and their messages/evidence) older than the project's retention setting.
/// Only affects projects where settings->>'thread_retention_days' is set and is a valid integer.
/// Uses leaf-first deletion: only deletes threads with no children at all.
/// Parent threads are cleaned up in subsequent tick runs as their children are deleted first.
/// Returns count of deleted threads.
pub async fn purge_expired_threads(
    db: impl sqlx::Executor<'_, Database = Postgres>,
) -> Result<u64, sqlx::Error> {
    // The CTE must delete in FK order:
    //   1. message_evidence (FK → messages, RESTRICT)
    //   2. messages (FK → threads, RESTRICT)
    //   3. threads (self-FK via parent_thread_id, RESTRICT)
    // llm_usage_log has ON DELETE CASCADE so it's handled automatically.
    //
    // Leaf-first strategy: only delete expired threads that have NO children
    // in the threads table. This avoids the transitive parent FK violation
    // (e.g. A→B→C where A and B are expired but C is not — deleting A would
    // violate the FK from B). Parents converge to deletion over multiple ticks
    // as their children are removed first.
    //
    // Safe cast: filter thread_retention_days with regex to skip non-integer
    // values that would cause ::int to throw and fail the entire query.
    let result = sqlx::query(
        "WITH candidates AS ( \
            SELECT t.id FROM threads t \
            JOIN projects p ON t.project_id = p.id \
            WHERE (p.settings->>'thread_retention_days') IS NOT NULL \
            AND (p.settings->>'thread_retention_days') ~ '^[0-9]+$' \
            AND t.updated_at < NOW() - ((p.settings->>'thread_retention_days')::int || ' days')::interval \
        ), \
        expired AS ( \
            SELECT c.id FROM candidates c \
            WHERE NOT EXISTS ( \
                SELECT 1 FROM threads child \
                WHERE child.parent_thread_id = c.id \
            ) \
        ), \
        del_memory AS ( \
            DELETE FROM thread_memory WHERE thread_id IN (SELECT id FROM expired) \
        ), \
        del_evidence AS ( \
            DELETE FROM message_evidence \
            WHERE message_id IN (SELECT id FROM messages WHERE thread_id IN (SELECT id FROM expired)) \
        ), \
        del_msgs AS ( \
            DELETE FROM messages WHERE thread_id IN (SELECT id FROM expired) \
        ) \
        DELETE FROM threads WHERE id IN (SELECT id FROM expired)",
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
