use crate::types::ArtifactRef;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use uuid::Uuid;

/// Everything a tool executor needs, provided by the job runner.
pub struct ToolContext {
    pub project_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub tool_run_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub artifacts: Vec<ArtifactRef>,
    pub scratch_dir: PathBuf,
    pub output_store: Box<dyn OutputStore>,
    pub clock: Box<dyn Clock>,
    pub core_config: CoreConfig,
    pub plugin_config: serde_json::Value,
    pub tool_config: serde_json::Value,
}

/// Handle for tools to store large outputs as artifacts.
#[async_trait]
pub trait OutputStore: Send + Sync {
    async fn store(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
    ) -> Result<Uuid, crate::ToolError>;

    /// Store an artifact with additional metadata (e.g. repivot tracking).
    /// Default implementation ignores metadata and delegates to `store()`.
    async fn store_with_metadata(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
        _metadata: serde_json::Value,
    ) -> Result<Uuid, crate::ToolError> {
        self.store(filename, data, mime_type).await
    }

    /// Store an artifact with a human-readable description.
    /// Default implementation ignores description and delegates to `store()`.
    async fn store_with_description(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
        _description: &str,
    ) -> Result<Uuid, crate::ToolError> {
        self.store(filename, data, mime_type).await
    }
}

/// Build metadata JSON for a repivot artifact.
pub fn repivot_metadata(original_artifact_id: Uuid) -> serde_json::Value {
    serde_json::json!({ "repivot_from": original_artifact_id.to_string() })
}

/// Build metadata JSON for a fan-out child artifact.
pub fn fanout_metadata(parent_artifact_id: Uuid) -> serde_json::Value {
    serde_json::json!({ "fan_out_from": parent_artifact_id.to_string() })
}

/// Testable clock abstraction.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Production clock.
pub struct UtcClock;

impl Clock for UtcClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Framework-owned configuration.
#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub storage_root: PathBuf,
    pub scratch_root: PathBuf,
    /// Use OAIE sandbox instead of bubblewrap (--oaie CLI flag).
    pub use_oaie: bool,
}
