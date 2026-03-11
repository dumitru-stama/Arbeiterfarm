use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use af_agents::AgentRuntime;
use af_auth::Action;
use af_core::{AgentEvent, LlmRoute};
use af_jobs::invoker::JobQueueInvoker;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use axum::extract::Query;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{MessageResponse, QueueMessageRequest, QueueMessageResponse, SendMessageRequest};
use crate::error::ApiError;
use crate::AppState;

/// POST /api/v1/threads/:id/messages — SSE streaming response
pub async fn send_sse(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Auth check in short-lived scoped tx (entity lookup inside tx to prevent enumeration)
    let thread = {
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
        thread
    };

    // SSE stream limit per user
    let stream_user_id = user.user_id().unwrap_or(uuid::Uuid::nil());
    let stream_guard = state
        .stream_tracker
        .acquire(stream_user_id)
        .await
        .map_err(|_| ApiError::QuotaExceeded("concurrent stream limit reached".into()))?;

    let agent_name = body
        .agent_name
        .as_deref()
        .unwrap_or(&thread.agent_name);

    let mut agent_config = af_agents::resolve_agent_config(
        &state.pool,
        agent_name,
        &state.agent_configs,
    )
    .await
    .ok_or_else(|| ApiError::BadRequest(format!("agent '{agent_name}' not found")))?;

    // Per-message route override
    if let Some(ref route_str) = body.route {
        agent_config.default_route = LlmRoute::from_str(route_str);
    }

    // Per-message system prompt override
    if let Some(ref prompt) = body.system_prompt_override {
        agent_config.system_prompt = prompt.clone();
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
    // Only set user_id for non-admin users so that the agent runtime uses
    // RLS-scoped transactions. Admin users bypass RLS (matching the auth check above).
    if !user.is_admin() {
        if let Some(uid) = user.user_id() {
            runtime.set_user_id(uid);
        }
    }

    let max_stream_secs: u64 = std::env::var("AF_MAX_STREAM_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);

    // Apply per-agent timeout (capped by global limit)
    let effective_secs = agent_config
        .timeout_secs
        .map(|s| (s as u64).min(max_stream_secs))
        .unwrap_or(max_stream_secs);

    let (tx, rx) = mpsc::channel::<AgentEvent>(256);
    let content = body.content.clone();

    tokio::spawn(async move {
        // Keep stream_guard alive for the duration of the stream
        let _guard = stream_guard;
        let error_tx = tx.clone();

        // Stream duration limit (per-agent or global, whichever is lower)
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(effective_secs),
            runtime.send_message_streaming(thread_id, &agent_config, &content, tx),
        )
        .await;
        match result {
            Ok(Err(e)) => {
                eprintln!("[af] agent error: {e}");
                let _ = error_tx.send(AgentEvent::Error(format!("{e}"))).await;
            }
            Err(_) => {
                let _ = error_tx
                    .send(AgentEvent::Error("stream duration limit exceeded".into()))
                    .await;
            }
            Ok(Ok(())) => {}
        }
    });

    let stream = ReceiverStream::new(rx).map(|event| {
        let sse_event = match &event {
            AgentEvent::Token(t) => Event::default().event("token").data(t),
            AgentEvent::Reasoning(t) => Event::default().event("reasoning").data(t),
            AgentEvent::ToolCallStart {
                tool_name,
                tool_input,
            } => {
                let data = serde_json::json!({
                    "tool_name": tool_name,
                    "tool_input": tool_input,
                });
                Event::default()
                    .event("tool_start")
                    .data(data.to_string())
            }
            AgentEvent::ToolCallResult {
                tool_name,
                success,
                summary,
            } => {
                let data = serde_json::json!({
                    "tool_name": tool_name,
                    "success": success,
                    "summary": summary,
                });
                Event::default()
                    .event("tool_result")
                    .data(data.to_string())
            }
            AgentEvent::Evidence { ref_type, ref_id } => {
                let data = serde_json::json!({
                    "ref_type": ref_type,
                    "ref_id": ref_id,
                });
                Event::default()
                    .event("evidence")
                    .data(data.to_string())
            }
            AgentEvent::Done {
                message_id,
                content,
            } => {
                let data = serde_json::json!({
                    "message_id": message_id,
                    "content": content,
                });
                Event::default()
                    .event("done")
                    .data(data.to_string())
            }
            AgentEvent::Usage {
                prompt_tokens,
                completion_tokens,
                cached_read_tokens,
                cache_creation_tokens,
                route,
                context_window,
            } => {
                let data = serde_json::json!({
                    "prompt_tokens": prompt_tokens,
                    "completion_tokens": completion_tokens,
                    "cached_read_tokens": cached_read_tokens,
                    "cache_creation_tokens": cache_creation_tokens,
                    "route": route,
                    "context_window": context_window,
                });
                Event::default()
                    .event("usage")
                    .data(data.to_string())
            }
            AgentEvent::ContextCompacted {
                estimated_tokens,
                messages_compacted,
                context_window,
            } => {
                let data = serde_json::json!({
                    "estimated_tokens": estimated_tokens,
                    "messages_compacted": messages_compacted,
                    "context_window": context_window,
                });
                Event::default()
                    .event("context_compacted")
                    .data(data.to_string())
            }
            AgentEvent::Error(msg) => Event::default().event("error").data(msg.clone()),
        };
        Ok(sse_event)
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// POST /api/v1/threads/:id/messages/queue — insert user message without triggering LLM
pub async fn queue_message(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<QueueMessageRequest>,
) -> Result<Json<QueueMessageResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    if body.content.trim().is_empty() {
        return Err(ApiError::BadRequest("content must not be empty".into()));
    }

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

    let row = af_db::messages::insert_message(&mut *tx, thread_id, "user", Some(&body.content), None).await?;
    tx.commit().await?;

    Ok(Json(QueueMessageResponse {
        id: row.id,
        seq: row.seq,
        created_at: row.created_at,
    }))
}

/// POST /api/v1/threads/:id/messages/sync — non-streaming response
pub async fn send_sync(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Auth check in short-lived scoped tx (entity lookup inside tx to prevent enumeration)
    let thread = {
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
        thread
    };

    let agent_name = body
        .agent_name
        .as_deref()
        .unwrap_or(&thread.agent_name);

    let mut agent_config = af_agents::resolve_agent_config(
        &state.pool,
        agent_name,
        &state.agent_configs,
    )
    .await
    .ok_or_else(|| ApiError::BadRequest(format!("agent '{agent_name}' not found")))?;

    // Per-message route override
    if let Some(ref route_str) = body.route {
        agent_config.default_route = LlmRoute::from_str(route_str);
    }

    // Per-message system prompt override
    if let Some(ref prompt) = body.system_prompt_override {
        agent_config.system_prompt = prompt.clone();
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

    // Apply same timeout policy as SSE endpoint (agent timeout capped by global)
    let max_stream_secs: u64 = std::env::var("AF_MAX_STREAM_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);
    let effective_secs = agent_config
        .timeout_secs
        .map(|s| (s as u64).min(max_stream_secs))
        .unwrap_or(max_stream_secs);

    let events = tokio::time::timeout(
        std::time::Duration::from_secs(effective_secs),
        runtime.send_message(thread_id, &agent_config, &body.content),
    )
    .await
    .map_err(|_| ApiError::Internal("agent execution timed out".into()))?
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let json_events: Vec<serde_json::Value> = events
        .iter()
        .map(|event| match event {
            AgentEvent::Token(t) => serde_json::json!({"type": "token", "data": t}),
            AgentEvent::Reasoning(t) => serde_json::json!({"type": "reasoning", "data": t}),
            AgentEvent::ToolCallStart {
                tool_name,
                tool_input,
            } => serde_json::json!({
                "type": "tool_start",
                "tool_name": tool_name,
                "tool_input": tool_input,
            }),
            AgentEvent::ToolCallResult {
                tool_name,
                success,
                summary,
            } => serde_json::json!({
                "type": "tool_result",
                "tool_name": tool_name,
                "success": success,
                "summary": summary,
            }),
            AgentEvent::Evidence { ref_type, ref_id } => serde_json::json!({
                "type": "evidence",
                "ref_type": ref_type,
                "ref_id": ref_id,
            }),
            AgentEvent::Done {
                message_id,
                content,
            } => serde_json::json!({
                "type": "done",
                "message_id": message_id,
                "content": content,
            }),
            AgentEvent::Usage {
                prompt_tokens,
                completion_tokens,
                cached_read_tokens,
                cache_creation_tokens,
                route,
                context_window,
            } => serde_json::json!({
                "type": "usage",
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "cached_read_tokens": cached_read_tokens,
                "cache_creation_tokens": cache_creation_tokens,
                "route": route,
                "context_window": context_window,
            }),
            AgentEvent::ContextCompacted {
                estimated_tokens,
                messages_compacted,
                context_window,
            } => serde_json::json!({
                "type": "context_compacted",
                "estimated_tokens": estimated_tokens,
                "messages_compacted": messages_compacted,
                "context_window": context_window,
            }),
            AgentEvent::Error(msg) => serde_json::json!({"type": "error", "data": msg}),
        })
        .collect();

    Ok(Json(json_events))
}

/// GET /api/v1/threads/:id/messages — list messages in thread
pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<Vec<MessageResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

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
        .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} not found")))?;

    require_project_access(&mut *tx, &user, thread.project_id, Action::Read).await?;

    let rows = af_db::messages::get_thread_messages(&mut *tx, thread_id).await?;
    tx.commit().await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

#[derive(serde::Deserialize)]
pub struct PromptPreviewQuery {
    pub agent: Option<String>,
}

/// GET /api/v1/threads/:id/prompt-preview — show the resolved system prompt
pub async fn prompt_preview(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(thread_id): Path<Uuid>,
    Query(query): Query<PromptPreviewQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

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
        .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} not found")))?;

    require_project_access(&mut *tx, &user, thread.project_id, Action::Read).await?;
    tx.commit().await?;

    let agent_name = query.agent.as_deref().unwrap_or(&thread.agent_name);
    let agent_config = af_agents::resolve_agent_config(
        &state.pool,
        agent_name,
        &state.agent_configs,
    )
    .await
    .ok_or_else(|| ApiError::BadRequest(format!("agent '{agent_name}' not found")))?;

    let caps = state
        .router
        .resolve(&agent_config.default_route)
        .map(|b| b.capabilities());
    let supports_native_tools = caps.as_ref().map(|c| c.supports_tool_calls).unwrap_or(false);
    let compact_tools = caps.as_ref().map(|c| c.is_local).unwrap_or(false);

    let mut system_prompt = if supports_native_tools {
        af_agents::prompt_builder::build_system_prompt_minimal(&agent_config, &state.specs, compact_tools)
    } else {
        af_agents::prompt_builder::build_system_prompt(&agent_config, &state.specs)
    };

    // Append artifact context with parent sample resolution
    let artifacts = af_db::artifacts::list_artifacts(&state.pool, thread.project_id).await
        .unwrap_or_default();
    let parent_map: std::collections::HashMap<uuid::Uuid, uuid::Uuid> =
        af_db::tool_run_artifacts::resolve_parent_samples(&state.pool, thread.project_id).await
            .unwrap_or_default()
            .into_iter()
            .collect();
    let artifact_ctx: Vec<_> = artifacts
        .into_iter()
        .map(|a| {
            let parent_id = parent_map.get(&a.id).copied();
            (a.id, a.filename, a.description, a.source_tool_run_id, parent_id)
        })
        .collect();
    let _index = af_agents::prompt_builder::append_artifact_context(&mut system_prompt, &artifact_ctx, thread.target_artifact_id);

    Ok(Json(serde_json::json!({
        "agent_name": agent_name,
        "mode": if supports_native_tools { "native_tools" } else { "json_blocks" },
        "system_prompt": system_prompt,
    })))
}
