use chrono::Utc;
use af_core::{
    AgentConfig, AgentEvent, EvidenceResolverRegistry, LlmRoute, OrchestratorEvent,
    PostToolHook, ToolInvoker, ToolSpecRegistry,
};
use af_llm::LlmRouter;
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::AgentError;
use crate::runtime::AgentRuntime;
use crate::signal_parser::{self, AgentSignal, SignalKind};

/// Maximum number of repivots per workflow execution.
const MAX_REPIVOTS: u32 = 5;

/// Maximum fan-out depth (allows recursive analysis of nested archives).
const MAX_FANOUT_DEPTH: u32 = 3;

/// Maximum number of child artifacts per fan-out.
const MAX_FANOUT_CHILDREN: usize = 50;

/// Orchestrates multi-agent workflows: groups of agents executing in parallel,
/// groups executing sequentially, all sharing the same thread.
///
/// Agents can emit signal markers in their output to dynamically route
/// the workflow (request, skip, or prioritize other agents).
pub struct OrchestratorRuntime {
    pool: PgPool,
    router: Arc<LlmRouter>,
    specs: Arc<ToolSpecRegistry>,
    invoker: Arc<dyn ToolInvoker>,
    evidence_resolvers: Option<Arc<EvidenceResolverRegistry>>,
    post_tool_hook: Option<Arc<dyn PostToolHook>>,
    user_id: Option<Uuid>,
    route_override: Option<LlmRoute>,
    fanout_depth: u32,
    /// SHA256 hashes already analyzed in this fan-out chain (cycle detection).
    visited_sha256: HashSet<String>,
}

impl OrchestratorRuntime {
    pub fn new(
        pool: PgPool,
        router: Arc<LlmRouter>,
        specs: Arc<ToolSpecRegistry>,
        invoker: Arc<dyn ToolInvoker>,
    ) -> Self {
        Self {
            pool,
            router,
            specs,
            invoker,
            evidence_resolvers: None,
            post_tool_hook: None,
            user_id: None,
            route_override: None,
            fanout_depth: 0,
            visited_sha256: HashSet::new(),
        }
    }

    pub fn set_evidence_resolvers(&mut self, resolvers: Arc<EvidenceResolverRegistry>) {
        self.evidence_resolvers = Some(resolvers);
    }

    pub fn set_post_tool_hook(&mut self, hook: Arc<dyn PostToolHook>) {
        self.post_tool_hook = Some(hook);
    }

    pub fn set_user_id(&mut self, user_id: Uuid) {
        self.user_id = Some(user_id);
    }

    pub fn set_route_override(&mut self, route: LlmRoute) {
        self.route_override = Some(route);
    }

