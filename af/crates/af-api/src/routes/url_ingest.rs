use axum::extract::{Path, Query, State};
use axum::Json;
use af_auth::Action;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{SubmitUrlsRequest, UrlIngestResponse};
use crate::error::ApiError;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
}

/// POST /projects/{id}/url-ingest — submit URLs for ingestion (Manager+)
pub async fn submit(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Json(body): Json<SubmitUrlsRequest>,
) -> Result<Json<Vec<UrlIngestResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Auth check
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

    // Validate
    if body.urls.is_empty() {
        return Err(ApiError::BadRequest("no URLs provided".into()));
    }
    if body.urls.len() > 50 {
        return Err(ApiError::BadRequest("max 50 URLs per request".into()));
    }
    for url in &body.urls {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ApiError::BadRequest(format!(
                "invalid URL (must start with http:// or https://): {url}"
            )));
        }
        if url.len() > 2048 {
            return Err(ApiError::BadRequest("URL too long (max 2048 chars)".into()));
        }
    }

    let submitted_by = user.user_id();
    let rows = af_db::url_ingest::enqueue_urls(
        &state.pool,
        project_id,
        &body.urls,
        submitted_by,
    )
    .await?;

    Ok(Json(rows.into_iter().map(UrlIngestResponse::from).collect()))
}

/// GET /projects/{id}/url-ingest — list queue items (Viewer+)
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<UrlIngestResponse>>, ApiError> {
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

    let rows = af_db::url_ingest::list_queue(
        &state.pool,
        Some(project_id),
        query.status.as_deref(),
        100,
    )
    .await?;

    Ok(Json(rows.into_iter().map(UrlIngestResponse::from).collect()))
}

/// DELETE /projects/{id}/url-ingest/{queue_id} — cancel pending item (Manager+)
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path((project_id, queue_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
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
        require_project_access(&mut *tx, &user, project_id, Action::Write).await?;
        tx.commit().await?;
    }

    let cancelled = af_db::url_ingest::cancel_for_project(&state.pool, queue_id, project_id).await?;
    if cancelled {
        Ok(Json(serde_json::json!({"status": "cancelled"})))
    } else {
        Err(ApiError::NotFound("item not found or not pending".into()))
    }
}

/// POST /projects/{id}/url-ingest/{queue_id}/retry — retry failed item (Manager+)
pub async fn retry(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path((project_id, queue_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
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
        require_project_access(&mut *tx, &user, project_id, Action::Write).await?;
        tx.commit().await?;
    }

    let retried = af_db::url_ingest::retry_for_project(&state.pool, queue_id, project_id).await?;
    if retried {
        Ok(Json(serde_json::json!({"status": "pending"})))
    } else {
        Err(ApiError::NotFound("item not found or not failed".into()))
    }
}
