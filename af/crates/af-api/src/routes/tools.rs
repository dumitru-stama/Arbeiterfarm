use axum::extract::{Path, State};
use axum::Json;
use af_auth::Action;
use af_core::{ToolInvoker, ToolRequest};
use af_jobs::invoker::JobQueueInvoker;
use std::sync::Arc;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{RunToolRequest, ToolRunResponse, ToolSpecResponse};
use crate::error::ApiError;
use crate::AppState;

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(_identity): AuthenticatedUser,
) -> Result<Json<Vec<ToolSpecResponse>>, ApiError> {
    let mut names = state.specs.list();
    names.sort();

    let specs: Vec<ToolSpecResponse> = names
        .iter()
        .filter_map(|name| {
            state.specs.get_latest(name).map(|spec| ToolSpecResponse {
                name: spec.name.clone(),
                version: spec.version,
                description: spec.description.clone(),
                source: state.source_map.tools.get(&spec.name).cloned(),
            })
        })
        .collect();

    Ok(Json(specs))
}

pub async fn run(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(name): Path<String>,
    Json(body): Json<RunToolRequest>,
) -> Result<Json<ToolRunResponse>, ApiError> {
    // Verify tool exists
    state
        .specs
        .get_latest(&name)
        .ok_or_else(|| ApiError::NotFound(format!("tool '{name}' not found")))?;

    let user = AuthenticatedUser(identity);

    // Auth check in short-lived scoped tx
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        require_project_access(&mut *tx, &user, body.project_id, Action::Write).await?;
        tx.commit().await?;
    }

    let invoker = JobQueueInvoker::new(
        state.pool.clone(),
        state.core_config.clone(),
        state.specs.clone(),
        state.executors.clone(),
    );

    let request = ToolRequest {
        tool_name: name.clone(),
        input_json: body.input,
        project_id: body.project_id,
        thread_id: None,
        parent_message_id: None,
        actor_user_id: user.user_id(),
    };

    match ToolInvoker::invoke(&invoker, request).await {
        Ok(result) => Ok(Json(ToolRunResponse {
            output: result.output_json,
            produced_artifacts: result.produced_artifacts,
            status: "completed".to_string(),
        })),
        Err(err) => {
            if err.code == "enqueue_error" && err.message.contains("quota exceeded") {
                Err(ApiError::QuotaExceeded(err.message))
            } else {
                Err(ApiError::Internal(err.to_string()))
            }
        }
    }
}
