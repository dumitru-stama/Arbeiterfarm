use async_trait::async_trait;
use af_plugin_api::{
    OutputRedirectPolicy, SandboxProfile, ToolContext, ToolError, ToolExecutor, ToolOutputKind,
    ToolPolicy, ToolResult, ToolSpec,
};
use serde_json::json;

/// Echo tool spec — for Slice 1 end-to-end verification.
pub fn echo_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "echo.tool".to_string(),
        version: 1,
        deprecated: false,
        description: "Echo tool: reads input artifact metadata, writes echo_output.json".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": {
                "ArtifactId": {
                    "type": "string",
                    "format": "uuid",
                    "description": "UUID of an artifact in the current project"
                }
            },
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

/// Echo tool executor — reads artifact metadata, writes echo_output.json via OutputStore.
pub struct EchoToolExecutor;

#[async_trait]
impl ToolExecutor for EchoToolExecutor {
    fn tool_name(&self) -> &str {
        "echo.tool"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        _input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        // Build artifact summary from context
        let artifact_summaries: Vec<serde_json::Value> = ctx
            .artifacts
            .iter()
            .map(|a| {
                json!({
                    "id": a.id.to_string(),
                    "filename": a.filename,
                    "sha256": a.sha256,
                    "size_bytes": a.size_bytes,
                    "mime_type": a.mime_type,
                })
            })
            .collect();

        let echo_output = json!({
            "echo": "hello from echo.tool",
            "project_id": ctx.project_id.to_string(),
            "tool_run_id": ctx.tool_run_id.to_string(),
            "input_artifacts": artifact_summaries,
        });

        // Write output as an artifact via OutputStore
        let output_bytes = serde_json::to_vec_pretty(&echo_output).unwrap();
        let artifact_id = ctx
            .output_store
            .store("echo_output.json", &output_bytes, Some("application/json"))
            .await?;

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: echo_output,
            stdout: None,
            stderr: None,
            produced_artifacts: vec![artifact_id],
            primary_artifact: Some(artifact_id),
            evidence: Vec::new(),
        })
    }
}
