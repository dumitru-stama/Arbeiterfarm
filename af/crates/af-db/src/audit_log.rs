use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AuditLogRow {
    pub id: Uuid,
    pub event_type: String,
    pub actor_subject: Option<String>,
    pub actor_user_id: Option<Uuid>,
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

pub async fn insert(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    event_type: &str,
    actor: Option<&str>,
    actor_user_id: Option<Uuid>,
    detail: Option<&serde_json::Value>,
) -> Result<Uuid, sqlx::Error> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO audit_log (event_type, actor_subject, actor_user_id, detail) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(event_type)
    .bind(actor)
    .bind(actor_user_id)
    .bind(detail)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

pub async fn list(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    limit: i64,
    event_type: Option<&str>,
) -> Result<Vec<AuditLogRow>, sqlx::Error> {
    if let Some(et) = event_type {
        sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, event_type, actor_subject, actor_user_id, detail, created_at \
             FROM audit_log WHERE event_type = $1 \
             ORDER BY created_at DESC LIMIT $2",
        )
        .bind(et)
        .bind(limit)
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, event_type, actor_subject, actor_user_id, detail, created_at \
             FROM audit_log ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(db)
        .await
    }
}
