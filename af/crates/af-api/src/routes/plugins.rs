use axum::extract::State;
use axum::Json;
use std::sync::Arc;

use crate::auth::AuthenticatedUser;
use crate::dto::PluginResponse;
use crate::error::ApiError;
use crate::AppState;

/// GET /api/v1/plugins — list loaded plugins and their tools/agents/workflows.
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(_identity): AuthenticatedUser,
) -> Result<Json<Vec<PluginResponse>>, ApiError> {
    let by_source = state.source_map.by_source();
    let mut plugins: Vec<PluginResponse> = by_source
        .into_iter()
        .map(|(name, inv)| PluginResponse {
            name,
            tools: inv.tools,
            agents: inv.agents,
            workflows: inv.workflows,
        })
        .collect();
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(plugins))
}
