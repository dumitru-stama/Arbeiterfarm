use axum::extract::{Path, Query, State};
use axum::Json;
use af_auth::Action;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{CreateThreadRequest, ThreadResponse};
use crate::error::ApiError;
use crate::AppState;

pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateThreadRequest>,
) -> Result<Json<ThreadResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Validate thread_type
    let thread_type = body.thread_type.as_str();
    if !["agent", "workflow", "thinking"].contains(&thread_type) {
        return Err(ApiError::BadRequest(format!(
            "invalid thread_type '{}': must be agent, workflow, or thinking",
            thread_type
        )));
    }

    // Verify agent exists (DB first, then compiled-in)
    af_agents::resolve_agent_config(&state.pool, &body.agent_name, &state.agent_configs)
        .await
        .ok_or_else(|| ApiError::BadRequest(format!("agent '{}' not found", body.agent_name)))?;

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    // Verify project exists
    af_db::projects::get_project(&mut *tx, project_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} not found")))?;

    require_project_access(&mut *tx, &user, project_id, Action::Write).await?;

    let row = af_db::threads::create_thread_full(
        &mut *tx,
        project_id,
        &body.agent_name,
        body.title.as_deref(),
        thread_type,
        body.target_artifact_id,
    )
    .await?;

    tx.commit().await?;
    Ok(Json(row.into()))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ThreadResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

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
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} not found")))?;

    require_project_access(&mut *tx, &user, project_id, Action::Read).await?;

    let rows = af_db::threads::list_threads(&mut *tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// GET /api/v1/threads/:id/children — list child threads
pub async fn children(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<Vec<ThreadResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let thread = af_db::threads::get_thread(&mut *tx, thread_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("access denied".into()))?;

    require_project_access(&mut *tx, &user, thread.project_id, Action::Read).await?;

    let rows = af_db::threads::list_child_threads(&mut *tx, thread_id).await?;
    tx.commit().await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// DELETE /api/v1/threads/:id
pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
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

    let thread = af_db::threads::get_thread(&mut *tx, thread_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} not found")))?;

    require_project_access(&mut *tx, &user, thread.project_id, Action::Write).await?;

    af_db::threads::delete_thread(&mut *tx, thread_id).await?;

    let detail = serde_json::json!({
        "thread_id": thread_id.to_string(),
        "project_id": thread.project_id.to_string(),
        "title": thread.title,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "thread_deleted", None, actor_uid, Some(&detail)).await;
    });

    tx.commit().await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
pub struct ExportQuery {
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_format() -> String {
    "markdown".to_string()
}

pub async fn export(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> Result<axum::response::Response, ApiError> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let user = AuthenticatedUser(identity);

    // Auth check + export in scoped tx (RLS enforced for all queries)
    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    // Fetch thread inside scoped tx to prevent entity enumeration
    let thread = af_db::threads::get_thread(&mut *tx, thread_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("access denied".into()))?;

    require_project_access(&mut *tx, &user, thread.project_id, Action::Read).await?;

    let format = match query.format.to_lowercase().as_str() {
        "json" => af_db::thread_export::ExportFormat::Json,
        _ => af_db::thread_export::ExportFormat::Markdown,
    };

    let is_json = matches!(format, af_db::thread_export::ExportFormat::Json);

    let output = af_db::thread_export::run_thread_export(&mut *tx, thread_id, format)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    tx.commit().await?;

    let content_type = if is_json {
        "application/json"
    } else {
        "text/markdown; charset=utf-8"
    };

    Ok(([(header::CONTENT_TYPE, content_type)], output).into_response())
}
