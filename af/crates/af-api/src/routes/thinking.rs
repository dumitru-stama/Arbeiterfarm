use axum::extract::{Path, State};
use axum::Json;
use af_agents::AgentRuntime;
use af_auth::Action;
use af_core::{AgentEvent, LlmRoute};
use af_jobs::invoker::JobQueueInvoker;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{RunThinkingRequest, StartThinkingRequest};
use crate::error::ApiError;
use crate::AppState;

/// POST /api/v1/projects/:id/thinking — start autonomous thinking thread
///
/// Creates a thread, spawns the thinking task in the background, and returns
/// the thread ID as JSON immediately. The thinking continues server-side.
pub async fn start(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    Json(body): Json<StartThinkingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
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

        af_db::projects::get_project(&mut *tx, project_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("project {project_id} not found")))?;

        require_project_access(&mut *tx, &user, project_id, Action::Write).await?;
        tx.commit().await?;
    }

    // Stream limit per user (released when background task finishes)
    let stream_user_id = user.user_id().unwrap_or(uuid::Uuid::nil());
    let stream_guard = state
        .stream_tracker
        .acquire(stream_user_id)
        .await
        .map_err(|_| ApiError::QuotaExceeded("concurrent stream limit reached".into()))?;

    // Resolve thinker agent
    let agent_name = body.agent_name.as_deref().unwrap_or("thinker");
    let mut agent_config = af_agents::resolve_agent_config(
        &state.pool,
        agent_name,
        &state.agent_configs,
    )
    .await
    .ok_or_else(|| ApiError::BadRequest(format!("agent '{agent_name}' not found")))?;

    // Per-request route override
    if let Some(ref route_str) = body.route {
        agent_config.default_route = LlmRoute::from_str(route_str);
    }

    // Create thinking thread
    let title = body
        .title
        .as_deref()
        .unwrap_or("Thinking thread");
    let thread = af_db::threads::create_thread_typed(
        &state.pool,
        project_id,
        agent_name,
        Some(title),
        "thinking",
    )
    .await
    .map_err(|e| ApiError::Internal(format!("create thread: {e}")))?;

    let router = state.router.clone();

    let invoker = Arc::new(JobQueueInvoker::new(
        state.pool.clone(),
        state.core_config.clone(),
        state.specs.clone(),
        state.executors.clone(),
    ));

    let mut runtime = AgentRuntime::new(
        state.pool.clone(),
        router,
        state.specs.clone(),
        invoker,
    );
    runtime.set_evidence_resolvers(state.evidence_resolvers.clone());
    if let Some(ref hook) = state.post_tool_hook {
        runtime.set_post_tool_hook(hook.clone());
    }
    runtime.set_compaction_threshold(state.compaction_threshold);
    if let Some(ref sb) = state.summarization_backend {
        runtime.set_summarization_backend(sb.clone());
    }
    if !user.is_admin() {
        if let Some(uid) = user.user_id() {
            runtime.set_user_id(uid);
        }
    }

    let max_stream_secs: u64 = std::env::var("AF_MAX_STREAM_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);

    let effective_secs = agent_config
        .timeout_secs
        .map(|s| (s as u64).min(max_stream_secs))
        .unwrap_or(max_stream_secs);

    // Fire-and-forget channel — the background task sends events into it,
    // but nobody reads them. All sends use `let _ =` so they handle the
    // closed receiver gracefully.
    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);
    let goal = body.goal.clone();
    let thread_id = thread.id;

    tokio::spawn(async move {
        let _guard = stream_guard;
        let error_tx = tx.clone();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(effective_secs),
            runtime.send_message_streaming(thread_id, &agent_config, &goal, tx),
        )
        .await;
        match result {
            Ok(Err(e)) => {
                eprintln!("[af] thinking error: {e}");
                let _ = error_tx.send(AgentEvent::Error(format!("{e}"))).await;
            }
            Err(_) => {
                let _ = error_tx
                    .send(AgentEvent::Error("thinking thread duration limit exceeded".into()))
                    .await;
            }
            Ok(Ok(())) => {}
        }
    });

    Ok(Json(serde_json::json!({ "thread_id": thread_id })))
}

/// POST /api/v1/threads/:id/thinking — start thinking on an existing thread
///
/// Like `start`, but operates on an already-created thread instead of creating
/// one. Used by the UI "Run Analysis" button.
pub async fn run(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<RunThinkingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Look up thread and auth-check its project
    let project_id = {
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
        thread.project_id
    };
    let _ = project_id; // used for auth above

    // Stream limit per user
    let stream_user_id = user.user_id().unwrap_or(uuid::Uuid::nil());
    let stream_guard = state
        .stream_tracker
        .acquire(stream_user_id)
        .await
        .map_err(|_| ApiError::QuotaExceeded("concurrent stream limit reached".into()))?;

    // Resolve thinker agent
    let agent_name = body.agent_name.as_deref().unwrap_or("thinker");
    let mut agent_config = af_agents::resolve_agent_config(
        &state.pool,
        agent_name,
        &state.agent_configs,
    )
    .await
    .ok_or_else(|| ApiError::BadRequest(format!("agent '{agent_name}' not found")))?;

    if let Some(ref route_str) = body.route {
        agent_config.default_route = LlmRoute::from_str(route_str);
    }

    let router = state.router.clone();
    let invoker = Arc::new(JobQueueInvoker::new(
        state.pool.clone(),
        state.core_config.clone(),
        state.specs.clone(),
        state.executors.clone(),
    ));

    let mut runtime = AgentRuntime::new(
        state.pool.clone(),
        router,
        state.specs.clone(),
        invoker,
    );
    runtime.set_evidence_resolvers(state.evidence_resolvers.clone());
    if let Some(ref hook) = state.post_tool_hook {
        runtime.set_post_tool_hook(hook.clone());
    }
    runtime.set_compaction_threshold(state.compaction_threshold);
    if let Some(ref sb) = state.summarization_backend {
        runtime.set_summarization_backend(sb.clone());
    }
    if !user.is_admin() {
        if let Some(uid) = user.user_id() {
            runtime.set_user_id(uid);
        }
    }

    let max_stream_secs: u64 = std::env::var("AF_MAX_STREAM_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);
    let effective_secs = agent_config
        .timeout_secs
        .map(|s| (s as u64).min(max_stream_secs))
        .unwrap_or(max_stream_secs);

    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);
    let goal = body.goal.clone();

    tokio::spawn(async move {
        let _guard = stream_guard;
        let error_tx = tx.clone();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(effective_secs),
            runtime.send_message_streaming(thread_id, &agent_config, &goal, tx),
        )
        .await;
        match result {
            Ok(Err(e)) => {
                eprintln!("[af] thinking error: {e}");
                let _ = error_tx.send(AgentEvent::Error(format!("{e}"))).await;
            }
            Err(_) => {
                let _ = error_tx
                    .send(AgentEvent::Error("thinking thread duration limit exceeded".into()))
                    .await;
            }
            Ok(Ok(())) => {}
        }
    });

    Ok(Json(serde_json::json!({ "status": "started" })))
}
