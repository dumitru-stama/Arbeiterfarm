use serde::{Deserialize, Serialize};
use sqlx::{Executor, FromRow, Postgres};
use uuid::Uuid;

/// A row returned by aggregation queries (SUM grouped by route).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UsageByRouteRow {
    pub route: String,
    pub call_count: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cached_read_tokens: i64,
    pub cache_creation_tokens: i64,
}

/// Insert a single LLM usage log entry (one per LLM API call).
pub async fn insert<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    thread_id: Uuid,
    project_id: Uuid,
    user_id: Option<Uuid>,
    route: &str,
    prompt_tokens: u32,
    completion_tokens: u32,
    cached_read_tokens: u32,
    cache_creation_tokens: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO llm_usage_log (thread_id, project_id, user_id, route, prompt_tokens, completion_tokens, cached_read_tokens, cache_creation_tokens)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
    )
    .bind(thread_id)
    .bind(project_id)
    .bind(user_id)
    .bind(route)
    .bind(prompt_tokens as i32)
    .bind(completion_tokens as i32)
    .bind(cached_read_tokens as i32)
    .bind(cache_creation_tokens as i32)
    .execute(db)
    .await?;
    Ok(())
}

/// Aggregate usage by route for a project.
pub async fn aggregate_by_project<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
) -> Result<Vec<UsageByRouteRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UsageByRouteRow>(
        "SELECT route,
                COUNT(*) as call_count,
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_read_tokens), 0) as cached_read_tokens,
                COALESCE(SUM(cache_creation_tokens), 0) as cache_creation_tokens
         FROM llm_usage_log
         WHERE project_id = $1
         GROUP BY route
         ORDER BY route"
    )
    .bind(project_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Aggregate usage by route for a thread.
pub async fn aggregate_by_thread<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    thread_id: Uuid,
) -> Result<Vec<UsageByRouteRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UsageByRouteRow>(
        "SELECT route,
                COUNT(*) as call_count,
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_read_tokens), 0) as cached_read_tokens,
                COALESCE(SUM(cache_creation_tokens), 0) as cache_creation_tokens
         FROM llm_usage_log
         WHERE thread_id = $1
         GROUP BY route
         ORDER BY route"
    )
    .bind(thread_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Aggregate usage by route for a user within a calendar month.
pub async fn aggregate_by_user_month<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
    year: i32,
    month: u32,
) -> Result<Vec<UsageByRouteRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UsageByRouteRow>(
        "SELECT route,
                COUNT(*) as call_count,
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_read_tokens), 0) as cached_read_tokens,
                COALESCE(SUM(cache_creation_tokens), 0) as cache_creation_tokens
         FROM llm_usage_log
         WHERE user_id = $1
           AND EXTRACT(YEAR FROM created_at) = $2
           AND EXTRACT(MONTH FROM created_at) = $3
         GROUP BY route
         ORDER BY route"
    )
    .bind(user_id)
    .bind(year)
    .bind(month as i32)
    .fetch_all(db)
    .await?;
    Ok(rows)
}
