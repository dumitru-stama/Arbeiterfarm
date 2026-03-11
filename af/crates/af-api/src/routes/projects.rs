use axum::extract::{Path, State};
use axum::Json;
use af_auth::Action;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{CreateProjectRequest, ModelCostBreakdown, ProjectCostResponse, ProjectResponse};
use crate::error::ApiError;
use crate::AppState;

pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<CreateProjectRequest>,
) -> Result<Json<ProjectResponse>, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("project name is required".into()));
    }

    let user = AuthenticatedUser(identity);

    let row = if user.is_admin() {
        // Admin bypasses RLS — use plain transaction
        if let Some(uid) = user.user_id() {
            let mut tx = state.pool.begin().await?;
            let project =
                af_db::projects::create_project_with_owner(&mut *tx, &body.name, uid).await?;
            af_db::project_members::add_member(&mut *tx, project.id, uid, "owner").await?;
            tx.commit().await?;
            project
        } else {
            af_db::projects::create_project(&state.pool, &body.name).await?
        }
    } else if let Some(uid) = user.user_id() {
        let mut tx = af_db::scoped::begin_scoped(&state.pool, uid).await?;
        let project =
            af_db::projects::create_project_with_owner(&mut *tx, &body.name, uid).await?;
        af_db::project_members::add_member(&mut *tx, project.id, uid, "owner").await?;
        tx.commit().await?;
        project
    } else {
        return Err(ApiError::Forbidden("no user_id on identity".into()));
    };

    Ok(Json(row.into()))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Vec<ProjectResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

    let rows = if user.is_admin() {
        af_db::projects::list_projects(&state.pool).await?
    } else if let Some(uid) = user.user_id() {
        let mut tx = af_db::scoped::begin_scoped(&state.pool, uid).await?;
        let rows = af_db::projects::list_projects_for_user(&mut *tx, uid).await?;
        tx.commit().await?;
        rows
    } else {
        vec![]
    };

    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let row = af_db::projects::get_project(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?;

    require_project_access(&mut *tx, &user, id, Action::Read).await?;

    tx.commit().await?;
    Ok(Json(row.into()))
}

/// DELETE /api/v1/projects/:id
pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let project = af_db::projects::get_project(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?;

    // Only owner or admin can delete
    require_project_access(&mut *tx, &user, id, Action::ManageMembers).await?;

    af_db::projects::delete_project(&mut *tx, id).await?;

    let detail = serde_json::json!({
        "project_id": id.to_string(),
        "project_name": project.name,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "project_deleted", None, actor_uid, Some(&detail)).await;
    });

    tx.commit().await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// GET /api/v1/projects/:id/cost — aggregate LLM usage and cost for a project
pub async fn cost(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectCostResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    af_db::projects::get_project(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?;

    require_project_access(&mut *tx, &user, id, Action::Read).await?;
    tx.commit().await?;

    let rows = af_db::llm_usage_log::aggregate_by_project(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let resp = compute_project_cost(id, &rows);
    Ok(Json(resp))
}

/// PATCH /api/v1/projects/:id/settings — update project settings (JSONB merge)
pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<ProjectResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    af_db::projects::get_project(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?;

    require_project_access(&mut *tx, &user, id, Action::ManageMembers).await?;

    // Handle NDA flag separately (dedicated column, not JSONB)
    if let Some(nda_val) = body.get("nda") {
        if let Some(nda) = nda_val.as_bool() {
            af_db::projects::set_nda(&mut tx, id, nda, user.user_id()).await?;
        } else {
            return Err(ApiError::BadRequest("nda must be a boolean".into()));
        }
    }

    // Apply remaining settings as JSONB merge (strip "nda" key to avoid storing it in JSONB)
    let mut settings = body.clone();
    if let Some(obj) = settings.as_object_mut() {
        obj.remove("nda");
    }
    let row = if settings.as_object().map_or(false, |o| !o.is_empty()) {
        af_db::projects::update_settings(&mut *tx, id, &settings)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?
    } else {
        // Re-fetch if only NDA was changed
        af_db::projects::get_project(&mut *tx, id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?
    };

    tx.commit().await?;
    Ok(Json(row.into()))
}

/// GET /api/v1/projects/:id/settings — get project settings
pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let row = af_db::projects::get_project(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {id} not found")))?;

    require_project_access(&mut *tx, &user, id, Action::Read).await?;

    tx.commit().await?;
    Ok(Json(row.settings))
}

fn compute_project_cost(
    project_id: Uuid,
    rows: &[af_db::llm_usage_log::UsageByRouteRow],
) -> ProjectCostResponse {
    let mut breakdown = Vec::new();
    let mut total_prompt: i64 = 0;
    let mut total_completion: i64 = 0;
    let mut total_cached_read: i64 = 0;
    let mut total_cache_creation: i64 = 0;
    let mut total_cost: Option<f64> = Some(0.0);

    for row in rows {
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

        total_prompt += row.prompt_tokens;
        total_completion += row.completion_tokens;
        total_cached_read += row.cached_read_tokens;
        total_cache_creation += row.cache_creation_tokens;

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

    ProjectCostResponse {
        project_id,
        breakdown,
        total_prompt_tokens: total_prompt,
        total_completion_tokens: total_completion,
        total_cached_read_tokens: total_cached_read,
        total_cache_creation_tokens: total_cache_creation,
        total_cost_usd: total_cost,
    }
}
