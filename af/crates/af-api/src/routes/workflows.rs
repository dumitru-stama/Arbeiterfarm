use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use af_agents::OrchestratorRuntime;
use af_auth::Action;
use af_core::{LlmRoute, OrchestratorEvent};
use af_jobs::invoker::JobQueueInvoker;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{
    CreateWorkflowRequest, ExecuteWorkflowRequest, UpdateWorkflowRequest, WorkflowResponse,
};
use crate::error::ApiError;
use crate::AppState;

/// GET /api/v1/workflows
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(_identity): AuthenticatedUser,
) -> Result<Json<Vec<WorkflowResponse>>, ApiError> {
    let rows = af_db::workflows::list(&state.pool).await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

/// GET /api/v1/workflows/:name
pub async fn get_one(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(_identity): AuthenticatedUser,
    Path(name): Path<String>,
) -> Result<Json<WorkflowResponse>, ApiError> {
    let row = af_db::workflows::get(&state.pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("workflow '{name}' not found")))?;
    Ok(Json(row.into()))
}

/// POST /api/v1/workflows
pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Json(body): Json<CreateWorkflowRequest>,
) -> Result<Json<WorkflowResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    // Reject if name conflicts with a builtin
    if let Ok(Some(existing)) = af_db::workflows::get(&state.pool, &body.name).await {
        if existing.is_builtin {
            return Err(ApiError::BadRequest(format!(
                "cannot create workflow with builtin name '{}'",
                body.name
            )));
        }
    }

    let row = af_db::workflows::upsert(
        &state.pool,
        &body.name,
        body.description.as_deref(),
        &body.steps,
        false,
        Some("user"),
    )
    .await?;

    // Audit
    let detail = serde_json::json!({ "workflow_name": &body.name });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "workflow_created", None, actor_uid, Some(&detail)).await;
    });

    Ok(Json(row.into()))
}

/// PUT /api/v1/workflows/:name
pub async fn update(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateWorkflowRequest>,
) -> Result<Json<WorkflowResponse>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let existing = af_db::workflows::get(&state.pool, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("workflow '{name}' not found")))?;

    let description = body.description.as_deref().or(existing.description.as_deref());
    let steps = body.steps.as_ref().unwrap_or(&existing.steps);

    let row = af_db::workflows::upsert(
        &state.pool,
        &name,
        description,
        steps,
        existing.is_builtin,
        existing.source_plugin.as_deref(),
    )
    .await?;

    // Audit
    let detail = serde_json::json!({ "workflow_name": &name });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "workflow_updated", None, actor_uid, Some(&detail)).await;
    });

    Ok(Json(row.into()))
}

/// DELETE /api/v1/workflows/:name
pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin required".into()));
    }

    let deleted = af_db::workflows::delete(&state.pool, &name).await?;
    if deleted {
        // Audit
        let detail = serde_json::json!({ "workflow_name": &name });
        let actor_uid = user.user_id();
        let pool = state.pool.clone();
        tokio::spawn(async move {
            let _ = af_db::audit_log::insert(&pool, "workflow_deleted", None, actor_uid, Some(&detail)).await;
        });
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        if let Ok(Some(row)) = af_db::workflows::get(&state.pool, &name).await {
            if row.is_builtin {
                return Err(ApiError::BadRequest(format!(
                    "cannot delete builtin workflow '{name}'"
                )));
            }
        }
        Err(ApiError::NotFound(format!("workflow '{name}' not found")))
    }
}

