//! Meta-tools for autonomous agent orchestration (thinking threads).
//!
//! Four Trusted in-process tools that let a supervisor agent invoke other agents,
//! read their results, and inspect project state.

use async_trait::async_trait;
use af_core::{
    AgentConfig, AgentEvent, CoreConfig, EvidenceResolverRegistry, LlmRoute, PostToolHook,
    ToolExecutor, ToolExecutorRegistry, ToolSpecRegistry,
};
use af_core::context::ToolContext;
use af_core::types::{
    OutputRedirectPolicy, SandboxProfile, ToolError, ToolOutputKind, ToolPolicy, ToolResult,
    ToolSpec,
};
use af_llm::LlmRouter;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// LazyMetaRefs — solves chicken-and-egg: executors need Arc<Registries>
// but are registered before Arc wrapping.
// ---------------------------------------------------------------------------

pub struct LazyMetaRefs {
    pub specs: std::sync::OnceLock<Arc<ToolSpecRegistry>>,
    pub executors: std::sync::OnceLock<Arc<ToolExecutorRegistry>>,
}

impl LazyMetaRefs {
    pub fn new() -> Self {
        Self {
            specs: std::sync::OnceLock::new(),
            executors: std::sync::OnceLock::new(),
        }
    }
}

/// Set the OnceLock values after Arc wrapping.
pub fn finalize_meta_refs(
    lazy_refs: &LazyMetaRefs,
    specs: Arc<ToolSpecRegistry>,
    executors: Arc<ToolExecutorRegistry>,
) {
    let _ = lazy_refs.specs.set(specs);
    let _ = lazy_refs.executors.set(executors);
}

// ---------------------------------------------------------------------------
// Tool spec declarations
// ---------------------------------------------------------------------------

pub fn declare_meta_tools(specs: &mut ToolSpecRegistry) {
    for spec in meta_specs() {
        specs
            .register(spec)
            .expect("failed to register meta-tool spec");
    }
}

fn meta_specs() -> Vec<ToolSpec> {
    vec![
        invoke_agent_spec(),
        list_agents_spec(),
        read_thread_spec(),
        list_artifacts_spec(),
        read_artifact_spec(),
    ]
}

