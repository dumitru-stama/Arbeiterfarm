use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ApiKeyRow {
    pub id: Uuid,
    #[serde(default)]
    pub user_id: Uuid,
    #[serde(default)]
    pub key_hash: String,
    pub key_prefix: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn create_key(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
    key_hash: &str,
    key_prefix: &str,
    name: &str,
    scopes: &[String],
) -> Result<ApiKeyRow, sqlx::Error> {
    sqlx::query_as::<_, ApiKeyRow>(
        "INSERT INTO api_keys (user_id, key_hash, key_prefix, name, scopes) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, user_id, key_hash, key_prefix, name, scopes, \
                   expires_at, last_used_at, created_at",
    )
    .bind(user_id)
    .bind(key_hash)
    .bind(key_prefix)
    .bind(name)
    .bind(scopes)
    .fetch_one(db)
    .await
}

pub async fn lookup_by_hash(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    key_hash: &str,
) -> Result<Option<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiKeyRow>(
        "SELECT id, user_id, key_hash, key_prefix, name, scopes, \
                expires_at, last_used_at, created_at \
         FROM api_keys WHERE key_hash = $1",
    )
    .bind(key_hash)
    .fetch_optional(db)
    .await
}

pub async fn update_last_used(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    key_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
        .bind(key_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn list_for_user(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<Vec<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiKeyRow>(
        "SELECT id, user_id, key_hash, key_prefix, name, scopes, \
                expires_at, last_used_at, created_at \
         FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn delete(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    key_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = $1")
        .bind(key_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
