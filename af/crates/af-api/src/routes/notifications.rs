use axum::extract::{Path, Query, State};
use axum::Json;
use af_auth::Action;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{
    ChannelResponse, CreateChannelRequest, NotificationQueueResponse, UpdateChannelRequest,
};
use crate::error::ApiError;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
}

// ---------------------------------------------------------------------------
// Channel endpoints
// ---------------------------------------------------------------------------

/// POST /projects/{id}/notification-channels — create channel (Manager+)
pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateChannelRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
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

    // Validate input lengths
    if body.name.is_empty() || body.name.len() > 100 {
        return Err(ApiError::BadRequest("channel name must be 1-100 characters".into()));
    }

    // Validate channel type
    if !["webhook", "email", "matrix", "webdav"].contains(&body.channel_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "invalid channel type '{}' (must be webhook, email, matrix, or webdav)",
            body.channel_type
        )));
    }

    // Validate config by type
    validate_config(&body.channel_type, &body.config)?;

    let row = af_db::notifications::create_channel(
        &state.pool,
        project_id,
        &body.name,
        &body.channel_type,
        &body.config,
    )
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("duplicate key") || msg.contains("unique constraint") {
            ApiError::BadRequest(format!("channel name '{}' already exists in this project", body.name))
        } else {
            ApiError::Internal(format!("failed to create channel: {e}"))
        }
    })?;

    Ok(Json(ChannelResponse::from(row)))
}

/// GET /projects/{id}/notification-channels — list channels (Viewer+)
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ChannelResponse>>, ApiError> {
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

    let rows = af_db::notifications::list_channels(&state.pool, project_id).await?;
    Ok(Json(rows.into_iter().map(ChannelResponse::from).collect()))
}

/// PUT /projects/{id}/notification-channels/{ch_id} — update channel (Manager+)
pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path((project_id, ch_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateChannelRequest>,
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

    // Verify channel belongs to project and get current state for defaults
    let channel = af_db::notifications::get_channel(&state.pool, ch_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("channel not found".into()))?;
    if channel.project_id != project_id {
        return Err(ApiError::NotFound("channel not found".into()));
    }

    // Validate config
    validate_config(&channel.channel_type, &body.config)?;

    let enabled = body.enabled.unwrap_or(channel.enabled);
    // Use project-scoped update for defense-in-depth
    let updated = af_db::notifications::update_channel_for_project(
        &state.pool, ch_id, project_id, &body.config, enabled,
    )
    .await?;
    if updated {
        Ok(Json(serde_json::json!({"status": "updated"})))
    } else {
        Err(ApiError::NotFound("channel not found".into()))
    }
}

/// DELETE /projects/{id}/notification-channels/{ch_id} — delete channel (Manager+)
pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path((project_id, ch_id)): Path<(Uuid, Uuid)>,
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

    let deleted =
        af_db::notifications::delete_channel_for_project(&state.pool, ch_id, project_id)
            .await?;
    if deleted {
        Ok(Json(serde_json::json!({"status": "deleted"})))
    } else {
        Err(ApiError::NotFound("channel not found".into()))
    }
}

/// POST /projects/{id}/notification-channels/{ch_id}/test — test channel (Manager+)
pub async fn test_channel(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path((project_id, ch_id)): Path<(Uuid, Uuid)>,
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

    let channel = af_db::notifications::get_channel(&state.pool, ch_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("channel not found".into()))?;
    if channel.project_id != project_id {
        return Err(ApiError::NotFound("channel not found".into()));
    }

    let row = af_db::notifications::enqueue(
        &state.pool,
        project_id,
        ch_id,
        "Test notification from Arbeiterfarm",
        "This is a test notification to verify channel configuration.",
        None,
        user.user_id(),
    )
    .await?;

    Ok(Json(serde_json::json!({
        "status": "queued",
        "id": row.id.to_string(),
    })))
}

// ---------------------------------------------------------------------------
// Queue endpoints
// ---------------------------------------------------------------------------

/// GET /projects/{id}/notifications — list queue (Viewer+)
pub async fn list_queue(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<NotificationQueueResponse>>, ApiError> {
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

    let rows = af_db::notifications::list_queue(
        &state.pool,
        Some(project_id),
        query.status.as_deref(),
        100,
    )
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(NotificationQueueResponse::from)
            .collect(),
    ))
}

/// DELETE /projects/{id}/notifications/{queue_id} — cancel pending (Manager+)
pub async fn cancel_notification(
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

    let cancelled =
        af_db::notifications::cancel_for_project(&state.pool, queue_id, project_id).await?;
    if cancelled {
        Ok(Json(serde_json::json!({"status": "cancelled"})))
    } else {
        Err(ApiError::NotFound("item not found or not pending".into()))
    }
}

/// POST /projects/{id}/notifications/{queue_id}/retry — retry failed (Manager+)
pub async fn retry_notification(
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

    let retried =
        af_db::notifications::retry_for_project(&state.pool, queue_id, project_id).await?;
    if retried {
        Ok(Json(serde_json::json!({"status": "pending"})))
    } else {
        Err(ApiError::NotFound("item not found or not failed".into()))
    }
}

// ---------------------------------------------------------------------------
// Validation helper
// ---------------------------------------------------------------------------

fn validate_config(channel_type: &str, config: &serde_json::Value) -> Result<(), ApiError> {
    match channel_type {
        "webhook" => {
            let url = config["url"]
                .as_str()
                .ok_or_else(|| ApiError::BadRequest("webhook config requires 'url'".into()))?;
            if !url.starts_with("https://") {
                return Err(ApiError::BadRequest("webhook URL must use https://".into()));
            }
        }
        "email" => {
            let to = config["to"]
                .as_array()
                .ok_or_else(|| ApiError::BadRequest("email config requires 'to' array".into()))?;
            if to.is_empty() {
                return Err(ApiError::BadRequest("email 'to' must not be empty".into()));
            }
            if config["credential_id"].as_str().is_none() {
                return Err(ApiError::BadRequest(
                    "email config requires 'credential_id'".into(),
                ));
            }
        }
        "matrix" => {
            if config["homeserver"].as_str().is_none() {
                return Err(ApiError::BadRequest(
                    "matrix config requires 'homeserver'".into(),
                ));
            }
            if config["room_id"].as_str().is_none() {
                return Err(ApiError::BadRequest(
                    "matrix config requires 'room_id'".into(),
                ));
            }
            if config["access_token"].as_str().is_none() {
                return Err(ApiError::BadRequest(
                    "matrix config requires 'access_token'".into(),
                ));
            }
        }
        "webdav" => {
            let url = config["url"]
                .as_str()
                .ok_or_else(|| ApiError::BadRequest("webdav config requires 'url'".into()))?;
            if !url.starts_with("https://") {
                return Err(ApiError::BadRequest("webdav URL must use https://".into()));
            }
        }
        _ => {}
    }
    Ok(())
}