    /// Execute a workflow on a thread.
    ///
    /// Steps are grouped by `group` number. Groups execute in ascending order.
    /// Within a group, steps with `parallel: true` run concurrently first,
    /// then steps without it run sequentially. All agents share the same thread.
    ///
    /// After each group completes, agent output is scanned for signal markers
    /// (`signal:request:...`, `signal:skip:...`, `signal:priority:...`) and
    /// remaining groups are dynamically modified before continuing.
    ///
    /// **Repivot**: If a tool produces a replacement artifact (metadata contains
    /// `repivot_from`), completed agents with `can_repivot: true` are re-queued
    /// and a retarget notice is inserted into the thread.
    ///
    /// **Fan-out**: If a tool produces child artifacts (metadata contains
    /// `fan_out_from`), each child gets its own thread running the full workflow
    /// independently. The parent blocks until all children complete.
    /// Returns a boxed future to allow recursive fan-out spawning via tokio::spawn.
    pub fn execute_workflow<'a>(
        &'a self,
        thread_id: Uuid,
        workflow_name: &'a str,
        steps: &'a [af_db::workflows::WorkflowStep],
        initial_message: &'a str,
        agent_configs: &'a [AgentConfig],
        event_tx: mpsc::Sender<OrchestratorEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send + 'a>> {
        Box::pin(async move {
        // Clone steps into owned groups so we can mutate between iterations
        let mut groups: BTreeMap<u32, Vec<af_db::workflows::WorkflowStep>> = BTreeMap::new();
        for step in steps {
            groups.entry(step.group).or_default().push(step.clone());
        }

        // Collect all known agent names for signal validation
        let known_agents: HashSet<String> = agent_configs.iter().map(|a| a.name.clone()).collect();

        // Look up project_id from thread (needed for repivot artifact query)
        let thread_row = af_db::threads::get_thread(&self.pool, thread_id)
            .await
            .map_err(|e| AgentError::Db(e.to_string()))?
            .ok_or_else(|| AgentError::Db(format!("thread {} not found", thread_id)))?;
        let project_id = thread_row.project_id;

        // Insert user message once (scoped to enforce RLS if user_id is set)
        if let Some(uid) = self.user_id {
            let mut tx = af_db::scoped::begin_scoped(&self.pool, uid)
                .await
                .map_err(|e| AgentError::Db(e.to_string()))?;
            af_db::messages::insert_message(
                &mut *tx,
                thread_id,
                "user",
                Some(initial_message),
                None,
            )
            .await
            .map_err(|e| AgentError::Db(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| AgentError::Db(e.to_string()))?;
        } else {
            af_db::messages::insert_message(
                &self.pool,
                thread_id,
                "user",
                Some(initial_message),
                None,
            )
            .await
            .map_err(|e| AgentError::Db(e.to_string()))?;
        }

        // Repivot tracking
        let mut repivot_count: u32 = 0;
        let mut completed_agents: Vec<String> = Vec::new();

        // Dynamic group loop: pop first group, execute, parse signals, mutate remaining
        loop {
            let current_group_num = match groups.keys().next().copied() {
                Some(n) => n,
                None => break, // no more groups
            };
            let group_steps = groups.remove(&current_group_num).unwrap();

            // Record time before group execution for repivot/fanout artifact query
            let group_start_time = Utc::now();

            // Read global max for per-step timeout capping
            let global_max_secs: u64 = std::env::var("AF_MAX_STREAM_DURATION_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1800);

            eprintln!("[orchestrator] group {} starting with {} step(s)", current_group_num,
                group_steps.len());

            let mut agent_names = Vec::new();
            let mut done_contents: Vec<(String, String)> = Vec::new();
            let mut timed_out_set: HashSet<String> = HashSet::new();

            // Partition steps: parallel-flagged steps run concurrently first,
            // then sequential steps run one-by-one after the parallel batch.
            let (parallel_steps, sequential_steps): (Vec<_>, Vec<_>) =
                group_steps.iter().partition(|s| s.parallel);

            if !parallel_steps.is_empty() {
                eprintln!(
                    "[orchestrator] group {} running {} parallel step(s)",
                    current_group_num, parallel_steps.len()
                );
            }

            // --- Parallel batch ---
            // Steps marked parallel=true run concurrently via tokio::JoinSet.
            // They share the same thread so DB writes are safe; the LLM calls
            // and tool executions run independently.
            if !parallel_steps.is_empty() {
                let mut join_set: tokio::task::JoinSet<(
                    String,                          // agent_name
                    bool,                            // timed_out
                    Option<(String, String)>,         // done_content
                    Result<(), AgentError>,           // result
                )> = tokio::task::JoinSet::new();

                for step in &parallel_steps {
                    let agent_config = resolve_agent(&step.agent, agent_configs, &self.pool).await;
                    let agent_config = match agent_config {
                        Some(c) => c,
                        None => {
                            eprintln!("[orchestrator] ERROR: agent '{}' not found!", step.agent);
                            let _ = event_tx
                                .send(OrchestratorEvent::Error(format!(
                                    "agent '{}' not found", step.agent
                                )))
                                .await;
                            continue;
                        }
                    };

                    agent_names.push(step.agent.clone());

                    let mut task_config = agent_config.clone();
                    task_config.system_prompt =
                        build_workflow_prompt(&agent_config.system_prompt, &step.prompt, &known_agents);
                    if let Some(ref route) = self.route_override {
                        task_config.default_route = route.clone();
                    }

                    let effective_timeout = step.timeout_secs
                        .or(task_config.timeout_secs)
                        .map(|s| std::time::Duration::from_secs((s as u64).min(global_max_secs)));

                    let agent_name = step.agent.clone();
                    let pool = self.pool.clone();
                    let router = self.router.clone();
                    let specs = self.specs.clone();
                    let invoker = self.invoker.clone();
                    let evidence_resolvers = self.evidence_resolvers.clone();
                    let post_tool_hook = self.post_tool_hook.clone();
                    let user_id = self.user_id;
                    let event_tx_clone = event_tx.clone();
                    let tid = thread_id;

                    join_set.spawn(async move {
                        eprintln!("[orchestrator] running parallel agent '{}'...", agent_name);

                        let mut runtime = AgentRuntime::new(pool.clone(), router, specs, invoker);
                        if let Some(resolvers) = evidence_resolvers {
                            runtime.set_evidence_resolvers(resolvers);
                        }
                        if let Some(hook) = post_tool_hook {
                            runtime.set_post_tool_hook(hook);
                        }
                        if let Some(uid) = user_id {
                            runtime.set_user_id(uid);
                        }
                        runtime.set_agent_name(agent_name.clone());

                        let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(256);
                        let fwd_tx = event_tx_clone;
                        let fwd_name = agent_name.clone();
                        let fwd_handle: tokio::task::JoinHandle<Option<(String, String)>> =
                            tokio::spawn(async move {
                                let mut done_content: Option<(String, String)> = None;
                                while let Some(ev) = agent_rx.recv().await {
                                    if let AgentEvent::Done { ref content, .. } = ev {
                                        done_content = Some((fwd_name.clone(), content.clone()));
                                    }
                                    let _ = fwd_tx
                                        .send(OrchestratorEvent::AgentEvent {
                                            agent_name: fwd_name.clone(),
                                            event: ev,
                                        })
                                        .await;
                                }
                                done_content
                            });

                        let mut timed_out = false;
                        let result = if let Some(dur) = effective_timeout {
                            match tokio::time::timeout(
                                dur,
                                runtime.continue_thread_streaming(tid, &task_config, agent_tx),
                            ).await {
                                Ok(r) => r,
                                Err(_) => {
                                    timed_out = true;
                                    let timeout_msg = format!(
                                        "[TIMEOUT] Agent '{}' timed out after {}s. \
                                         Partial results may be incomplete.",
                                        agent_name, dur.as_secs()
                                    );
                                    let _ = af_db::messages::insert_message(
                                        &pool, tid, "system", Some(&timeout_msg), None,
                                    ).await;
                                    Ok(())
                                }
                            }
                        } else {
                            runtime.continue_thread_streaming(tid, &task_config, agent_tx).await
                        };

                        let done_content = fwd_handle.await.ok().flatten();
                        (agent_name, timed_out, done_content, result)
                    });
                }

                // Collect results from all parallel agents
                while let Some(join_result) = join_set.join_next().await {
                    match join_result {
                        Ok((name, timed_out, content, result)) => {
                            match &result {
                                Ok(()) => eprintln!("[orchestrator] parallel agent '{}' completed", name),
                                Err(e) => {
                                    eprintln!("[orchestrator] parallel agent '{}' FAILED: {}", name, e);
                                    let _ = event_tx
                                        .send(OrchestratorEvent::Error(format!(
                                            "agent '{}' failed: {}", name, e
                                        )))
                                        .await;
                                }
                            }
                            if timed_out {
                                timed_out_set.insert(name);
                            }
                            if let Some(c) = content {
                                done_contents.push(c);
                            }
                        }
                        Err(e) => {
                            eprintln!("[orchestrator] parallel agent panicked: {e}");
                            let _ = event_tx
                                .send(OrchestratorEvent::Error(format!("parallel agent panicked: {e}")))
                                .await;
                        }
                    }
                }
            }

            // --- Sequential steps ---
            // Steps without parallel=true run one-by-one after the parallel batch.
            // This is the original behavior, safe for Ghidra tools and rate-limited backends.
            for step in &sequential_steps {
                eprintln!("[orchestrator] resolving agent '{}'...", step.agent);
                let agent_config = resolve_agent(&step.agent, agent_configs, &self.pool).await;
                let agent_config = match agent_config {
                    Some(c) => {
                        eprintln!("[orchestrator] agent '{}' resolved, route={:?}", step.agent, c.default_route);
                        c
                    }
                    None => {
                        eprintln!("[orchestrator] ERROR: agent '{}' not found!", step.agent);
                        let _ = event_tx
                            .send(OrchestratorEvent::Error(format!(
                                "agent '{}' not found",
                                step.agent
                            )))
                            .await;
                        continue;
                    }
                };

                agent_names.push(step.agent.clone());

                // Build workflow-enhanced system prompt
                let mut task_config = agent_config.clone();
                task_config.system_prompt =
                    build_workflow_prompt(&agent_config.system_prompt, &step.prompt, &known_agents);
                if let Some(ref route) = self.route_override {
                    task_config.default_route = route.clone();
                }

                let agent_name = step.agent.clone();

                // Compute effective timeout: step > agent > none; always capped at global max
                let effective_timeout = step.timeout_secs
                    .or(task_config.timeout_secs)
                    .map(|s| std::time::Duration::from_secs((s as u64).min(global_max_secs)));

                eprintln!("[orchestrator] running agent '{}' in group {}...", agent_name, current_group_num);

                let mut agent_timed_out = false;
                let mut runtime = AgentRuntime::new(
                    self.pool.clone(),
                    self.router.clone(),
                    self.specs.clone(),
                    self.invoker.clone(),
                );
                if let Some(ref resolvers) = self.evidence_resolvers {
                    runtime.set_evidence_resolvers(resolvers.clone());
                }
                if let Some(ref hook) = self.post_tool_hook {
                    runtime.set_post_tool_hook(hook.clone());
                }
                if let Some(uid) = self.user_id {
                    runtime.set_user_id(uid);
                }
                runtime.set_agent_name(agent_name.clone());

                // Create a forwarding channel that wraps AgentEvents
                // and captures Done content for signal parsing
                let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(256);

                let fwd_tx = event_tx.clone();
                let fwd_name = agent_name.clone();
                let fwd_handle: tokio::task::JoinHandle<Option<(String, String)>> =
                    tokio::spawn(async move {
                        let mut done_content: Option<(String, String)> = None;
                        while let Some(ev) = agent_rx.recv().await {
                            // Capture Done content for signal parsing
                            if let AgentEvent::Done { ref content, .. } = ev {
                                done_content =
                                    Some((fwd_name.clone(), content.clone()));
                            }
                            let _ = fwd_tx
                                .send(OrchestratorEvent::AgentEvent {
                                    agent_name: fwd_name.clone(),
                                    event: ev,
                                })
                                .await;
                        }
                        done_content
                    });

                let result = if let Some(dur) = effective_timeout {
                    match tokio::time::timeout(
                        dur,
                        runtime.continue_thread_streaming(thread_id, &task_config, agent_tx),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            agent_timed_out = true;
                            // Insert [TIMEOUT] marker into thread for downstream agents
                            let timeout_msg = format!(
                                "[TIMEOUT] Agent '{}' timed out after {}s. \
                                 Partial results may be incomplete.",
                                agent_name,
                                dur.as_secs()
                            );
                            let _ = af_db::messages::insert_message(
                                &self.pool,
                                thread_id,
                                "system",
                                Some(&timeout_msg),
                                None,
                            )
                            .await;
                            let _ = event_tx
                                .send(OrchestratorEvent::Error(timeout_msg))
                                .await;
                            Ok(()) // treat as completed, don't abort other agents
                        }
                    }
                } else {
                    runtime
                        .continue_thread_streaming(thread_id, &task_config, agent_tx)
                        .await
                };

                // Wait for forwarding to complete and get captured content
                let done_content = fwd_handle.await.ok().flatten();

                match &result {
                    Ok(()) => eprintln!("[orchestrator] agent '{}' completed successfully", agent_name),
                    Err(e) => {
                        eprintln!("[orchestrator] agent '{}' FAILED: {}", agent_name, e);
                        let _ = event_tx
                            .send(OrchestratorEvent::Error(format!(
                                "agent '{}' failed: {}",
                                agent_name, e
                            )))
                            .await;
                    }
                }

                if agent_timed_out {
                    timed_out_set.insert(agent_name.clone());
                }
                if let Some(c) = done_content {
                    done_contents.push(c);
                }
            }

            let _ = event_tx
                .send(OrchestratorEvent::GroupComplete {
                    group: current_group_num,
                    agents: agent_names.clone(),
                })
                .await;

            // Track completed agents for repivot re-queuing (exclude timed-out)
            completed_agents.extend(
                agent_names
                    .iter()
                    .filter(|name| !timed_out_set.contains(name.as_str()))
                    .cloned(),
            );

            // Parse signals from all agent outputs and apply to remaining groups
            let mut all_signals = Vec::new();
            for (agent_name, content) in &done_contents {
                let signals = signal_parser::parse_signals(content, agent_name);
                all_signals.extend(signals);
            }
            if !all_signals.is_empty() {
                let resolved = signal_parser::resolve_conflicts(all_signals);
                apply_signals(
                    &resolved,
                    &mut groups,
                    current_group_num,
                    &known_agents,
                    &event_tx,
                )
                .await;
            }

            // Repivot detection: check for replacement artifacts produced during this group
            if repivot_count < MAX_REPIVOTS {
                if let Ok(repivot_artifacts) =
                    af_db::artifacts::find_repivot_artifacts_since(
                        &self.pool,
                        project_id,
                        group_start_time,
                    )
                    .await
                {
                    for artifact in repivot_artifacts {
                        let original_id_str = artifact
                            .metadata
                            .get("repivot_from")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();

                        // Insert retarget notice into thread so all agents see it
                        let notice = format!(
                            "[RETARGET] A replacement artifact has been produced. \
                             Original artifact: {original_id_str} -> New artifact: {} ({}). \
                             All subsequent analysis should target the new artifact.",
                            artifact.id, artifact.filename,
                        );
                        let _ = af_db::messages::insert_message(
                            &self.pool,
                            thread_id,
                            "system",
                            Some(&notice),
                            None,
                        )
                        .await;

                        // Determine insertion group for re-queued agents
                        let next_group_num = groups
                            .keys()
                            .next()
                            .copied()
                            .unwrap_or(current_group_num + 1);

                        // Re-queue eligible completed agents
                        let mut requeued: Vec<String> = Vec::new();
                        for agent_name in &completed_agents {
                            // Skip if already planned in remaining groups
                            if already_planned(agent_name, &groups) {
                                continue;
                            }
                            // Find original step to check can_repivot
                            let original_step =
                                steps.iter().find(|s| s.agent == *agent_name);
                            let can_repivot = original_step
                                .map(|s| s.can_repivot)
                                .unwrap_or(true);
                            if !can_repivot {
                                continue;
                            }
                            let original_prompt = original_step
                                .map(|s| s.prompt.as_str())
                                .unwrap_or("");

                            let new_step = af_db::workflows::WorkflowStep {
                                agent: agent_name.clone(),
                                group: next_group_num,
                                prompt: format!(
                                    "{original_prompt} [RETARGET: analyze new artifact {} ({}) \
                                     instead of original {original_id_str}]",
                                    artifact.id, artifact.filename,
                                ),
                                can_repivot: false, // prevent cascading
                                timeout_secs: original_step.and_then(|s| s.timeout_secs),
                                parallel: original_step.map(|s| s.parallel).unwrap_or(false),
                            };
                            groups
                                .entry(next_group_num)
                                .or_default()
                                .push(new_step);
                            requeued.push(agent_name.clone());
                        }

                        // Append [RETARGET: ...] to all existing remaining steps
                        let retarget_suffix = format!(
                            " [RETARGET: analyze new artifact {} ({}) instead of original {original_id_str}]",
                            artifact.id, artifact.filename,
                        );
                        for steps_in_group in groups.values_mut() {
                            for step in steps_in_group.iter_mut() {
                                // Don't double-annotate re-queued steps
                                if !step.prompt.contains("[RETARGET:") {
                                    step.prompt.push_str(&retarget_suffix);
                                }
                            }
                        }

                        let _ = event_tx
                            .send(OrchestratorEvent::RepivotApplied {
                                original_artifact_id: original_id_str,
                                new_artifact_id: artifact.id.to_string(),
                                new_filename: artifact.filename.clone(),
                                requeued_agents: requeued,
                            })
                            .await;

                        repivot_count += 1;
                        break; // only 1 repivot per group
                    }
                }
            }

            // Fan-out detection: check for extracted child artifacts
            if self.fanout_depth < MAX_FANOUT_DEPTH {
                if let Ok(fanout_artifacts) =
                    af_db::artifacts::find_fanout_artifacts_since(
                        &self.pool,
                        project_id,
                        group_start_time,
                    )
                    .await
                {
                    if !fanout_artifacts.is_empty() {
                        // Group children by their fan_out_from parent artifact
                        let mut by_parent: HashMap<String, Vec<af_db::artifacts::ArtifactRow>> =
                            HashMap::new();
                        for artifact in fanout_artifacts {
                            let parent_id = artifact
                                .metadata
                                .get("fan_out_from")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            by_parent.entry(parent_id).or_default().push(artifact);
                        }

                        for (parent_artifact_id, children) in &by_parent {
                            // Enforce child cap
                            let capped = if children.len() > MAX_FANOUT_CHILDREN {
                                let _ = event_tx
                                    .send(OrchestratorEvent::Error(format!(
                                        "fan-out from artifact {} has {} children, capping at {}",
                                        parent_artifact_id,
                                        children.len(),
                                        MAX_FANOUT_CHILDREN,
                                    )))
                                    .await;
                                &children[..MAX_FANOUT_CHILDREN]
                            } else {
                                children.as_slice()
                            };

                            let mut child_handles = Vec::new();
                            let mut child_thread_ids = Vec::new();

                            // Build visited set for children: include all children's SHA256s
                            let mut child_visited = self.visited_sha256.clone();
                            for c in capped {
                                child_visited.insert(c.sha256.clone());
                            }

                            for child_artifact in capped {
                                // Cycle detection: skip if this SHA256 was already analyzed
                                if self.visited_sha256.contains(&child_artifact.sha256) {
                                    let _ = event_tx
                                        .send(OrchestratorEvent::Error(format!(
                                            "fan-out cycle detected: sha256:{}.. already analyzed, skipping {}",
                                            &child_artifact.sha256[..16.min(child_artifact.sha256.len())],
                                            child_artifact.filename,
                                        )))
                                        .await;
                                    continue;
                                }

                                // Create child thread
                                let child_title = format!(
                                    "{}-fanout: {}",
                                    workflow_name, child_artifact.filename
                                );
                                let child_thread = match af_db::threads::create_child_thread(
                                    &self.pool,
                                    project_id,
                                    &format!("{}-fanout", workflow_name),
                                    Some(&child_title),
                                    thread_id,
                                )
                                .await
                                {
                                    Ok(t) => t,
                                    Err(e) => {
                                        let _ = event_tx
                                            .send(OrchestratorEvent::Error(format!(
                                                "failed to create child thread for {}: {}",
                                                child_artifact.filename, e
                                            )))
                                            .await;
                                        continue;
                                    }
                                };

                                let child_thread_id = child_thread.id;
                                child_thread_ids.push(child_thread_id);

                                // Build the child's initial message with lineage info
                                let child_initial_msg = format!(
                                    "[FAN-OUT] This thread was created from parent thread {} \
                                     for child artifact {} ({}, sha256:{}). \
                                     Parent artifact: {}.\n\n\
                                     Analyze artifact {} ({})",
                                    thread_id,
                                    child_artifact.id,
                                    child_artifact.filename,
                                    child_artifact.sha256,
                                    parent_artifact_id,
                                    child_artifact.id,
                                    child_artifact.filename,
                                );

                                // Spawn child workflow with its own OrchestratorRuntime
                                let pool = self.pool.clone();
                                let router = self.router.clone();
                                let specs = self.specs.clone();
                                let invoker = self.invoker.clone();
                                let evidence_resolvers = self.evidence_resolvers.clone();
                                let post_tool_hook = self.post_tool_hook.clone();
                                let user_id = self.user_id;
                                let child_event_tx = event_tx.clone();
                                let child_steps = steps.to_vec();
                                let child_agent_configs = agent_configs.to_vec();
                                let child_workflow_name = workflow_name.to_string();
                                let child_depth = self.fanout_depth + 1;
                                let child_route_override = self.route_override.clone();
                                let child_visited_set = child_visited.clone();

                                let handle = tokio::spawn(async move {
                                    let mut orch = OrchestratorRuntime::new(
                                        pool, router, specs, invoker,
                                    );
                                    if let Some(resolvers) = evidence_resolvers {
                                        orch.set_evidence_resolvers(resolvers);
                                    }
                                    if let Some(hook) = post_tool_hook {
                                        orch.set_post_tool_hook(hook);
                                    }
                                    if let Some(uid) = user_id {
                                        orch.set_user_id(uid);
                                    }
                                    if let Some(route) = child_route_override {
                                        orch.set_route_override(route);
                                    }
                                    orch.fanout_depth = child_depth;
                                    orch.visited_sha256 = child_visited_set;

                                    orch.execute_workflow(
                                        child_thread_id,
                                        &child_workflow_name,
                                        &child_steps,
                                        &child_initial_msg,
                                        &child_agent_configs,
                                        child_event_tx,
                                    )
                                    .await
                                });

                                child_handles.push((child_thread_id, handle));
                            }

                            // Emit FanOutStarted
                            let _ = event_tx
                                .send(OrchestratorEvent::FanOutStarted {
                                    parent_artifact_id: parent_artifact_id.clone(),
                                    child_count: child_thread_ids.len(),
                                    child_thread_ids: child_thread_ids.clone(),
                                })
                                .await;

                            // Insert notice into parent thread
                            let child_ids_str: Vec<String> =
                                child_thread_ids.iter().map(|id| id.to_string()).collect();
                            let notice = format!(
                                "[FAN-OUT] Spawned {} child threads for artifact {}: [{}]",
                                child_thread_ids.len(),
                                parent_artifact_id,
                                child_ids_str.join(", "),
                            );
                            let _ = af_db::messages::insert_message(
                                &self.pool,
                                thread_id,
                                "system",
                                Some(&notice),
                                None,
                            )
                            .await;

                            // Wait for all children to complete
                            let mut completed = 0usize;
                            let mut failed = 0usize;
                            for (child_tid, handle) in child_handles {
                                match handle.await {
                                    Ok(Ok(())) => completed += 1,
                                    Ok(Err(e)) => {
                                        failed += 1;
                                        let _ = event_tx
                                            .send(OrchestratorEvent::Error(format!(
                                                "child thread {} failed: {}",
                                                child_tid, e
                                            )))
                                            .await;
                                    }
                                    Err(e) => {
                                        failed += 1;
                                        let _ = event_tx
                                            .send(OrchestratorEvent::Error(format!(
                                                "child thread {} panicked: {}",
                                                child_tid, e
                                            )))
                                            .await;
                                    }
                                }
                            }

                            // Insert completion notice into parent thread
                            let total = completed + failed;
                            let complete_notice = format!(
                                "[FAN-OUT COMPLETE] {completed} succeeded, {failed} failed ({total} total) \
                                 for artifact {parent_artifact_id}",
                            );
                            let _ = af_db::messages::insert_message(
                                &self.pool,
                                thread_id,
                                "system",
                                Some(&complete_notice),
                                None,
                            )
                            .await;

                            // Emit FanOutComplete
                            let _ = event_tx
                                .send(OrchestratorEvent::FanOutComplete {
                                    parent_thread_id: thread_id,
                                    child_thread_ids,
                                    completed,
                                    failed,
                                })
                                .await;
                        }
                    }
                }
            }
        }

        let _ = event_tx
            .send(OrchestratorEvent::WorkflowComplete {
                workflow_name: workflow_name.to_string(),
            })
            .await;

        Ok(())
        }) // Box::pin(async move {
    }
}

