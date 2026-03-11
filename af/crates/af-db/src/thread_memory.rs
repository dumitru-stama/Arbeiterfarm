use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ThreadMemoryRow {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub key: String,
    pub value: String,
}

/// Upsert a memory entry for a thread. Overwrites on (thread_id, key) conflict.
pub async fn upsert_memory(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    key: &str,
    value: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO thread_memory (thread_id, key, value) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (thread_id, key) DO UPDATE SET value = $3, updated_at = now()",
    )
    .bind(thread_id)
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

/// Get all memory entries for a thread, ordered by key.
pub async fn get_thread_memory(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
) -> Result<Vec<ThreadMemoryRow>, sqlx::Error> {
    sqlx::query_as::<_, ThreadMemoryRow>(
        "SELECT id, thread_id, key, value FROM thread_memory \
         WHERE thread_id = $1 ORDER BY key",
    )
    .bind(thread_id)
    .fetch_all(db)
    .await
}

/// Delete all memory entries for a thread.
pub async fn delete_thread_memory(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM thread_memory WHERE thread_id = $1")
        .bind(thread_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}
