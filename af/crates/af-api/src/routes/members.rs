use axum::extract::{Path, State};
use axum::Json;
use af_auth::Action;
use af_db::project_members::ALL_USERS_SENTINEL;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{AddMemberRequest, ProjectMemberResponse};
use crate::error::ApiError;
use crate::AppState;

/// Resolve "@all" to sentinel UUID, otherwise parse as UUID.
fn resolve_user_id(s: &str) -> Result<Uuid, ApiError> {
    if s == "@all" {
        Ok(ALL_USERS_SENTINEL)
    } else {
        s.parse::<Uuid>()
            .map_err(|_| ApiError::BadRequest(format!("invalid user_id: {s}")))
    }
}

/// GET /projects/{id}/members
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ProjectMemberResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    require_project_access(&mut *tx, &user, project_id, Action::Read).await?;

    let rows =
        af_db::project_members::list_members_with_names(&mut *tx, project_id).await?;

    tx.commit().await?;

    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// POST /projects/{id}/members
pub async fn add(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Validate role
    let role = body.role.as_str();
    if !matches!(role, "manager" | "collaborator" | "viewer") {
        return Err(ApiError::BadRequest(
            "role must be one of: manager, collaborator, viewer".into(),
        ));
    }

    let target_uid = resolve_user_id(&body.user_id)?;

    // Cannot assign yourself
    if let Some(uid) = user.user_id() {
        if uid == target_uid {
            return Err(ApiError::BadRequest("cannot add yourself as a member".into()));
        }
    }

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    require_project_access(&mut *tx, &user, project_id, Action::ManageMembers).await?;

    // Verify the target user exists (unless sentinel)
    if target_uid != ALL_USERS_SENTINEL {
        let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
            .bind(target_uid)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(ApiError::NotFound(format!("user {target_uid} not found")));
        }
    }

    af_db::project_members::add_member(&mut *tx, project_id, target_uid, role).await?;

    tx.commit().await?;

    // Audit
    let detail = serde_json::json!({
        "project_id": project_id,
        "target_user_id": target_uid,
        "role": role,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "member_added", None, actor_uid, Some(&detail)).await;
    });

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /projects/{id}/members/{user_id}
pub async fn remove(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path((project_id, member_user_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    let target_uid = resolve_user_id(&member_user_id)?;

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    require_project_access(&mut *tx, &user, project_id, Action::ManageMembers).await?;

    // Prevent removing the project owner
    let owner_row: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT owner_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?;

    if let Some((Some(owner_id),)) = owner_row {
        if owner_id == target_uid {
            return Err(ApiError::BadRequest("cannot remove the project owner".into()));
        }
    }

    af_db::project_members::remove_member(&mut *tx, project_id, target_uid).await?;

    tx.commit().await?;

    // Audit
    let detail = serde_json::json!({
        "project_id": project_id,
        "target_user_id": target_uid,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "member_removed", None, actor_uid, Some(&detail)).await;
    });

    Ok(Json(serde_json::json!({ "ok": true })))
}
