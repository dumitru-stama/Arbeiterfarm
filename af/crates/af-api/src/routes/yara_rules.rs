use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListRulesQuery {
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ListScanResultsQuery {
    pub artifact_id: Option<Uuid>,
    pub rule_name: Option<String>,
}

/// GET /api/v1/admin/yara/rules
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(params): Query<ListRulesQuery>,
) -> Result<Json<Vec<af_db::yara::YaraRuleRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::yara::list_rules(&state.pool, params.project_id).await?;
    Ok(Json(rows))
}

/// GET /api/v1/admin/yara/rules/:id
pub async fn get_rule(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<af_db::yara::YaraRuleRow>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let row = af_db::yara::get_rule(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("yara rule '{id}' not found")))?;
    Ok(Json(row))
}

/// DELETE /api/v1/admin/yara/rules/:id
pub async fn remove_rule(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let deleted = af_db::yara::delete_rule(&state.pool, id).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("yara rule '{id}' not found")))
    }
}

/// GET /api/v1/admin/yara/scan-results
pub async fn list_scan_results(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(params): Query<ListScanResultsQuery>,
) -> Result<Json<Vec<af_db::yara::YaraScanResultRow>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }
    let rows = af_db::yara::list_scan_results(
        &state.pool,
        params.artifact_id,
        params.rule_name.as_deref(),
    )
    .await?;
    Ok(Json(rows))
}
