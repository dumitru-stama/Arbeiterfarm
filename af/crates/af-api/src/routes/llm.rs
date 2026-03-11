use axum::extract::State;
use axum::Json;
use std::sync::Arc;

use crate::auth::AuthenticatedUser;
use crate::dto::{LlmBackendResponse, LlmBackendsResponse};
use crate::error::ApiError;
use crate::AppState;

/// Check whether a route string matches any of the user's allowed route patterns.
fn route_matches_any(route_str: &str, allowed: &[af_db::user_allowed_routes::UserAllowedRouteRow]) -> bool {
    for row in allowed {
        if row.route == route_str {
            return true;
        }
        if let Some(prefix) = row.route.strip_suffix('*') {
            if route_str.starts_with(prefix) {
                return true;
            }
        }
    }
    false
}

/// GET /api/v1/llm/backends
pub async fn list_backends(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
) -> Result<Json<LlmBackendsResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    let backends = state
        .router
        .list_backends()
        .into_iter()
        .map(|b| LlmBackendResponse {
            name: b.name,
            supports_tool_calls: b.capabilities.supports_tool_calls,
            supports_streaming: b.capabilities.supports_streaming,
            is_local: b.capabilities.is_local,
            context_window: b.capabilities.context_window,
            max_output_tokens: b.capabilities.max_output_tokens,
            cost_per_mtok_input: b.capabilities.cost_per_mtok_input,
            cost_per_mtok_output: b.capabilities.cost_per_mtok_output,
            supports_vision: b.capabilities.supports_vision,
            knowledge_cutoff: b.capabilities.knowledge_cutoff,
        })
        .collect::<Vec<_>>();

    let mut routes = vec!["auto".to_string()];
    if backends.iter().any(|b| b.is_local) {
        routes.push("local".to_string());
    }
    for backend in &backends {
        routes.push(format!("backend:{}", backend.name));
    }

    // Filter by user's allowed routes (if restricted)
    if let Some(uid) = user.user_id() {
        let allowed = af_db::user_allowed_routes::list_routes(&state.pool, uid)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if !allowed.is_empty() {
            let filtered_backends: Vec<LlmBackendResponse> = backends
                .into_iter()
                .filter(|b| route_matches_any(&b.name, &allowed))
                .collect();
            let filtered_routes: Vec<String> = routes
                .into_iter()
                .filter(|r| route_matches_any(r, &allowed))
                .collect();
            return Ok(Json(LlmBackendsResponse {
                backends: filtered_backends,
                routes: filtered_routes,
            }));
        }
    }

    Ok(Json(LlmBackendsResponse { backends, routes }))
}
