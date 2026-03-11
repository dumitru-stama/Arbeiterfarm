use axum::extract::{Path, Query, State};
use axum::Json;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub project_id: Option<Uuid>,
    pub status: Option<String>,
}

/// GET /admin/embed-queue — list embed queue items (admin only)
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<af_db::embed_queue::EmbedQueueRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin only".into()));
    }

    let rows = af_db::embed_queue::list_queue(
        &state.pool,
        query.project_id,
        query.status.as_deref(),
        100,
    )
    .await?;

    Ok(Json(rows))
}

/// DELETE /admin/embed-queue/{id} — cancel a pending item (admin only)
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin only".into()));
    }

    let cancelled = af_db::embed_queue::cancel(&state.pool, id).await?;
    if cancelled {
        Ok(Json(serde_json::json!({"status": "cancelled"})))
    } else {
        Err(ApiError::NotFound("item not found or not pending".into()))
    }
}

/// POST /admin/embed-queue/{id}/retry — retry a failed item (admin only)
pub async fn retry(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin only".into()));
    }

    let retried = af_db::embed_queue::retry(&state.pool, id).await?;
    if retried {
        Ok(Json(serde_json::json!({"status": "pending"})))
    } else {
        Err(ApiError::NotFound("item not found or not failed".into()))
    }
}