/// Build the system prompt for a workflow agent, including signal documentation
/// and the list of available agent names.
fn build_workflow_prompt(base_prompt: &str, task_prompt: &str, known_agents: &HashSet<String>) -> String {
    let mut agent_list: Vec<&str> = known_agents.iter().map(|s| s.as_str()).collect();
    agent_list.sort();

    format!(
        "{base_prompt}\n\n\
         You are part of a multi-agent workflow. Your specific task: {task_prompt}\n\n\
         ## Workflow Signals\n\n\
         You can emit signals to influence which agents run next. \
         Only emit signals when you have clear evidence-based reasons. \
         Do not emit signals speculatively.\n\n\
         Format: `signal:<kind>:<agent_name>:<reason>`\n\n\
         - `signal:request:<agent>:<reason>` — Request that an agent be added to the next group.\n\
         - `signal:skip:<agent>:<reason>` — Remove an agent from all future groups.\n\
         - `signal:priority:<agent>:<reason>` — Move an agent to run sooner.\n\n\
         Available agents: {agent_list}",
        agent_list = agent_list.join(", "),
    )
}

/// Apply resolved signals to the remaining workflow groups.
///
/// - `request`: Add agent to next group if it exists in known_agents and is not
///   already present in remaining groups.
/// - `skip`: Remove agent from all remaining groups.
/// - `priority`: Move agent from a later group to the next group.
async fn apply_signals(
    signals: &[AgentSignal],
    groups: &mut BTreeMap<u32, Vec<af_db::workflows::WorkflowStep>>,
    current_group_num: u32,
    known_agents: &HashSet<String>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
) {
    // Determine the insertion group: the first remaining group key
    let next_group_num = groups.keys().next().copied().unwrap_or(current_group_num + 1);

    for signal in signals {
        match signal.kind {
            SignalKind::Request => {
                // Only add if agent is known and not already planned in remaining groups
                if !known_agents.contains(&signal.target_agent) {
                    continue;
                }
                if already_planned(&signal.target_agent, groups) {
                    continue;
                }

                let step = af_db::workflows::WorkflowStep {
                    agent: signal.target_agent.clone(),
                    group: next_group_num,
                    prompt: signal.reason.clone(),
                    can_repivot: true,
                    timeout_secs: None,
                    parallel: false,
                };
                groups.entry(next_group_num).or_default().push(step);

                let _ = event_tx
                    .send(OrchestratorEvent::SignalApplied {
                        kind: signal.kind.to_string(),
                        target_agent: signal.target_agent.clone(),
                        reason: signal.reason.clone(),
                        source_agent: signal.source_agent.clone(),
                    })
                    .await;
            }
            SignalKind::Skip => {
                let target = &signal.target_agent;
                let mut removed = false;

                for steps in groups.values_mut() {
                    let before = steps.len();
                    steps.retain(|s| s.agent != *target);
                    if steps.len() < before {
                        removed = true;
                    }
                }
                // Clean up empty groups
                groups.retain(|_, steps| !steps.is_empty());

                if removed {
                    let _ = event_tx
                        .send(OrchestratorEvent::SignalApplied {
                            kind: signal.kind.to_string(),
                            target_agent: signal.target_agent.clone(),
                            reason: signal.reason.clone(),
                            source_agent: signal.source_agent.clone(),
                        })
                        .await;
                }
            }
            SignalKind::Priority => {
                let target = &signal.target_agent;

                // Find the agent in a group beyond the next group
                let mut found_in_group: Option<(u32, af_db::workflows::WorkflowStep)> = None;
                for (&gnum, steps) in groups.iter() {
                    if gnum == next_group_num {
                        // Already in next group — no-op
                        if steps.iter().any(|s| s.agent == *target) {
                            found_in_group = None;
                            break;
                        }
                        continue;
                    }
                    if let Some(pos) = steps.iter().position(|s| s.agent == *target) {
                        found_in_group = Some((gnum, steps[pos].clone()));
                        break;
                    }
                }

                if let Some((from_group, mut step)) = found_in_group {
                    // Remove from original group
                    if let Some(steps) = groups.get_mut(&from_group) {
                        steps.retain(|s| s.agent != *target);
                        if steps.is_empty() {
                            groups.remove(&from_group);
                        }
                    }

                    // Insert into next group with priority reason appended
                    step.group = next_group_num;
                    step.prompt = format!("{} [PRIORITY: {}]", step.prompt, signal.reason);
                    groups.entry(next_group_num).or_default().push(step);

                    let _ = event_tx
                        .send(OrchestratorEvent::SignalApplied {
                            kind: signal.kind.to_string(),
                            target_agent: signal.target_agent.clone(),
                            reason: signal.reason.clone(),
                            source_agent: signal.source_agent.clone(),
                        })
                        .await;
                }
            }
        }
    }
}