/// POST /api/v1/threads/:id/workflow — execute workflow (SSE streaming)
pub async fn execute(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<ExecuteWorkflowRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Auth check in scoped tx (entity lookup inside tx to prevent enumeration)
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        let thread = af_db::threads::get_thread(&mut *tx, thread_id)
            .await?
            .ok_or_else(|| ApiError::Forbidden("access denied".into()))?;
        require_project_access(&mut *tx, &user, thread.project_id, Action::Write).await?;
        tx.commit().await?;
    };

    // SSE stream limit per user
    let stream_user_id = user.user_id().unwrap_or(uuid::Uuid::nil());
    let stream_guard = state
        .stream_tracker
        .acquire(stream_user_id)
        .await
        .map_err(|_| ApiError::QuotaExceeded("concurrent stream limit reached".into()))?;

    let workflow = af_db::workflows::get(&state.pool, &body.workflow_name)
        .await?
        .ok_or_else(|| {
            ApiError::BadRequest(format!("workflow '{}' not found", body.workflow_name))
        })?;

    let steps: Vec<af_db::workflows::WorkflowStep> =
        serde_json::from_value(workflow.steps.clone()).map_err(|e| {
            ApiError::Internal(format!("invalid workflow steps: {e}"))
        })?;

    let invoker = Arc::new(JobQueueInvoker::new(
        state.pool.clone(),
        state.core_config.clone(),
        state.specs.clone(),
        state.executors.clone(),
    ));

    let mut orchestrator = OrchestratorRuntime::new(
        state.pool.clone(),
        state.router.clone(),
        state.specs.clone(),
        invoker,
    );
    orchestrator.set_evidence_resolvers(state.evidence_resolvers.clone());
    if let Some(ref hook) = state.post_tool_hook {
        orchestrator.set_post_tool_hook(hook.clone());
    }
    if let Some(uid) = user.user_id() {
        orchestrator.set_user_id(uid);
    }
    if let Some(ref route_str) = body.route {
        orchestrator.set_route_override(LlmRoute::from_str(route_str));
    }

    let (tx, rx) = mpsc::channel::<OrchestratorEvent>(512);
    let content = body.content.clone();
    let workflow_name = body.workflow_name.clone();
    let agent_configs = state.agent_configs.clone();

    let max_stream_secs: u64 = std::env::var("AF_MAX_STREAM_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);

    tokio::spawn(async move {
        let _guard = stream_guard;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(max_stream_secs),
            orchestrator.execute_workflow(
                thread_id,
                &workflow_name,
                &steps,
                &content,
                &agent_configs,
                tx,
            ),
        )
        .await;
        match result {
            Ok(Err(e)) => tracing::error!("orchestrator error: {e}"),
            Err(_) => tracing::error!("workflow stream duration limit exceeded"),
            Ok(Ok(())) => {}
        }
    });

    let stream = ReceiverStream::new(rx).map(|event| {
        let sse_event = match &event {
            OrchestratorEvent::AgentEvent { agent_name, event } => {
                let data = serde_json::json!({
                    "agent_name": agent_name,
                    "event": event,
                });
                Event::default()
                    .event("agent_event")
                    .data(data.to_string())
            }
            OrchestratorEvent::GroupComplete { group, agents } => {
                let data = serde_json::json!({
                    "group": group,
                    "agents": agents,
                });
                Event::default()
                    .event("group_complete")
                    .data(data.to_string())
            }
            OrchestratorEvent::WorkflowComplete { workflow_name } => {
                let data = serde_json::json!({
                    "workflow_name": workflow_name,
                });
                Event::default()
                    .event("workflow_complete")
                    .data(data.to_string())
            }
            OrchestratorEvent::SignalApplied {
                kind,
                target_agent,
                reason,
                source_agent,
            } => {
                let data = serde_json::json!({
                    "kind": kind,
                    "target_agent": target_agent,
                    "reason": reason,
                    "source_agent": source_agent,
                });
                Event::default()
                    .event("signal_applied")
                    .data(data.to_string())
            }
            OrchestratorEvent::RepivotApplied {
                original_artifact_id,
                new_artifact_id,
                new_filename,
                requeued_agents,
            } => {
                let data = serde_json::json!({
                    "original_artifact_id": original_artifact_id,
                    "new_artifact_id": new_artifact_id,
                    "new_filename": new_filename,
                    "requeued_agents": requeued_agents,
                });
                Event::default()
                    .event("repivot_applied")
                    .data(data.to_string())
            }
            OrchestratorEvent::FanOutStarted {
                parent_artifact_id,
                child_count,
                child_thread_ids,
            } => {
                let data = serde_json::json!({
                    "parent_artifact_id": parent_artifact_id,
                    "child_count": child_count,
                    "child_thread_ids": child_thread_ids,
                });
                Event::default()
                    .event("fan_out_started")
                    .data(data.to_string())
            }
            OrchestratorEvent::FanOutComplete {
                parent_thread_id,
                child_thread_ids,
                completed,
                failed,
            } => {
                let data = serde_json::json!({
                    "parent_thread_id": parent_thread_id,
                    "child_thread_ids": child_thread_ids,
                    "completed": completed,
                    "failed": failed,
                });
                Event::default()
                    .event("fan_out_complete")
                    .data(data.to_string())
            }
            OrchestratorEvent::Error(msg) => {
                Event::default().event("error").data(msg.clone())
            }
        };
        Ok(sse_event)
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
