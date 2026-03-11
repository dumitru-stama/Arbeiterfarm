use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WebFetchRuleRow {
    pub id: Uuid,
    pub scope: String,
    pub project_id: Option<Uuid>,
    pub rule_type: String,
    pub pattern_type: String,
    pub pattern: String,
    pub description: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CountryBlockRow {
    pub country_code: String,
    pub country_name: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CacheRow {
    pub url_hash: String,
    pub url: String,
    pub status_code: i32,
    pub content_type: Option<String>,
    pub body: Option<String>,
    pub headers: Option<serde_json::Value>,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Rules CRUD
// ---------------------------------------------------------------------------

pub async fn list_rules<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    scope: Option<&str>,
    project_id: Option<Uuid>,
) -> Result<Vec<WebFetchRuleRow>, sqlx::Error> {
    if let Some(pid) = project_id {
        sqlx::query_as::<_, WebFetchRuleRow>(
            "SELECT * FROM web_fetch_rules WHERE (scope = 'global' OR project_id = $1) ORDER BY created_at",
        )
        .bind(pid)
        .fetch_all(db)
        .await
    } else if let Some(s) = scope {
        sqlx::query_as::<_, WebFetchRuleRow>(
            "SELECT * FROM web_fetch_rules WHERE scope = $1 ORDER BY created_at",
        )
        .bind(s)
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, WebFetchRuleRow>(
            "SELECT * FROM web_fetch_rules ORDER BY created_at",
        )
        .fetch_all(db)
        .await
    }
}

pub async fn add_rule<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    scope: &str,
    project_id: Option<Uuid>,
    rule_type: &str,
    pattern_type: &str,
    pattern: &str,
    description: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<WebFetchRuleRow, sqlx::Error> {
    sqlx::query_as::<_, WebFetchRuleRow>(
        "INSERT INTO web_fetch_rules (scope, project_id, rule_type, pattern_type, pattern, description, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *",
    )
    .bind(scope)
    .bind(project_id)
    .bind(rule_type)
    .bind(pattern_type)
    .bind(pattern)
    .bind(description)
    .bind(created_by)
    .fetch_one(db)
    .await
}

pub async fn remove_rule<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM web_fetch_rules WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Country blocks CRUD
// ---------------------------------------------------------------------------

pub async fn list_country_blocks<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
) -> Result<Vec<CountryBlockRow>, sqlx::Error> {
    sqlx::query_as::<_, CountryBlockRow>(
        "SELECT * FROM web_fetch_country_blocks ORDER BY country_code",
    )
    .fetch_all(db)
    .await
}

pub async fn add_country_block<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    country_code: &str,
    country_name: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<CountryBlockRow, sqlx::Error> {
    sqlx::query_as::<_, CountryBlockRow>(
        "INSERT INTO web_fetch_country_blocks (country_code, country_name, created_by)
         VALUES ($1, $2, $3)
         ON CONFLICT (country_code) DO UPDATE SET country_name = EXCLUDED.country_name
         RETURNING *",
    )
    .bind(country_code)
    .bind(country_name)
    .bind(created_by)
    .fetch_one(db)
    .await
}

pub async fn remove_country_block<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    country_code: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM web_fetch_country_blocks WHERE country_code = $1")
        .bind(country_code)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

pub async fn cache_get<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    url_hash: &str,
) -> Result<Option<CacheRow>, sqlx::Error> {
    sqlx::query_as::<_, CacheRow>(
        "SELECT * FROM web_fetch_cache WHERE url_hash = $1 AND expires_at > NOW()",
    )
    .bind(url_hash)
    .fetch_optional(db)
    .await
}

pub async fn cache_put<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    url_hash: &str,
    url: &str,
    status_code: i32,
    content_type: Option<&str>,
    body: Option<&str>,
    headers: Option<&serde_json::Value>,
    ttl_secs: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO web_fetch_cache (url_hash, url, status_code, content_type, body, headers, fetched_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW() + make_interval(secs => $7))
         ON CONFLICT (url_hash) DO UPDATE SET
           url = EXCLUDED.url,
           status_code = EXCLUDED.status_code,
           content_type = EXCLUDED.content_type,
           body = EXCLUDED.body,
           headers = EXCLUDED.headers,
           fetched_at = EXCLUDED.fetched_at,
           expires_at = EXCLUDED.expires_at",
    )
    .bind(url_hash)
    .bind(url)
    .bind(status_code)
    .bind(content_type)
    .bind(body)
    .bind(headers)
    .bind(ttl_secs as f64)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn cache_purge_expired<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM web_fetch_cache WHERE expires_at <= NOW()")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}
