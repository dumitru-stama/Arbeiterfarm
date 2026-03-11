use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// GET /api/v1/health/security — admin-only security posture endpoint.
pub async fn security(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    Ok(Json(json!({
        "sandbox_available": state.security_config.sandbox_available,
        "sandbox_enforced": state.security_config.sandbox_enforced,
        "tls_enabled": state.security_config.tls_enabled,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}