/// Check if an agent is already present in any remaining group.
fn already_planned(
    agent_name: &str,
    groups: &BTreeMap<u32, Vec<af_db::workflows::WorkflowStep>>,
) -> bool {
    groups
        .values()
        .any(|steps| steps.iter().any(|s| s.agent == agent_name))
}

/// Resolve an agent config: try DB first, fall back to compiled-in configs.
async fn resolve_agent(
    name: &str,
    fallback_configs: &[AgentConfig],
    pool: &PgPool,
) -> Option<AgentConfig> {
    // Try DB first
    if let Ok(Some(row)) = af_db::agents::get(pool, name).await {
        let allowed_tools: Vec<String> = row
            .allowed_tools
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        return Some(AgentConfig {
            name: row.name,
            system_prompt: row.system_prompt,
            allowed_tools,
            default_route: LlmRoute::from_str(&row.default_route),
            metadata: row.metadata,
            tool_call_budget: None,
            timeout_secs: row.timeout_secs.map(|s| s as u32),
        });
    }

    // Fall back to compiled-in
    fallback_configs.iter().find(|a| a.name == name).cloned()
}

/// Public helper for resolving agents (used by API and CLI).
pub async fn resolve_agent_config(
    pool: &PgPool,
    name: &str,
    fallback_configs: &[AgentConfig],
) -> Option<AgentConfig> {
    resolve_agent(name, fallback_configs, pool).await
}
