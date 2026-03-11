use axum::extract::{Path, State};
use axum::Json;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::dto::{ModelCostBreakdown, MonthlyCostResponse, QuotaResponse, QuotaUsageResponse, UpdateQuotaRequest, UsageResponse};
use crate::error::ApiError;
use crate::AppState;

/// GET /api/v1/quota — get own quota + today's usage
pub async fn get_own(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<QuotaUsageResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    let uid = user.user_id().ok_or_else(|| {
        ApiError::Forbidden("no user_id on identity".into())
    })?;

    let quota = af_db::user_quotas::ensure_quota(&state.pool, uid)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let usage = af_db::user_quotas::get_daily_usage(&state.pool, uid)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let allowed_routes_rows = af_db::user_allowed_routes::list_routes(&state.pool, uid)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let allowed_routes: Vec<String> = allowed_routes_rows.iter().map(|r| r.route.clone()).collect();

    let usage_resp = match usage {
        Some(u) => UsageResponse {
            llm_prompt_tokens: u.llm_prompt_tokens,
            llm_completion_tokens: u.llm_completion_tokens,
            vt_lookups: u.vt_lookups,
            tool_runs: u.tool_runs,
        },
        None => UsageResponse {
            llm_prompt_tokens: 0,
            llm_completion_tokens: 0,
            vt_lookups: 0,
            tool_runs: 0,
        },
    };

    Ok(Json(QuotaUsageResponse {
        quota: quota.into(),
        usage: usage_resp,
        allowed_routes,
    }))
}

/// GET /api/v1/admin/quota/:user_id — admin: get quota for any user
pub async fn admin_get(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
) -> Result<Json<QuotaUsageResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let quota = af_db::user_quotas::ensure_quota(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let usage = af_db::user_quotas::get_daily_usage(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let allowed_routes_rows = af_db::user_allowed_routes::list_routes(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let allowed_routes: Vec<String> = allowed_routes_rows.iter().map(|r| r.route.clone()).collect();

    let usage_resp = match usage {
        Some(u) => UsageResponse {
            llm_prompt_tokens: u.llm_prompt_tokens,
            llm_completion_tokens: u.llm_completion_tokens,
            vt_lookups: u.vt_lookups,
            tool_runs: u.tool_runs,
        },
        None => UsageResponse {
            llm_prompt_tokens: 0,
            llm_completion_tokens: 0,
            vt_lookups: 0,
            tool_runs: 0,
        },
    };

    Ok(Json(QuotaUsageResponse {
        quota: quota.into(),
        usage: usage_resp,
        allowed_routes,
    }))
}

/// PUT /api/v1/admin/quota/:user_id — admin: update quota fields
pub async fn admin_update(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UpdateQuotaRequest>,
) -> Result<Json<QuotaResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    // Ensure quota row exists first
    af_db::user_quotas::ensure_quota(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Update each provided field
    if let Some(v) = body.max_storage_bytes {
        af_db::user_quotas::set_quota(&state.pool, user_id, "max_storage_bytes", v)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if let Some(v) = body.max_projects {
        af_db::user_quotas::set_quota(&state.pool, user_id, "max_projects", v as i64)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if let Some(v) = body.max_concurrent_runs {
        af_db::user_quotas::set_quota(&state.pool, user_id, "max_concurrent_runs", v as i64)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if let Some(v) = body.max_llm_tokens_per_day {
        af_db::user_quotas::set_quota(&state.pool, user_id, "max_llm_tokens_per_day", v)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if let Some(v) = body.max_upload_bytes {
        af_db::user_quotas::set_quota(&state.pool, user_id, "max_upload_bytes", v)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if let Some(v) = body.max_vt_lookups_per_day {
        af_db::user_quotas::set_quota(&state.pool, user_id, "max_vt_lookups_per_day", v as i64)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    // Return updated quota
    let updated = af_db::user_quotas::get_quota(&state.pool, user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::Internal("quota row missing after update".into()))?;

    Ok(Json(updated.into()))
}

/// GET /api/v1/cost/monthly — current calendar month spend for the authenticated user
pub async fn monthly_cost(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<MonthlyCostResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    let uid = user.user_id().ok_or_else(|| {
        ApiError::Forbidden("no user_id on identity".into())
    })?;

    let now = chrono::Utc::now();
    let year = now.format("%Y").to_string().parse::<i32>().unwrap_or(2026);
    let month = now.format("%m").to_string().parse::<u32>().unwrap_or(1);

    let rows = af_db::llm_usage_log::aggregate_by_user_month(&state.pool, uid, year, month)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut breakdown = Vec::new();
    let mut total_cost: Option<f64> = Some(0.0);

    for row in &rows {
        let model = row.route.rsplit_once(':').map(|(_, m)| m).unwrap_or(&row.route);
        let cost = af_llm::model_catalog::compute_cost(
            &row.route,
            row.prompt_tokens as u32,
            row.completion_tokens as u32,
            row.cached_read_tokens as u32,
            row.cache_creation_tokens as u32,
        );

        if cost.is_none() {
            total_cost = None;
        } else if let (Some(c), Some(ref mut t)) = (cost, &mut total_cost) {
            *t += c;
        }

        breakdown.push(ModelCostBreakdown {
            route: row.route.clone(),
            model: model.to_string(),
            call_count: row.call_count,
            prompt_tokens: row.prompt_tokens,
            completion_tokens: row.completion_tokens,
            cached_read_tokens: row.cached_read_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cost_usd: cost,
        });
    }

    Ok(Json(MonthlyCostResponse {
        year,
        month,
        breakdown,
        total_cost_usd: total_cost,
    }))
}