fn invoke_agent_spec() -> ToolSpec {
    ToolSpec {
        name: "internal.invoke_agent".into(),
        version: 1,
        deprecated: false,
        description: "Invoke a specialist agent on a new child thread. The agent runs to \
                      completion and returns a summary of its findings."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Name of the agent to invoke (e.g. 'surface', 'decompiler', 'intel')"
                },
                "message": {
                    "type": "string",
                    "description": "The goal or instruction for the child agent, including any artifact IDs to analyze"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum seconds for the child agent to run (default 300)",
                    "minimum": 10,
                    "maximum": 600
                }
            },
            "required": ["agent_name", "message"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 660_000, // 11 minutes
            max_calls_per_run: 10,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

fn list_agents_spec() -> ToolSpec {
    ToolSpec {
        name: "internal.list_agents".into(),
        version: 1,
        deprecated: false,
        description: "List available specialist agents with their descriptions and tool sets."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name_filter": {
                    "type": "string",
                    "description": "Optional name prefix to filter agents (e.g. 're-' for RE agents)"
                }
            }
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            max_calls_per_run: 3,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

fn read_thread_spec() -> ToolSpec {
    ToolSpec {
        name: "internal.read_thread".into(),
        version: 1,
        deprecated: false,
        description: "Read messages from a thread in the same project (e.g. a child thread \
                      spawned by internal.invoke_agent)."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "thread_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "UUID of the thread to read"
                },
                "last_n": {
                    "type": "integer",
                    "description": "Only return the last N messages (default: all)",
                    "minimum": 1,
                    "maximum": 100
                },
                "roles": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter by message role (e.g. ['assistant', 'tool']). Default: all roles."
                }
            },
            "required": ["thread_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            max_calls_per_run: 10,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

fn list_artifacts_spec() -> ToolSpec {
    ToolSpec {
        name: "internal.list_artifacts".into(),
        version: 1,
        deprecated: false,
        description: "List all artifacts in the current project (files uploaded or produced by tools)."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "include_descriptions": {
                    "type": "boolean",
                    "description": "Include artifact descriptions in the output (default: true)"
                }
            }
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            max_calls_per_run: 5,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

fn read_artifact_spec() -> ToolSpec {
    ToolSpec {
        name: "internal.read_artifact".into(),
        version: 1,
        deprecated: false,
        description: "Read the contents of an artifact (file) in the current project. \
                      Returns text content directly for text files, or a hex dump for \
                      binary files. Use this to inspect tool outputs, decompiled code, \
                      extracted strings, or any other artifact."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "artifact_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "UUID of the artifact to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Byte offset to start reading from (default: 0)",
                    "minimum": 0
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes to read (default: 65536, max: 131072)",
                    "minimum": 1,
                    "maximum": 131072
                },
                "encoding": {
                    "type": "string",
                    "enum": ["text", "hex", "auto"],
                    "description": "How to return content: 'text' (UTF-8), 'hex' (hexdump), \
                                   'auto' (detect from content, default)"
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 30_000,
            max_calls_per_run: 20,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

// ---------------------------------------------------------------------------
// Executor wiring
// ---------------------------------------------------------------------------

pub fn wire_meta_tools(
    executors: &mut af_core::ToolExecutorRegistry,
    pool: PgPool,
    router: Arc<LlmRouter>,
    core_config: CoreConfig,
    agent_configs: Vec<AgentConfig>,
    lazy_refs: Arc<LazyMetaRefs>,
    evidence_resolvers: Option<Arc<EvidenceResolverRegistry>>,
    post_tool_hook: Option<Arc<dyn PostToolHook>>,
) {
    executors
        .register(Box::new(MetaInvokeAgentExecutor {
            pool: pool.clone(),
            router: router.clone(),
            core_config: core_config.clone(),
            agent_configs: agent_configs.clone(),
            lazy_refs: lazy_refs.clone(),
            evidence_resolvers: evidence_resolvers.clone(),
            post_tool_hook: post_tool_hook.clone(),
        }))
        .expect("failed to register internal.invoke_agent executor");

    executors
        .register(Box::new(MetaListAgentsExecutor {
            pool: pool.clone(),
            agent_configs: agent_configs.clone(),
        }))
        .expect("failed to register internal.list_agents executor");

    executors
        .register(Box::new(MetaReadThreadExecutor {
            pool: pool.clone(),
        }))
        .expect("failed to register internal.read_thread executor");

    executors
        .register(Box::new(MetaListArtifactsExecutor {
            pool: pool.clone(),
        }))
        .expect("failed to register internal.list_artifacts executor");

    executors
        .register(Box::new(MetaReadArtifactExecutor { pool }))
        .expect("failed to register internal.read_artifact executor");
}

// ---------------------------------------------------------------------------
// internal.invoke_agent executor
// ---------------------------------------------------------------------------

struct MetaInvokeAgentExecutor {
    pool: PgPool,
    router: Arc<LlmRouter>,
    core_config: CoreConfig,
    agent_configs: Vec<AgentConfig>,
    lazy_refs: Arc<LazyMetaRefs>,
    evidence_resolvers: Option<Arc<EvidenceResolverRegistry>>,
    post_tool_hook: Option<Arc<dyn PostToolHook>>,
}

/// Extract a short description from an agent's system prompt without exposing
/// the full prompt text. Takes the first sentence (up to first `. ` or newline),
/// capped at 120 chars.
fn extract_agent_description(system_prompt: &str) -> String {
    let trimmed = system_prompt.trim();
    // Find the end of the first sentence
    let end = trimmed
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| trimmed.find('\n'))
        .unwrap_or(trimmed.len())
        .min(120);
    let desc = &trimmed[..trimmed.floor_char_boundary(end)];
    if desc.len() < trimmed.len() {
        format!("{desc}...")
    } else {
        desc.to_string()
    }
}

fn tool_err(code: &str, message: String) -> ToolError {
    ToolError {
        code: code.into(),
        message,
        retryable: false,
        details: serde_json::Value::Null,
    }
}

#[async_trait]
impl ToolExecutor for MetaInvokeAgentExecutor {
    fn tool_name(&self) -> &str {
        "internal.invoke_agent"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &serde_json::Value) -> Result<(), String> {
        let name = input
            .get("agent_name")
            .and_then(|v| v.as_str())
            .ok_or("'agent_name' is required")?;
        if name.trim().is_empty() {
            return Err("'agent_name' must not be empty".into());
        }
        let message = input
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or("'message' is required")?;
        if message.trim().is_empty() {
            return Err("'message' must not be empty".into());
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let agent_name = input["agent_name"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'agent_name' required".into()))?;
        let message = input["message"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'message' required".into()))?;
        let timeout_secs = input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(300)
            .min(600);

        // Resolve child agent config
        let mut child_config = crate::resolve_agent_config(
            &self.pool,
            agent_name,
            &self.agent_configs,
        )
        .await
        .ok_or_else(|| tool_err("agent_not_found", format!("agent '{agent_name}' not found")))?;

        // Anti-recursion: strip internal.* tools from child's allowed_tools
        child_config
            .allowed_tools
            .retain(|t| !t.starts_with("meta."));

        // Get parent thread to find project_id
        let parent_thread_id = ctx
            .thread_id
            .ok_or_else(|| tool_err("no_thread", "internal.invoke_agent requires a thread context".into()))?;

        // Create child thread
        let child_thread = af_db::threads::create_child_thread_typed(
            &self.pool,
            ctx.project_id,
            agent_name,
            Some(&format!("child:{agent_name}")),
            parent_thread_id,
            "agent",
        )
        .await
        .map_err(|e| tool_err("db_error", format!("create child thread: {e}")))?;

        // Create invoker from lazy refs
        let specs = self
            .lazy_refs
            .specs
            .get()
            .ok_or_else(|| tool_err("not_initialized", "meta-tool registries not finalized".into()))?
            .clone();
        let executors = self
            .lazy_refs
            .executors
            .get()
            .ok_or_else(|| tool_err("not_initialized", "meta-tool registries not finalized".into()))?
            .clone();

        let specs_for_runtime = specs.clone();
        let invoker: Arc<dyn af_core::ToolInvoker> =
            Arc::new(af_jobs::invoker::JobQueueInvoker::new(
                self.pool.clone(),
                self.core_config.clone(),
                specs,
                executors,
            ));

        let mut runtime = crate::AgentRuntime::new(
            self.pool.clone(),
            self.router.clone(),
            specs_for_runtime,
            invoker,
        );
        if let Some(ref resolvers) = self.evidence_resolvers {
            runtime.set_evidence_resolvers(resolvers.clone());
        }
        if let Some(ref hook) = self.post_tool_hook {
            runtime.set_post_tool_hook(hook.clone());
        }
        runtime.set_agent_name(agent_name.to_string());

        // Propagate user_id for route access checks, quota tracking, and RLS scoping
        if let Some(uid) = ctx.actor_user_id {
            runtime.set_user_id(uid);
        }

        // Run child agent with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            runtime.send_message(child_thread.id, &child_config, message),
        )
        .await;

        let (status, response_text, tool_calls_made) = match result {
            Ok(Ok(events)) => {
                let mut text = String::new();
                let mut tool_count = 0u32;
                for event in &events {
                    match event {
                        AgentEvent::Done { content, .. } => {
                            text = content.clone();
                        }
                        AgentEvent::ToolCallResult { .. } => {
                            tool_count += 1;
                        }
                        _ => {}
                    }
                }
                ("completed", text, tool_count)
            }
            Ok(Err(e)) => ("error", format!("Agent error: {e}"), 0),
            Err(_) => ("timeout", format!("Agent timed out after {timeout_secs}s"), 0),
        };

        // Truncate response to 4000 chars for the supervisor
        let truncated = if response_text.len() > 4000 {
            let end = response_text
                .char_indices()
                .nth(4000)
                .map(|(i, _)| i)
                .unwrap_or(response_text.len());
            format!("{}... [truncated]", &response_text[..end])
        } else {
            response_text
        };

        // Count artifacts produced in child thread
        let artifacts_produced = af_db::artifacts::list_artifacts(&self.pool, ctx.project_id)
            .await
            .map(|a| a.len())
            .unwrap_or(0);

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: serde_json::json!({
                "thread_id": child_thread.id.to_string(),
                "agent": agent_name,
                "status": status,
                "response": truncated,
                "tool_calls_made": tool_calls_made,
                "artifacts_in_project": artifacts_produced,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// internal.list_agents executor
// ---------------------------------------------------------------------------

struct MetaListAgentsExecutor {
    pool: PgPool,
    agent_configs: Vec<AgentConfig>,
}

#[async_trait]
impl ToolExecutor for MetaListAgentsExecutor {
    fn tool_name(&self) -> &str {
        "internal.list_agents"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let name_filter = input
            .get("name_filter")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Gather from DB
        let db_agents = af_db::agents::list(&self.pool)
            .await
            .unwrap_or_default();

        let mut agents: Vec<serde_json::Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // DB agents first (authoritative)
        for row in &db_agents {
            if !name_filter.is_empty() && !row.name.starts_with(name_filter) {
                continue;
            }
            seen.insert(row.name.clone());
            let tools: Vec<String> = row
                .allowed_tools
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            agents.push(serde_json::json!({
                "name": row.name,
                "description": extract_agent_description(&row.system_prompt),
                "tools": tools,
            }));
        }

        // Fallback configs not in DB
        for config in &self.agent_configs {
            if seen.contains(&config.name) {
                continue;
            }
            if !name_filter.is_empty() && !config.name.starts_with(name_filter) {
                continue;
            }
            agents.push(serde_json::json!({
                "name": config.name,
                "description": extract_agent_description(&config.system_prompt),
                "tools": config.allowed_tools,
            }));
        }

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: serde_json::json!({
                "agents": agents,
                "total": agents.len(),
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// internal.read_thread executor
// ---------------------------------------------------------------------------

struct MetaReadThreadExecutor {
    pool: PgPool,
}

#[async_trait]
impl ToolExecutor for MetaReadThreadExecutor {
    fn tool_name(&self) -> &str {
        "internal.read_thread"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &serde_json::Value) -> Result<(), String> {
        let tid = input
            .get("thread_id")
            .and_then(|v| v.as_str())
            .ok_or("'thread_id' is required")?;
        Uuid::parse_str(tid).map_err(|_| "'thread_id' must be a valid UUID".to_string())?;
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let thread_id_str = input["thread_id"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'thread_id' required".into()))?;
        let thread_id = Uuid::parse_str(thread_id_str)
            .map_err(|_| tool_err("invalid_input", "invalid UUID".into()))?;

        // Verify thread belongs to same project
        let thread = af_db::threads::get_thread(&self.pool, thread_id)
            .await
            .map_err(|e| tool_err("db_error", format!("get thread: {e}")))?
            .ok_or_else(|| tool_err("not_found", format!("thread {thread_id} not found")))?;

        if thread.project_id != ctx.project_id {
            return Err(tool_err(
                "access_denied",
                "thread belongs to a different project".into(),
            ));
        }

        let last_n = input
            .get("last_n")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let role_filter: Option<Vec<String>> = input
            .get("roles")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let mut messages = af_db::messages::get_thread_messages(&self.pool, thread_id)
            .await
            .map_err(|e| tool_err("db_error", format!("get messages: {e}")))?;

        // Filter by role
        if let Some(ref roles) = role_filter {
            messages.retain(|m| roles.contains(&m.role));
        }

        // Take last N
        if let Some(n) = last_n {
            if messages.len() > n {
                messages = messages.split_off(messages.len() - n);
            }
        }

        let result: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let content = m.content.as_deref().unwrap_or("");
                // Truncate individual messages to 2000 chars
                let truncated = if content.len() > 2000 {
                    format!("{}...", &content[..content.floor_char_boundary(2000)])
                } else {
                    content.to_string()
                };
                serde_json::json!({
                    "role": m.role,
                    "agent_name": m.agent_name,
                    "content": truncated,
                    "created_at": m.created_at.to_rfc3339(),
                })
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: serde_json::json!({
                "thread_id": thread_id.to_string(),
                "message_count": result.len(),
                "messages": result,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// internal.list_artifacts executor
// ---------------------------------------------------------------------------

struct MetaListArtifactsExecutor {
    pool: PgPool,
}

#[async_trait]
impl ToolExecutor for MetaListArtifactsExecutor {
    fn tool_name(&self) -> &str {
        "internal.list_artifacts"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let include_desc = input
            .get("include_descriptions")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let artifacts = af_db::artifacts::list_artifacts(&self.pool, ctx.project_id)
            .await
            .map_err(|e| tool_err("db_error", format!("list artifacts: {e}")))?;

        let result: Vec<serde_json::Value> = artifacts
            .iter()
            .map(|a| {
                let mut obj = serde_json::json!({
                    "id": a.id.to_string(),
                    "filename": a.filename,
                    "sha256": a.sha256,
                });
                if include_desc {
                    obj["description"] = serde_json::json!(a.description);
                }
                if let Some(ref src) = a.source_tool_run_id {
                    obj["source_tool_run_id"] = serde_json::json!(src.to_string());
                }
                obj
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: serde_json::json!({
                "artifacts": result,
                "total": result.len(),
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// internal.read_artifact executor
// ---------------------------------------------------------------------------

struct MetaReadArtifactExecutor {
    pool: PgPool,
}

/// Check if a byte slice looks like valid UTF-8 text (no control chars except whitespace).
fn looks_like_text(data: &[u8]) -> bool {
    // Quick check: must be valid UTF-8
    let text = match std::str::from_utf8(data) {
        Ok(t) => t,
        Err(_) => return false,
    };
    // Reject if >5% control characters (excluding \t \n \r)
    let total = text.len();
    if total == 0 {
        return true;
    }
    let control_count = text
        .chars()
        .filter(|c| c.is_control() && *c != '\t' && *c != '\n' && *c != '\r')
        .count();
    (control_count * 100 / total) < 5
}

/// Format bytes as a hex dump (16 bytes per line, with ASCII sidebar).
fn hexdump(data: &[u8], base_offset: u64) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let addr = base_offset + (i * 16) as u64;
        out.push_str(&format!("{addr:08x}  "));
        for (j, byte) in chunk.iter().enumerate() {
            if j == 8 {
                out.push(' ');
            }
            out.push_str(&format!("{byte:02x} "));
        }
        // Pad if short row
        let pad = 16 - chunk.len();
        for _ in 0..pad {
            out.push_str("   ");
        }
        if chunk.len() <= 8 {
            out.push(' ');
        }
        out.push_str(" |");
        for byte in chunk {
            if byte.is_ascii_graphic() || *byte == b' ' {
                out.push(*byte as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("|\n");
    }
    out
}

#[async_trait]
impl ToolExecutor for MetaReadArtifactExecutor {
    fn tool_name(&self) -> &str {
        "internal.read_artifact"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &serde_json::Value) -> Result<(), String> {
        let id = input
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .ok_or("'artifact_id' is required")?;
        Uuid::parse_str(id).map_err(|_| "'artifact_id' must be a valid UUID".to_string())?;
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact_id_str = input["artifact_id"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'artifact_id' required".into()))?;
        let artifact_id = Uuid::parse_str(artifact_id_str)
            .map_err(|_| tool_err("invalid_input", "invalid UUID".into()))?;

        let offset = input
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let max_bytes = input
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(65_536)
            .min(131_072) as usize;
        let encoding = input
            .get("encoding")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");

        // Look up artifact and verify project ownership
        let artifact = af_db::artifacts::get_artifact(&self.pool, artifact_id)
            .await
            .map_err(|e| tool_err("db_error", format!("get artifact: {e}")))?
            .ok_or_else(|| {
                tool_err("not_found", format!("artifact {artifact_id} not found"))
            })?;

        if artifact.project_id != ctx.project_id {
            return Err(tool_err(
                "access_denied",
                "artifact belongs to a different project".into(),
            ));
        }

        // Resolve blob storage path
        let blob = af_db::blobs::get_blob(&self.pool, &artifact.sha256)
            .await
            .map_err(|e| tool_err("db_error", format!("get blob: {e}")))?
            .ok_or_else(|| {
                tool_err("not_found", format!("blob {} not found on disk", artifact.sha256))
            })?;

        let file_size = blob.size_bytes as u64;

        // Read the requested range
        if offset >= file_size {
            return Ok(ToolResult {
                kind: ToolOutputKind::InlineJson,
                output_json: serde_json::json!({
                    "artifact_id": artifact_id.to_string(),
                    "filename": artifact.filename,
                    "size_bytes": file_size,
                    "offset": offset,
                    "bytes_read": 0,
                    "content": "",
                    "encoding": "text",
                    "truncated": false,
                }),
                stdout: None,
                stderr: None,
                produced_artifacts: vec![],
                primary_artifact: None,
                evidence: vec![],
            });
        }

        let read_len = max_bytes.min((file_size - offset) as usize);

        let data = {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let mut file = tokio::fs::File::open(&blob.storage_path)
                .await
                .map_err(|e| tool_err("io_error", format!("open file: {e}")))?;
            if offset > 0 {
                file.seek(std::io::SeekFrom::Start(offset))
                    .await
                    .map_err(|e| tool_err("io_error", format!("seek: {e}")))?;
            }
            let mut buf = vec![0u8; read_len];
            let n = file
                .read_exact(&mut buf)
                .await
                .map_err(|e| tool_err("io_error", format!("read: {e}")))?;
            buf.truncate(n);
            buf
        };

        let truncated = (offset + data.len() as u64) < file_size;

        let (content, used_encoding) = match encoding {
            "text" => {
                let text = String::from_utf8_lossy(&data).into_owned();
                (text, "text")
            }
            "hex" => {
                let dump = hexdump(&data, offset);
                (dump, "hex")
            }
            _ => {
                // auto: detect
                if looks_like_text(&data) {
                    let text = String::from_utf8_lossy(&data).into_owned();
                    (text, "text")
                } else {
                    let dump = hexdump(&data, offset);
                    (dump, "hex")
                }
            }
        };

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: serde_json::json!({
                "artifact_id": artifact_id.to_string(),
                "filename": artifact.filename,
                "size_bytes": file_size,
                "offset": offset,
                "bytes_read": data.len(),
                "content": content,
                "encoding": used_encoding,
                "truncated": truncated,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Default thinker agent config
// ---------------------------------------------------------------------------

pub fn build_thinker_agent() -> AgentConfig {
    AgentConfig {
        name: "thinker".into(),
        system_prompt: THINKER_SYSTEM_PROMPT.into(),
        allowed_tools: vec!["internal.*".into()],
        default_route: LlmRoute::Auto,
        metadata: serde_json::json!({}),
        tool_call_budget: Some(30),
        timeout_secs: Some(1800),
    }
}

const THINKER_SYSTEM_PROMPT: &str = "\
You are an autonomous analysis supervisor. Your job is to examine uploaded \
artifacts and coordinate specialist agents to produce a thorough analysis.

## Available meta-tools

- **internal.list_agents**: See which specialist agents are available and what they do.
- **internal.list_artifacts**: See which files are uploaded to the project.
- **internal.read_artifact**: Read the contents of any artifact (text files shown as-is, binary as hex dump). Use this to inspect decompiled code, tool outputs, extracted data, or any file directly.
- **internal.invoke_agent**: Spawn a specialist agent on a child thread with a specific goal.
- **internal.read_thread**: Read the results from a child thread after the agent completes.

## Strategy

1. Start by listing available artifacts and agents.
2. Read key artifacts directly when you need to understand file contents (e.g., decompiled code, extracted strings, tool reports).
3. Invoke specialist agents for tasks that require specialized tools (e.g., disassembly, decompilation, threat intel lookups).
4. Read child thread results and artifact outputs to decide if deeper analysis is needed.
5. After all relevant analysis is complete, synthesize a final summary.

## Guidelines

- Read artifact contents directly when possible — don't invoke an agent just to read a text file.
- Invoke agents for tasks that require their specialized tools (e.g., rizin, ghidra, VirusTotal).
- Always cite child thread IDs so findings are traceable.
- Invoke agents one at a time and read results before deciding next steps.
- Do not repeat analysis that a child agent has already completed.
- If an agent fails or times out, note this and continue with other agents.
- End with a clear verdict or summary that references all child findings.
";
