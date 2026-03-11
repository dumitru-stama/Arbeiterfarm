use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Declares a tool's identity and schema. Pure data, no execution logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub version: u32,
    pub deprecated: bool,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub policy: ToolPolicy,
    pub output_redirect: OutputRedirectPolicy,
}

/// Execution guardrails for a tool. Declared by the plugin, enforced by af-jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub sandbox: SandboxProfile,
    pub max_input_bytes: u64,
    pub max_input_depth: u32,
    pub timeout_ms: u64,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    pub max_output_bytes: u64,
    pub max_produced_artifacts: u32,
    pub egress_allowlist: Vec<String>,
    pub allow_shell: bool,
    pub allow_exec: bool,
    /// Unix Domain Socket paths to bind-mount (read-only) into bwrap sandbox.
    /// Used by gateway-pattern tools that talk to a local daemon via UDS.
    pub uds_bind_mounts: Vec<PathBuf>,
    /// Directories to bind-mount read-write into bwrap sandbox.
    /// Example: Ghidra cache directory needs write access for project files.
    #[serde(default)]
    pub writable_bind_mounts: Vec<PathBuf>,
    /// Extra directories to bind-mount read-only into bwrap sandbox.
    /// Example: /etc/ssl/certs for TLS, tool-specific config directories.
    #[serde(default)]
    pub extra_ro_bind_mounts: Vec<PathBuf>,
    /// Maximum number of times this tool can be called within a single agent run.
    /// Prevents LLM-as-adversary abuse (repeatedly calling the same tool).
    #[serde(default = "default_max_calls_per_run")]
    pub max_calls_per_run: u32,
}

fn default_max_calls_per_run() -> u32 {
    10
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            sandbox: SandboxProfile::NoNetReadOnly,
            max_input_bytes: 256 * 1024,
            max_input_depth: 16,
            timeout_ms: 60_000,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 256 * 1024,
            max_output_bytes: 64 * 1024 * 1024,
            max_produced_artifacts: 16,
            egress_allowlist: Vec::new(),
            allow_shell: false,
            allow_exec: false,
            uds_bind_mounts: Vec::new(),
            writable_bind_mounts: Vec::new(),
            extra_ro_bind_mounts: Vec::new(),
            max_calls_per_run: default_max_calls_per_run(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxProfile {
    NoNetReadOnly,
    NoNetReadOnlyTmpfs,
    PrivateLoopback,
    NetEgressAllowlist,
    Trusted,
}

/// A request to run a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool_name: String,
    pub input_json: serde_json::Value,
    pub project_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub parent_message_id: Option<Uuid>,
    #[serde(default)]
    pub actor_user_id: Option<Uuid>,
}

/// The result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub kind: ToolOutputKind,
    pub output_json: serde_json::Value,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub produced_artifacts: Vec<Uuid>,
    pub primary_artifact: Option<Uuid>,
    pub evidence: Vec<crate::evidence::EvidenceRef>,
}

/// Classifies the output type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolOutputKind {
    InlineJson,
    JsonArtifact,
    Text,
    Binary,
    Mixed,
}

/// Whether the worker may redirect oversized output_json to an artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputRedirectPolicy {
    Allowed,
    Forbidden,
}

impl Default for OutputRedirectPolicy {
    fn default() -> Self {
        Self::Allowed
    }
}

/// Typed, user-facing error from a tool executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: serde_json::Value,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

/// Reference to an artifact on disk + its metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: Uuid,
    pub sha256: String,
    pub filename: String,
    pub storage_path: PathBuf,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
    pub source_tool_run_id: Option<Uuid>,
}

/// Spawn config for out-of-process executors.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub binary_path: PathBuf,
    pub protocol_version: u32,
    pub supported_tools: Vec<(String, u32)>,
    /// Tool-specific configuration passed through to the OOP executor via OopContext.extra.
    /// Examples: rizin_path, ghidra_home, cache_dir.
    pub context_extra: serde_json::Value,
}

/// Executor registry entry — in-process or out-of-process.
pub enum ExecutorEntry {
    InProcess(Box<dyn crate::executor::ToolExecutor>),
    OutOfProcess(SpawnConfig),
}

/// Hook called after each successful tool invocation.
/// Used by distribution binaries to add domain-specific post-processing
/// (e.g., IOC extraction from tool output).
#[async_trait]
pub trait PostToolHook: Send + Sync {
    async fn on_tool_result(
        &self,
        tool_name: &str,
        output_json: &serde_json::Value,
        project_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<(), String>;
}
