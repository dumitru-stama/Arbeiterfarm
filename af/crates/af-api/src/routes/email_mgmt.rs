use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Query / request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListRulesQuery {
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct AddRuleRequest {
    pub scope: String,
    pub project_id: Option<Uuid>,
    pub rule_type: String,
    pub pattern_type: String,
    pub pattern: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertToneRequest {
    pub name: String,
    pub description: Option<String>,
    pub system_instruction: String,
}

#[derive(Debug, Deserialize)]
pub struct ListScheduledQuery {
    pub project_id: Option<Uuid>,
    pub status: Option<String>,
}

const VALID_SCOPES: &[&str] = &["global", "project"];
const VALID_RULE_TYPES: &[&str] = &["allow", "block"];
const VALID_PATTERN_TYPES: &[&str] = &["email", "domain", "domain_suffix"];

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// GET /api/v1/admin/email/credentials
pub async fn list_credentials(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Vec<af_db::email::EmailCredentialRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::email::list_all_credentials(&state.pool).await?;
    Ok(Json(rows))
}

/// DELETE /api/v1/admin/email/credentials/:id
pub async fn remove_credential(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let deleted = af_db::email::delete_credential(&state.pool, id).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("credential '{id}' not found")))
    }
}

// ---------------------------------------------------------------------------
// Recipient rules
// ---------------------------------------------------------------------------

/// GET /api/v1/admin/email/rules
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(params): Query<ListRulesQuery>,
) -> Result<Json<Vec<af_db::email::EmailRecipientRuleRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::email::list_recipient_rules(&state.pool, None, params.project_id).await?;
    Ok(Json(rows))
}

/// POST /api/v1/admin/email/rules
pub async fn add_rule(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<AddRuleRequest>,
) -> Result<Json<af_db::email::EmailRecipientRuleRow>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    if !VALID_SCOPES.contains(&body.scope.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "scope must be one of: {}",
            VALID_SCOPES.join(", ")
        )));
    }
    if !VALID_RULE_TYPES.contains(&body.rule_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "rule_type must be one of: {}",
            VALID_RULE_TYPES.join(", ")
        )));
    }
    if !VALID_PATTERN_TYPES.contains(&body.pattern_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "pattern_type must be one of: {}",
            VALID_PATTERN_TYPES.join(", ")
        )));
    }
    if body.scope == "project" && body.project_id.is_none() {
        return Err(ApiError::BadRequest(
            "project_id is required when scope is 'project'".into(),
        ));
    }
    if body.scope == "global" && body.project_id.is_some() {
        return Err(ApiError::BadRequest(
            "project_id must be null when scope is 'global'".into(),
        ));
    }
    if body.pattern.is_empty() {
        return Err(ApiError::BadRequest("pattern is required".into()));
    }

    let row = af_db::email::add_recipient_rule(
        &state.pool,
        &body.scope,
        body.project_id,
        &body.rule_type,
        &body.pattern_type,
        &body.pattern,
        body.description.as_deref(),
        user.user_id(),
    )
    .await?;
    Ok(Json(row))
}

/// DELETE /api/v1/admin/email/rules/:id
pub async fn remove_rule(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let deleted = af_db::email::remove_recipient_rule(&state.pool, id).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("rule '{id}' not found")))
    }
}

// ---------------------------------------------------------------------------
// Tone presets
// ---------------------------------------------------------------------------

/// GET /api/v1/admin/email/tones
pub async fn list_tones(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Vec<af_db::email::EmailTonePresetRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::email::list_tone_presets(&state.pool).await?;
    Ok(Json(rows))
}

/// POST /api/v1/admin/email/tones
pub async fn upsert_tone(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<UpsertToneRequest>,
) -> Result<Json<af_db::email::EmailTonePresetRow>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    if body.name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if body.system_instruction.is_empty() {
        return Err(ApiError::BadRequest("system_instruction is required".into()));
    }
    let row = af_db::email::upsert_tone_preset(
        &state.pool,
        &body.name,
        body.description.as_deref(),
        &body.system_instruction,
        false,
        user.user_id(),
    )
    .await?;
    Ok(Json(row))
}

/// DELETE /api/v1/admin/email/tones/:name
pub async fn remove_tone(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let deleted = af_db::email::delete_tone_preset(&state.pool, &name).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("tone preset '{name}' not found")))
    }
}

// ---------------------------------------------------------------------------
// Scheduled emails
// ---------------------------------------------------------------------------

/// GET /api/v1/admin/email/scheduled
pub async fn list_scheduled(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(params): Query<ListScheduledQuery>,
) -> Result<Json<Vec<af_db::email::EmailScheduledRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::email::list_scheduled(&state.pool, params.project_id, params.status.as_deref()).await?;
    Ok(Json(rows))
}

/// DELETE /api/v1/admin/email/scheduled/:id
pub async fn cancel_scheduled(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let cancelled = af_db::email::cancel_scheduled(&state.pool, id).await?;
    if cancelled {
        Ok(Json(serde_json::json!({"cancelled": true})))
    } else {
        Err(ApiError::NotFound(format!("scheduled email '{id}' not found or already processed")))
    }
}
