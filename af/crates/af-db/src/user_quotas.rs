use chrono::NaiveDate;
use sqlx::{FromRow, PgPool, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct UserQuotaRow {
    pub user_id: Uuid,
    pub max_storage_bytes: i64,
    pub max_projects: i32,
    pub max_concurrent_runs: i32,
    pub max_llm_tokens_per_day: i64,
    pub max_upload_bytes: i64,
    pub max_vt_lookups_per_day: i32,
}

#[derive(Debug, Clone, FromRow)]
pub struct UsageDailyRow {
    pub user_id: Uuid,
    pub date: NaiveDate,
    pub llm_prompt_tokens: i64,
    pub llm_completion_tokens: i64,
    pub vt_lookups: i32,
    pub tool_runs: i32,
}

/// Get the quota row for a user, if one exists.
pub async fn get_quota(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<Option<UserQuotaRow>, sqlx::Error> {
    sqlx::query_as::<_, UserQuotaRow>(
        "SELECT user_id, max_storage_bytes, max_projects, max_concurrent_runs, \
         max_llm_tokens_per_day, max_upload_bytes, max_vt_lookups_per_day \
         FROM user_quotas WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// Ensure a quota row exists for the user (insert defaults if missing), then return it.
/// Multi-query: keeps &PgPool.
pub async fn ensure_quota(pool: &PgPool, user_id: Uuid) -> Result<UserQuotaRow, sqlx::Error> {
    // Insert default quota row if none exists (ON CONFLICT = no-op)
    sqlx::query(
        "INSERT INTO user_quotas (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user_id)
    .execute(pool)
    .await?;

    // Fetch the actual row (guaranteed to exist now)
    sqlx::query_as::<_, UserQuotaRow>(
        "SELECT user_id, max_storage_bytes, max_projects, max_concurrent_runs, \
         max_llm_tokens_per_day, max_upload_bytes, max_vt_lookups_per_day \
         FROM user_quotas WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// Update a specific quota field. `field` must be one of the column names.
pub async fn set_quota(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
    field: &str,
    value: i64,
) -> Result<(), sqlx::Error> {
    // Allowlist of valid column names to prevent SQL injection
    let valid_fields = [
        "max_storage_bytes",
        "max_projects",
        "max_concurrent_runs",
        "max_llm_tokens_per_day",
        "max_upload_bytes",
        "max_vt_lookups_per_day",
    ];
    if !valid_fields.contains(&field) {
        return Err(sqlx::Error::Protocol(format!("invalid quota field: {field}")));
    }

    let query = format!("UPDATE user_quotas SET {field} = $1 WHERE user_id = $2");
    sqlx::query(&query)
        .bind(value)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Record LLM token usage for today (upsert into usage_daily).
pub async fn record_llm_usage(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
    prompt_tokens: i64,
    completion_tokens: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_daily (user_id, date, llm_prompt_tokens, llm_completion_tokens) \
         VALUES ($1, CURRENT_DATE, $2, $3) \
         ON CONFLICT (user_id, date) DO UPDATE SET \
           llm_prompt_tokens = usage_daily.llm_prompt_tokens + EXCLUDED.llm_prompt_tokens, \
           llm_completion_tokens = usage_daily.llm_completion_tokens + EXCLUDED.llm_completion_tokens",
    )
    .bind(user_id)
    .bind(prompt_tokens)
    .bind(completion_tokens)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a tool run for today.
pub async fn record_tool_run(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_daily (user_id, date, tool_runs) \
         VALUES ($1, CURRENT_DATE, 1) \
         ON CONFLICT (user_id, date) DO UPDATE SET \
           tool_runs = usage_daily.tool_runs + 1",
    )
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a VT lookup for today.
pub async fn record_vt_lookup(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_daily (user_id, date, vt_lookups) \
         VALUES ($1, CURRENT_DATE, 1) \
         ON CONFLICT (user_id, date) DO UPDATE SET \
           vt_lookups = usage_daily.vt_lookups + 1",
    )
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Get today's usage for a user.
pub async fn get_daily_usage(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<Option<UsageDailyRow>, sqlx::Error> {
    sqlx::query_as::<_, UsageDailyRow>(
        "SELECT user_id, date, llm_prompt_tokens, llm_completion_tokens, vt_lookups, tool_runs \
         FROM usage_daily WHERE user_id = $1 AND date = CURRENT_DATE",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// Check whether the user is under their daily LLM token quota.
/// Returns true if under limit (or if no quota/usage row exists — permissive default).
/// Multi-query: keeps &PgPool.
pub async fn check_llm_quota(pool: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let quota = match get_quota(pool, user_id).await? {
        Some(q) => q,
        None => return Ok(true), // no quota row = unlimited
    };

    let usage = get_daily_usage(pool, user_id).await?;
    let total_tokens = usage
        .map(|u| u.llm_prompt_tokens + u.llm_completion_tokens)
        .unwrap_or(0);

    Ok(total_tokens < quota.max_llm_tokens_per_day)
}

/// Check whether the user has storage capacity for `additional_bytes`.
/// Returns true if under limit (or if no quota row exists).
/// Multi-query: keeps &PgPool.
pub async fn check_storage_quota(
    pool: &PgPool,
    user_id: Uuid,
    additional_bytes: i64,
) -> Result<bool, sqlx::Error> {
    let quota = match get_quota(pool, user_id).await? {
        Some(q) => q,
        None => return Ok(true),
    };

    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT storage_bytes_used FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let current = row.map(|r| r.0).unwrap_or(0);
    Ok(current + additional_bytes <= quota.max_storage_bytes)
}

/// Count active (queued or running) tool runs for a user.
pub async fn count_active_runs(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tool_runs \
         WHERE actor_user_id = $1 AND status IN ('queued', 'running')",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Atomically increment the user's storage_bytes_used by delta_bytes (can be negative).
pub async fn update_storage_used(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
    delta_bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET storage_bytes_used = storage_bytes_used + $1 WHERE id = $2",
    )
    .bind(delta_bytes)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Atomically reserve `delta_bytes` of storage for a user.
/// Returns true if reservation succeeded (user had enough quota).
/// Returns false if quota would be exceeded (no change made).
/// If user has no quota row, reservation is allowed (permissive default).
pub async fn reserve_storage_atomic(
    pool: &PgPool,
    user_id: Uuid,
    delta_bytes: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE users SET storage_bytes_used = storage_bytes_used + $1 \
         WHERE id = $2 AND (\
           storage_bytes_used + $1 <= COALESCE(\
             (SELECT max_storage_bytes FROM user_quotas WHERE user_id = $2), \
             9223372036854775807\
           )\
         )",
    )
    .bind(delta_bytes)
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Release previously reserved storage (e.g. if blob write fails).
pub async fn release_storage(
    pool: &PgPool,
    user_id: Uuid,
    delta_bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET storage_bytes_used = GREATEST(0, storage_bytes_used - $1) WHERE id = $2",
    )
    .bind(delta_bytes)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Atomically reserve LLM tokens for a user. Only succeeds if the user would
/// remain under their daily quota after the reservation.
/// Returns true if the reservation succeeded (tokens were added to usage_daily).
/// Returns false if the quota would be exceeded (no change made).
/// If the user has no quota row, reservation is allowed (permissive default).
pub async fn reserve_llm_tokens(
    pool: &PgPool,
    user_id: Uuid,
    estimated_tokens: i64,
) -> Result<bool, sqlx::Error> {
    // Ensure usage_daily row exists for today
    sqlx::query(
        "INSERT INTO usage_daily (user_id, date, llm_prompt_tokens, llm_completion_tokens) \
         VALUES ($1, CURRENT_DATE, 0, 0) \
         ON CONFLICT (user_id, date) DO NOTHING",
    )
    .bind(user_id)
    .execute(pool)
    .await?;

    // Atomic reserve: increment only if under quota
    let result = sqlx::query(
        "UPDATE usage_daily SET llm_prompt_tokens = llm_prompt_tokens + $2 \
         WHERE user_id = $1 AND date = CURRENT_DATE \
         AND (llm_prompt_tokens + llm_completion_tokens + $2) <= \
             COALESCE((SELECT max_llm_tokens_per_day FROM user_quotas WHERE user_id = $1), \
                      9223372036854775807)",
    )
    .bind(user_id)
    .bind(estimated_tokens)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Adjust LLM token usage after actual counts are known.
/// `prompt_delta` can be negative to release over-estimated reservation.
pub async fn adjust_llm_tokens(
    pool: &PgPool,
    user_id: Uuid,
    prompt_delta: i64,
    completion_tokens: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE usage_daily SET \
           llm_prompt_tokens = GREATEST(0, llm_prompt_tokens + $2), \
           llm_completion_tokens = llm_completion_tokens + $3 \
         WHERE user_id = $1 AND date = CURRENT_DATE",
    )
    .bind(user_id)
    .bind(prompt_delta)
    .bind(completion_tokens)
    .execute(pool)
    .await?;
    Ok(())
}
