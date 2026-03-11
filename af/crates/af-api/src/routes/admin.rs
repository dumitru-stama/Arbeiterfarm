use axum::extract::{Path, State};
use axum::Json;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::dto::{
    AddRouteRequest, ApiKeyResponse, CreateApiKeyRequest, CreateApiKeyResponse, CreateUserRequest,
    RemoveRouteRequest, UserResponse, UserRoutesResponse,
};
use serde::Deserialize;
use crate::error::ApiError;
use crate::AppState;

// --- Users ---

/// GET /api/v1/admin/users
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Vec<UserResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let rows = af_db::users::list_users(&state.pool).await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// POST /api/v1/admin/users
pub async fn create_user(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<CreateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    if body.subject.is_empty() {
        return Err(ApiError::BadRequest("subject is required".into()));
    }

    let row = af_db::users::create_user(
        &state.pool,
        &body.subject,
        body.display_name.as_deref(),
        body.email.as_deref(),
        &body.roles,
    )
    .await?;
    Ok(Json(row.into()))
}

/// GET /api/v1/admin/users/:id
pub async fn get_user(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let row = af_db::users::get_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user '{id}' not found")))?;
    Ok(Json(row.into()))
}

// --- API Keys ---

/// POST /api/v1/admin/users/:id/api_keys
pub async fn create_key(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    if body.name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }

    // Verify user exists
    af_db::users::get_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user '{user_id}' not found")))?;

    let (raw_key, key_hash, key_prefix) = af_auth::generate_key();

    let row = af_db::api_keys::create_key(
        &state.pool,
        user_id,
        &key_hash,
        &key_prefix,
        &body.name,
        &[],
    )
    .await?;

    Ok(Json(CreateApiKeyResponse {
        id: row.id,
        raw_key,
        key_prefix: row.key_prefix,
        name: row.name,
        created_at: row.created_at,
    }))
}

/// GET /api/v1/admin/users/:id/api_keys
pub async fn list_keys(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<ApiKeyResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let rows = af_db::api_keys::list_for_user(&state.pool, user_id).await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// DELETE /api/v1/admin/api_keys/:id
pub async fn revoke_key(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(key_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let deleted = af_db::api_keys::delete(&state.pool, key_id).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("api key '{key_id}' not found")))
    }
}

// --- User Allowed Routes ---

/// GET /api/v1/admin/users/:id/routes
pub async fn list_user_routes(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
) -> Result<Json<UserRoutesResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let rows = af_db::user_allowed_routes::list_routes(&state.pool, user_id).await?;
    let routes: Vec<String> = rows.iter().map(|r| r.route.clone()).collect();
    let unrestricted = routes.is_empty();
    Ok(Json(UserRoutesResponse { routes, unrestricted }))
}

/// POST /api/v1/admin/users/:id/routes
pub async fn add_user_route(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<AddRouteRequest>,
) -> Result<Json<UserRoutesResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    if body.route.is_empty() {
        return Err(ApiError::BadRequest("route is required".into()));
    }

    // Verify user exists
    af_db::users::get_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user '{user_id}' not found")))?;

    af_db::user_allowed_routes::add_route(&state.pool, user_id, &body.route).await?;

    // Return updated list
    let rows = af_db::user_allowed_routes::list_routes(&state.pool, user_id).await?;
    let routes: Vec<String> = rows.iter().map(|r| r.route.clone()).collect();
    let unrestricted = routes.is_empty();
    Ok(Json(UserRoutesResponse { routes, unrestricted }))
}

/// DELETE /api/v1/admin/users/:id/routes
pub async fn remove_user_route(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<RemoveRouteRequest>,
) -> Result<Json<UserRoutesResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    if body.clear {
        af_db::user_allowed_routes::remove_all_routes(&state.pool, user_id).await?;
    } else if let Some(ref route) = body.route {
        af_db::user_allowed_routes::remove_route(&state.pool, user_id, route).await?;
    } else {
        return Err(ApiError::BadRequest(
            "provide 'route' to remove a specific route or 'clear: true' to remove all".into(),
        ));
    }

    // Return updated list
    let rows = af_db::user_allowed_routes::list_routes(&state.pool, user_id).await?;
    let routes: Vec<String> = rows.iter().map(|r| r.route.clone()).collect();
    let unrestricted = routes.is_empty();
    Ok(Json(UserRoutesResponse { routes, unrestricted }))
}

// --- Restricted Tools ---

#[derive(Deserialize)]
pub struct AddRestrictedToolRequest {
    pub tool_pattern: String,
    pub description: String,
}

#[derive(Deserialize)]
pub struct RemoveRestrictedToolRequest {
    pub tool_pattern: String,
}

/// GET /api/v1/admin/restricted-tools
pub async fn list_restricted_tools(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Vec<af_db::restricted_tools::RestrictedToolRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::restricted_tools::list_restricted(&state.pool).await?;
    Ok(Json(rows))
}

/// POST /api/v1/admin/restricted-tools
pub async fn add_restricted_tool(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<AddRestrictedToolRequest>,
) -> Result<Json<af_db::restricted_tools::RestrictedToolRow>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    if body.tool_pattern.is_empty() {
        return Err(ApiError::BadRequest("tool_pattern is required".into()));
    }
    let row =
        af_db::restricted_tools::add_restricted(&state.pool, &body.tool_pattern, &body.description)
            .await?;
    Ok(Json(row))
}

/// DELETE /api/v1/admin/restricted-tools
pub async fn remove_restricted_tool(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<RemoveRestrictedToolRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let deleted =
        af_db::restricted_tools::remove_restricted(&state.pool, &body.tool_pattern).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!(
            "restricted pattern '{}' not found",
            body.tool_pattern
        )))
    }
}

// --- User Tool Grants ---

#[derive(Deserialize)]
pub struct ToolGrantRequest {
    pub tool_pattern: String,
}

/// GET /api/v1/admin/users/:id/tool-grants
pub async fn list_user_grants(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<af_db::restricted_tools::UserToolGrantRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::restricted_tools::list_user_grants(&state.pool, user_id).await?;
    Ok(Json(rows))
}

/// POST /api/v1/admin/users/:id/tool-grants
pub async fn add_user_grant(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<ToolGrantRequest>,
) -> Result<Json<af_db::restricted_tools::UserToolGrantRow>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    if body.tool_pattern.is_empty() {
        return Err(ApiError::BadRequest("tool_pattern is required".into()));
    }
    af_db::users::get_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user '{user_id}' not found")))?;
    let row =
        af_db::restricted_tools::add_user_grant(&state.pool, user_id, &body.tool_pattern)
            .await?;
    Ok(Json(row))
}

/// DELETE /api/v1/admin/users/:id/tool-grants
pub async fn remove_user_grant(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<ToolGrantRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let deleted =
        af_db::restricted_tools::remove_user_grant(&state.pool, user_id, &body.tool_pattern)
            .await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!(
            "grant '{}' for user '{}' not found",
            body.tool_pattern, user_id
        )))
    }
}
