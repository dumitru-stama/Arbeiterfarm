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
pub struct AddRuleRequest {
    pub scope: String,
    pub project_id: Option<Uuid>,
    pub rule_type: String,
    pub pattern_type: String,
    pub pattern: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddCountryRequest {
    pub country_code: String,
    pub country_name: Option<String>,
}

/// Allowed values for validation.
const VALID_SCOPES: &[&str] = &["global", "project"];
const VALID_RULE_TYPES: &[&str] = &["allow", "block"];
const VALID_PATTERN_TYPES: &[&str] = &["domain", "domain_suffix", "url_prefix", "url_regex", "ip_cidr"];

/// Check if the user has access to a project (admin always passes).
async fn check_project_access_simple(
    pool: &sqlx::PgPool,
    user: &AuthenticatedUser,
    project_id: Uuid,
) -> Result<(), ApiError> {
    if user.is_admin() {
        return Ok(());
    }
    let user_id = user.user_id().ok_or_else(|| {
        ApiError::Forbidden("no user_id on identity".into())
    })?;
    let mut conn = pool.acquire().await.map_err(|_| {
        ApiError::Internal("internal error".into())
    })?;
    af_auth::authz::check_project_access(
        &mut conn,
        user_id,
        project_id,
        af_auth::authz::Action::Read,
    )
    .await
    .map_err(|_| ApiError::Forbidden("access denied".into()))
}

// --- URL Rules ---

/// GET /api/v1/web-rules
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(params): Query<ListRulesQuery>,
) -> Result<Json<Vec<af_db::web_fetch::WebFetchRuleRow>>, ApiError> {
    let user = AuthenticatedUser(identity);

    // If a project_id is specified, verify the user has access to that project
    if let Some(pid) = params.project_id {
        check_project_access_simple(&state.pool, &user, pid).await?;
    }

    let rows = af_db::web_fetch::list_rules(&state.pool, None, params.project_id).await?;
    Ok(Json(rows))
}

/// POST /api/v1/web-rules
pub async fn add_rule(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<AddRuleRequest>,
) -> Result<Json<af_db::web_fetch::WebFetchRuleRow>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Validate scope
    if !VALID_SCOPES.contains(&body.scope.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "scope must be one of: {}",
            VALID_SCOPES.join(", ")
        )));
    }

    // Validate rule_type
    if !VALID_RULE_TYPES.contains(&body.rule_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "rule_type must be one of: {}",
            VALID_RULE_TYPES.join(", ")
        )));
    }

    // Validate pattern_type
    if !VALID_PATTERN_TYPES.contains(&body.pattern_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "pattern_type must be one of: {}",
            VALID_PATTERN_TYPES.join(", ")
        )));
    }

    // Validate scope/project_id consistency
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

    // Authorization: global rules require admin, project rules require project access
    if body.scope == "global" && !user.is_admin() {
        return Err(ApiError::Forbidden("admin required for global rules".into()));
    }
    if body.scope == "project" {
        if let Some(pid) = body.project_id {
            check_project_access_simple(&state.pool, &user, pid).await?;
        }
    }

    if body.pattern.is_empty() {
        return Err(ApiError::BadRequest("pattern is required".into()));
    }

    // Validate url_regex patterns compile
    if body.pattern_type == "url_regex" {
        if let Err(e) = regex::Regex::new(&body.pattern) {
            return Err(ApiError::BadRequest(format!("invalid regex pattern: {e}")));
        }
    }

    // Validate ip_cidr patterns parse
    if body.pattern_type == "ip_cidr" && !body.pattern.contains('/') {
        return Err(ApiError::BadRequest(
            "ip_cidr pattern must be in CIDR notation (e.g. 10.0.0.0/8)".into(),
        ));
    }

    let row = af_db::web_fetch::add_rule(
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

/// DELETE /api/v1/web-rules/:id
pub async fn remove_rule(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let deleted = af_db::web_fetch::remove_rule(&state.pool, id).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("rule '{id}' not found")))
    }
}

// --- Country Blocks ---

/// GET /api/v1/web-rules/countries
pub async fn list_countries(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Vec<af_db::web_fetch::CountryBlockRow>>, ApiError> {
    let _ = identity;
    let rows = af_db::web_fetch::list_country_blocks(&state.pool).await?;
    Ok(Json(rows))
}

/// POST /api/v1/web-rules/countries
pub async fn add_country(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<AddCountryRequest>,
) -> Result<Json<af_db::web_fetch::CountryBlockRow>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    if body.country_code.len() != 2 || !body.country_code.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ApiError::BadRequest(
            "country_code must be a 2-letter ISO 3166-1 alpha-2 code (e.g. US, RU)".into(),
        ));
    }

    let row = af_db::web_fetch::add_country_block(
        &state.pool,
        &body.country_code.to_uppercase(),
        body.country_name.as_deref(),
        user.user_id(),
    )
    .await?;

    Ok(Json(row))
}

/// DELETE /api/v1/web-rules/countries/:code
pub async fn remove_country(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(code): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let deleted = af_db::web_fetch::remove_country_block(&state.pool, &code.to_uppercase()).await?;
    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(ApiError::NotFound(format!("country '{code}' not blocked")))
    }
}
