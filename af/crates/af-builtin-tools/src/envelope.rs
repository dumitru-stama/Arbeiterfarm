use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Envelope sent to the OOP executor binary on stdin.
#[derive(Debug, Serialize, Deserialize)]
pub struct OopEnvelope {
    pub tool_name: String,
    pub tool_version: u32,
    pub input: serde_json::Value,
    pub context: OopContext,
}

/// Execution context for the OOP executor.
#[derive(Debug, Serialize, Deserialize)]
pub struct OopContext {
    pub project_id: Uuid,
    pub tool_run_id: Uuid,
    pub scratch_dir: PathBuf,
    pub artifacts: Vec<OopArtifact>,
    /// The user who initiated the tool invocation (for per-user rate limiting).
    #[serde(default)]
    pub actor_user_id: Option<Uuid>,
    /// Tool-specific configuration from SpawnConfig.context_extra.
    /// Examples: {"rizin_path": "/usr/bin/rizin"} or {"ghidra_home": "/opt/ghidra"}.
    #[serde(default)]
    pub extra: serde_json::Value,
}

/// Artifact reference within the OOP envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct OopArtifact {
    pub id: Uuid,
    pub sha256: String,
    pub filename: String,
    pub storage_path: PathBuf,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}

/// Response from the OOP executor binary on stdout.
#[derive(Debug, Serialize, Deserialize)]
pub struct OopResponse {
    pub result: OopResult,
}

/// Result payload within the response.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum OopResult {
    #[serde(rename = "ok")]
    Ok {
        output: serde_json::Value,
        produced_files: Vec<ProducedFile>,
    },
    #[serde(rename = "error")]
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

/// A file produced by the tool, written to scratch dir.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProducedFile {
    pub filename: String,
    pub path: PathBuf,
    pub mime_type: Option<String>,
    /// Human-readable description stored on the artifact for LLM context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Handshake response for --handshake flag.
#[derive(Debug, Serialize, Deserialize)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub supported_tools: Vec<SupportedTool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SupportedTool {
    pub name: String,
    pub version: u32,
}
