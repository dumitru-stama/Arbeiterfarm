use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserRow {
    pub id: Uuid,
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn create_user(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    subject: &str,
    display_name: Option<&str>,
    email: Option<&str>,
    roles: &[String],
) -> Result<UserRow, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (subject, display_name, email, roles) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, subject, display_name, email, roles, enabled, created_at, updated_at",
    )
    .bind(subject)
    .bind(display_name)
    .bind(email)
    .bind(roles)
    .fetch_one(db)
    .await
}

pub async fn get_by_id(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, subject, display_name, email, roles, enabled, created_at, updated_at \
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn get_by_subject(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    subject: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, subject, display_name, email, roles, enabled, created_at, updated_at \
         FROM users WHERE subject = $1",
    )
    .bind(subject)
    .fetch_optional(db)
    .await
}

pub async fn list_users(
    db: impl sqlx::Executor<'_, Database = Postgres>,
) -> Result<Vec<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, subject, display_name, email, roles, enabled, created_at, updated_at \
         FROM users ORDER BY created_at DESC",
    )
    .fetch_all(db)
    .await
}
