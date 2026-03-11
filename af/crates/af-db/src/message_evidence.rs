use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct MessageEvidenceRow {
    pub id: Uuid,
    pub message_id: Uuid,
    pub ref_type: String,
    pub ref_id: Uuid,
    pub created_at: DateTime<Utc>,
}

pub async fn insert_evidence(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    message_id: Uuid,
    ref_type: &str,
    ref_id: Uuid,
) -> Result<MessageEvidenceRow, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageEvidenceRow>(
        "INSERT INTO message_evidence (message_id, ref_type, ref_id) \
         VALUES ($1, $2, $3) \
         RETURNING id, message_id, ref_type, ref_id, created_at",
    )
    .bind(message_id)
    .bind(ref_type)
    .bind(ref_id)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_for_message(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    message_id: Uuid,
) -> Result<Vec<MessageEvidenceRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MessageEvidenceRow>(
        "SELECT id, message_id, ref_type, ref_id, created_at \
         FROM message_evidence WHERE message_id = $1 ORDER BY created_at ASC",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn get_for_thread(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
) -> Result<Vec<MessageEvidenceRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MessageEvidenceRow>(
        "SELECT me.id, me.message_id, me.ref_type, me.ref_id, me.created_at \
         FROM message_evidence me \
         JOIN messages m ON me.message_id = m.id \
         WHERE m.thread_id = $1 \
         ORDER BY me.created_at ASC",
    )
    .bind(thread_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}
