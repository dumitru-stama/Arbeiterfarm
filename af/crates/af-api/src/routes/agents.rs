use axum::extract::{Path, State};
use axum::Json;
use std::sync::Arc;

use crate::auth::AuthenticatedUser;
use crate::dto::{AgentResponse, CreateAgentRequest, UpdateAgentRequest};
use crate::error::ApiError;
use crate::AppState;

/// GET /api/v1/agents
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(_identity): AuthenticatedUser,
) -> Result<Json<Vec<AgentResponse>>, ApiError> {
    let rows = af_db::agents::list(&state.pool).await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// GET /api/v1/agents/:name
pub async fn get_one(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(_identity): AuthenticatedUser,
    Path(name): Path<String>,
) -> Result<Json<AgentResponse>, ApiError> {
    let row = af_db::agents::get(&state.pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("agent '{name}' not found")))?;
    Ok(Json(row.into()))
}

/// POST /api/v1/agents
pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<CreateAgentRequest>,
) -> Result<Json<AgentResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    // Reject if name conflicts with a builtin
    if let Ok(Some(existing)) = af_db::agents::get(&state.pool, &body.name).await {
        if existing.is_builtin {
            return Err(ApiError::BadRequest(format!(
                "cannot create agent with builtin name '{}'",
                body.name
            )));
        }
    }

    // Validate timeout_secs range (protects against u32→i32 overflow too)
    if let Some(t) = body.timeout_secs {
        if t == 0 || t > 86400 {
            return Err(ApiError::BadRequest(
                "timeout_secs must be between 1 and 86400".into(),
            ));
        }
    }

    let tools_json = serde_json::Value::Array(
        body.allowed_tools
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect(),
    );
    let metadata = if body.metadata.is_null() {
        serde_json::json!({})
    } else {
        body.metadata
    };

    let timeout = body.timeout_secs.map(|s| s as i32);
    let row = af_db::agents::upsert(
        &state.pool,
        &body.name,
        &body.system_prompt,
        &tools_json,
        &body.default_route,
        &metadata,
        false,
        Some("user"),
        timeout,
    )
    .await?;

    // Audit
    let detail = serde_json::json!({ "agent_name": &body.name });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "agent_created", None, actor_uid, Some(&detail)).await;
    });

    Ok(Json(row.into()))
}

/// PUT /api/v1/agents/:name
pub async fn update(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateAgentRequest>,
) -> Result<Json<AgentResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let existing = af_db::agents::get(&state.pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("agent '{name}' not found")))?;

    let system_prompt = body
        .system_prompt
        .as_deref()
        .unwrap_or(&existing.system_prompt);
    let allowed_tools = body
        .allowed_tools
        .as_ref()
        .map(|tools| {
            serde_json::Value::Array(
                tools
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            )
        })
        .unwrap_or(existing.allowed_tools);
    let default_route = body
        .default_route
        .as_deref()
        .unwrap_or(&existing.default_route);
    let metadata = body.metadata.as_ref().unwrap_or(&existing.metadata);

    // timeout_secs: absent = keep existing; 0 = clear; 1..=86400 = set
    let timeout = match body.timeout_secs {
        None => existing.timeout_secs,
        Some(0) => None,
        Some(t) if t > 86400 => {
            return Err(ApiError::BadRequest(
                "timeout_secs must be between 1 and 86400".into(),
            ));
        }
        Some(t) => Some(t as i32),
    };
    let row = af_db::agents::upsert(
        &state.pool,
        &name,
        system_prompt,
        &allowed_tools,
        default_route,
        metadata,
        existing.is_builtin,
        existing.source_plugin.as_deref(),
        timeout,
    )
    .await?;

    // Audit
    let detail = serde_json::json!({ "agent_name": &name });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "agent_updated", None, actor_uid, Some(&detail)).await;
    });

    Ok(Json(row.into()))
}

/// DELETE /api/v1/agents/:name
pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let deleted = af_db::agents::delete(&state.pool, &name).await?;
    if deleted {
        // Audit
        let detail = serde_json::json!({ "agent_name": &name });
        let actor_uid = user.user_id();
        let pool = state.pool.clone();
        tokio::spawn(async move {
            let _ = af_db::audit_log::insert(&pool, "agent_deleted", None, actor_uid, Some(&detail)).await;
        });
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        // Either doesn't exist or is builtin
        if let Ok(Some(row)) = af_db::agents::get(&state.pool, &name).await {
            if row.is_builtin {
                return Err(ApiError::BadRequest(format!(
                    "cannot delete builtin agent '{name}'"
                )));
            }
        }
        Err(ApiError::NotFound(format!("agent '{name}' not found")))
    }
}
