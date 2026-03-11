use axum::extract::{Path, State};
use axum::Json;
use af_auth::Action;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{CreateHookRequest, HookResponse, UpdateHookRequest};
use crate::error::ApiError;
use crate::AppState;

/// POST /api/v1/projects/{id}/hooks
pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateHookRequest>,
) -> Result<Json<HookResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Auth: project write access
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        af_db::projects::get_project(&mut *tx, project_id)
            .await?
            .ok_or_else(|| ApiError::Forbidden("access denied".into()))?;
        require_project_access(&mut *tx, &user, project_id, Action::Write).await?;
        tx.commit().await?;
    }

    // Validate event_type
    if body.event_type != "artifact_uploaded" && body.event_type != "tick" {
        return Err(ApiError::BadRequest(
            "event_type must be 'artifact_uploaded' or 'tick'".into(),
        ));
    }

    // Validate exactly one target
    match (&body.workflow_name, &body.agent_name) {
        (Some(_), None) | (None, Some(_)) => {}
        _ => {
            return Err(ApiError::BadRequest(
                "exactly one of workflow_name or agent_name is required".into(),
            ));
        }
    }

    // Validate tick interval
    if body.event_type == "tick" {
        match body.tick_interval_minutes {
            Some(v) if v > 0 => {}
            _ => {
                return Err(ApiError::BadRequest(
                    "tick hooks require tick_interval_minutes > 0".into(),
                ));
            }
        }
    }

    // Validate name
    if body.name.is_empty() || body.name.len() > 200 {
        return Err(ApiError::BadRequest(
            "name must be 1-200 characters".into(),
        ));
    }

    // Validate referenced workflow/agent exists
    if let Some(ref wf_name) = body.workflow_name {
        af_db::workflows::get(&state.pool, wf_name)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!("workflow '{wf_name}' not found"))
            })?;
    }
    if let Some(ref ag_name) = body.agent_name {
        af_db::agents::get(&state.pool, ag_name)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!("agent '{ag_name}' not found"))
            })?;
    }

    let row = af_db::project_hooks::create(
        &state.pool,
        project_id,
        &body.name,
        &body.event_type,
        body.workflow_name.as_deref(),
        body.agent_name.as_deref(),
        &body.prompt_template,
        body.route_override.as_deref(),
        body.tick_interval_minutes,
    )
    .await?;

    // Audit
    let detail = serde_json::json!({
        "hook_id": row.id,
        "hook_name": &body.name,
        "event_type": &body.event_type,
        "project_id": project_id,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "hook_created", None, actor_uid, Some(&detail))
            .await;
    });

    Ok(Json(row.into()))
}

/// GET /api/v1/projects/{id}/hooks
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<HookResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        af_db::projects::get_project(&mut *tx, project_id)
            .await?
            .ok_or_else(|| ApiError::Forbidden("access denied".into()))?;
        require_project_access(&mut *tx, &user, project_id, Action::Read).await?;
        tx.commit().await?;
    }

    let rows = af_db::project_hooks::list_by_project(&state.pool, project_id).await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// GET /api/v1/hooks/{id}
pub async fn get_one(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(hook_id): Path<Uuid>,
) -> Result<Json<HookResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    let hook = af_db::project_hooks::get(&state.pool, hook_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("hook {hook_id} not found")))?;

    // Check project access
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        require_project_access(&mut *tx, &user, hook.project_id, Action::Read).await?;
        tx.commit().await?;
    }

    Ok(Json(hook.into()))
}

/// PUT /api/v1/hooks/{id}
pub async fn update(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(hook_id): Path<Uuid>,
    Json(body): Json<UpdateHookRequest>,
) -> Result<Json<HookResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    let hook = af_db::project_hooks::get(&state.pool, hook_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("hook {hook_id} not found")))?;

    // Check project write access
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        require_project_access(&mut *tx, &user, hook.project_id, Action::Write).await?;
        tx.commit().await?;
    }

    // Validate tick_interval_minutes if provided
    if let Some(v) = body.tick_interval_minutes {
        if v <= 0 {
            return Err(ApiError::BadRequest(
                "tick_interval_minutes must be > 0".into(),
            ));
        }
    }

    let route_override_ref = body
        .route_override
        .as_ref()
        .map(|opt| opt.as_deref());

    let updated = af_db::project_hooks::update(
        &state.pool,
        hook_id,
        body.enabled,
        body.prompt_template.as_deref(),
        route_override_ref,
        body.tick_interval_minutes,
    )
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("hook {hook_id} not found")))?;

    // Audit
    let detail = serde_json::json!({
        "hook_id": hook_id,
        "hook_name": &updated.name,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "hook_updated", None, actor_uid, Some(&detail))
            .await;
    });

    Ok(Json(updated.into()))
}

/// DELETE /api/v1/hooks/{id}
pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(hook_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    let hook = af_db::project_hooks::get(&state.pool, hook_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("hook {hook_id} not found")))?;

    // Check project write access
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        require_project_access(&mut *tx, &user, hook.project_id, Action::Write).await?;
        tx.commit().await?;
    }

    let deleted = af_db::project_hooks::delete(&state.pool, hook_id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!("hook {hook_id} not found")));
    }

    // Audit
    let detail = serde_json::json!({
        "hook_id": hook_id,
        "hook_name": &hook.name,
        "project_id": hook.project_id,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "hook_deleted", None, actor_uid, Some(&detail))
            .await;
    });

    Ok(Json(serde_json::json!({"deleted": true})))
}
