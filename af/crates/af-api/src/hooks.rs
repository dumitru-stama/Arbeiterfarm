//! Hook execution engine: fires hooks in response to events.
//!
//! Safety: hooks fire ONLY on user-initiated uploads (API route + CLI), NOT on
//! tool-produced artifacts. Tools create artifacts via `af_db::artifacts::create_artifact()`
//! directly, bypassing the hook-firing path. This prevents indirect recursion where
//! hook → workflow → tool → new artifact → hook.
//!
//! Defense-in-depth: `MAX_HOOKS_PER_EVENT` limits how many hooks can fire per
//! single event to prevent hook storms from misconfigured projects.

use af_agents::OrchestratorRuntime;
use af_core::LlmRoute;
use af_db::project_hooks::ProjectHookRow;
use af_jobs::invoker::JobQueueInvoker;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

/// Maximum number of hooks that can fire per single event.
/// Prevents accidental hook storms from misconfigured projects.
const MAX_HOOKS_PER_EVENT: usize = 10;

/// Expand `{{variable}}` placeholders in a template string.
fn expand_template(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{{{key}}}}}"), value);
    }
    result
}

/// Fire all enabled `artifact_uploaded` hooks for a project.
/// Non-blocking: spawns each hook execution as a separate task and returns immediately.
pub async fn fire_artifact_hooks(
    state: &Arc<AppState>,
    project_id: Uuid,
    project_name: &str,
    artifact_id: Uuid,
    filename: &str,
    sha256: &str,
) {
    fire_artifact_hooks_inner(state, project_id, project_name, artifact_id, filename, sha256, false).await;
}

/// Fire all enabled `artifact_uploaded` hooks and wait for them to complete.
/// Designed for CLI usage where the process must not exit before hooks finish.
pub async fn fire_artifact_hooks_blocking(
    state: &Arc<AppState>,
    project_id: Uuid,
    project_name: &str,
    artifact_id: Uuid,
    filename: &str,
    sha256: &str,
) {
    fire_artifact_hooks_inner(state, project_id, project_name, artifact_id, filename, sha256, true).await;
}

async fn fire_artifact_hooks_inner(
    state: &Arc<AppState>,
    project_id: Uuid,
    project_name: &str,
    artifact_id: Uuid,
    filename: &str,
    sha256: &str,
    blocking: bool,
) {
    let hooks = match af_db::project_hooks::list_enabled_by_event(
        &state.pool,
        project_id,
        "artifact_uploaded",
    )
    .await
    {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("failed to load artifact hooks for project {project_id}: {e}");
            return;
        }
    };

    if hooks.is_empty() {
        return;
    }

    if hooks.len() > MAX_HOOKS_PER_EVENT {
        tracing::warn!(
            "project {project_id} has {} artifact_uploaded hooks (max {MAX_HOOKS_PER_EVENT}), firing first {MAX_HOOKS_PER_EVENT} only",
            hooks.len()
        );
    }

    let mut vars = HashMap::new();
    vars.insert("artifact_id", artifact_id.to_string());
    vars.insert("filename", filename.to_string());
    vars.insert("sha256", sha256.to_string());
    vars.insert("project_id", project_id.to_string());
    vars.insert("project_name", project_name.to_string());

    let mut handles = Vec::new();
    for hook in hooks.into_iter().take(MAX_HOOKS_PER_EVENT) {
        let content = expand_template(&hook.prompt_template, &vars);
        let state = state.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = execute_hook(&state, &hook, &content).await {
                tracing::error!("hook '{}' failed: {e}", hook.name);
            }
        });
        handles.push(handle);
    }

    if blocking {
        for handle in handles {
            let _ = handle.await;
        }
    }
}

/// Fire all due tick hooks (cross-project).
/// Spawns each hook as a background task and returns immediately.
pub async fn fire_tick_hooks(state: &Arc<AppState>) {
    let _ = fire_tick_hooks_inner(state, false).await;
}

/// Fire all due tick hooks and wait for them to complete.
/// Designed for the `af tick` CLI command (cron).
pub async fn fire_tick_hooks_blocking(state: &Arc<AppState>) {
    let _ = fire_tick_hooks_inner(state, true).await;
}

