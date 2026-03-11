use crate::context::ToolContext;
use crate::types::{ToolError, ToolRequest, ToolResult};
use async_trait::async_trait;

/// Implemented by plugins. The core job runner calls this.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn tool_name(&self) -> &str;
    fn tool_version(&self) -> u32;

    /// Semantic validation — called AFTER JSON Schema validation passes.
    fn validate(&self, _ctx: &ToolContext, _input: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// Run the tool.
    async fn execute(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError>;
}

/// The abstraction af-agents uses to run tools.
#[async_trait]
pub trait ToolInvoker: Send + Sync {
    async fn invoke(&self, request: ToolRequest) -> Result<ToolResult, ToolError>;
}

/// Hook to enrich tool_config before execution. Plugins can inject per-invocation
/// data (e.g. NDA flags, renames) without the SDK crate knowing plugin-specific details.
#[async_trait]
pub trait ToolConfigHook: Send + Sync {
    async fn enrich(
        &self,
        tool_name: &str,
        project_id: uuid::Uuid,
        artifacts: &[crate::ArtifactRef],
        tool_config: &mut serde_json::Value,
    );
}