async fn fire_tick_hooks_inner(state: &Arc<AppState>, blocking: bool) {
    let hooks = match af_db::project_hooks::list_due_tick_hooks(&state.pool).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("failed to load due tick hooks: {e}");
            return;
        }
    };

    let mut handles = Vec::new();

    for hook in hooks {
        // Atomic claim — prevents double-firing across multiple server instances
        let claimed = match af_db::project_hooks::claim_tick(
            &state.pool,
            hook.id,
            hook.tick_generation,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("failed to claim tick hook {}: {e}", hook.id);
                continue;
            }
        };

        if !claimed {
            continue;
        }

        // Look up project name
        let project_name = match af_db::projects::get_project(&state.pool, hook.project_id).await
        {
            Ok(Some(p)) => p.name,
            _ => String::new(),
        };

        let tick_count = hook.tick_generation + 1;
        let mut vars = HashMap::new();
        vars.insert("project_id", hook.project_id.to_string());
        vars.insert("project_name", project_name);
        vars.insert("hook_name", hook.name.clone());
        vars.insert("tick_count", tick_count.to_string());

        let content = expand_template(&hook.prompt_template, &vars);
        let state = state.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = execute_hook(&state, &hook, &content).await {
                tracing::error!("tick hook '{}' failed: {e}", hook.name);
            }
        });
        handles.push(handle);
    }

    if blocking {
        for handle in handles {
            let _ = handle.await;
        }
    }
}

/// Execute a single hook: create a thread and run the workflow or agent.
async fn execute_hook(state: &AppState, hook: &ProjectHookRow, content: &str) -> Result<(), String> {
    let agent_name = hook
        .workflow_name
        .as_deref()
        .or(hook.agent_name.as_deref())
        .unwrap_or("hook");

    // Determine thread_type based on hook target
    let thread_type = if hook.workflow_name.is_some() {
        "workflow"
    } else {
        // Check if the agent has internal.* tools (i.e., is a thinker)
        let is_thinker = if let Some(ref an) = hook.agent_name {
            af_agents::resolve_agent_config(&state.pool, an, &state.agent_configs)
                .await
                .map(|c| c.allowed_tools.iter().any(|t| t.starts_with("internal.")))
                .unwrap_or(false)
        } else {
            false
        };
        if is_thinker { "thinking" } else { "agent" }
    };

    // Create thread for this hook execution
    let thread = af_db::threads::create_thread_typed(
        &state.pool,
        hook.project_id,
        agent_name,
        Some(&format!("hook:{}", hook.name)),
        thread_type,
    )
    .await
    .map_err(|e| format!("create thread: {e}"))?;

    // Audit log
    let detail = serde_json::json!({
        "hook_id": hook.id,
        "hook_name": hook.name,
        "event_type": hook.event_type,
        "project_id": hook.project_id,
        "thread_id": thread.id,
    });
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ =
            af_db::audit_log::insert(&pool, "hook_fired", None, None, Some(&detail)).await;
    });

    let invoker = Arc::new(JobQueueInvoker::new(
        state.pool.clone(),
        state.core_config.clone(),
        state.specs.clone(),
        state.executors.clone(),
    ));

    if let Some(ref workflow_name) = hook.workflow_name {
        // Workflow execution
        let workflow = af_db::workflows::get(&state.pool, workflow_name)
            .await
            .map_err(|e| format!("load workflow: {e}"))?
            .ok_or_else(|| format!("workflow '{workflow_name}' not found"))?;

        let steps: Vec<af_db::workflows::WorkflowStep> =
            serde_json::from_value(workflow.steps.clone())
                .map_err(|e| format!("parse workflow steps: {e}"))?;

        let mut orchestrator = OrchestratorRuntime::new(
            state.pool.clone(),
            state.router.clone(),
            state.specs.clone(),
            invoker,
        );
        orchestrator.set_evidence_resolvers(state.evidence_resolvers.clone());
        if let Some(ref post_hook) = state.post_tool_hook {
            orchestrator.set_post_tool_hook(post_hook.clone());
        }
        if let Some(ref route_str) = hook.route_override {
            orchestrator.set_route_override(LlmRoute::from_str(route_str));
        }

        // Drain channel — we don't stream hook results to anyone
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            while rx.recv().await.is_some() {}
        });

        orchestrator
            .execute_workflow(
                thread.id,
                workflow_name,
                &steps,
                content,
                &state.agent_configs,
                tx,
            )
            .await
            .map_err(|e| format!("workflow execution: {e}"))?;
    } else if let Some(ref agent_name_str) = hook.agent_name {
        // Single agent execution
        let agent_config = af_agents::resolve_agent_config(
            &state.pool,
            agent_name_str,
            &state.agent_configs,
        )
        .await
        .ok_or_else(|| format!("agent '{agent_name_str}' not found"))?;

        let mut runtime = af_agents::AgentRuntime::new(
            state.pool.clone(),
            state.router.clone(),
            state.specs.clone(),
            invoker,
        );
        runtime.set_evidence_resolvers(state.evidence_resolvers.clone());
        if let Some(ref post_hook) = state.post_tool_hook {
            runtime.set_post_tool_hook(post_hook.clone());
        }
        runtime.set_agent_name(agent_name_str.clone());

        runtime
            .send_message(thread.id, &agent_config, content)
            .await
            .map_err(|e| format!("agent execution: {e}"))?;
    }

    Ok(())
}
