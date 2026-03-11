use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, PluginDb, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;

/// In-process executor: queries the database for function renames from other
/// non-NDA projects that analyzed the same binary (same SHA256). Returns
/// suggestions that the agent can then apply via `ghidra.rename`.
pub struct GhidraSuggestRenamesExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

#[async_trait]
impl ToolExecutor for GhidraSuggestRenamesExecutor {
    fn tool_name(&self) -> &str {
        "ghidra.suggest_renames"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        _input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact = ctx
            .artifacts
            .first()
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into()))?;

        let suggestions = crate::ghidra_renames_db::get_cross_project_renames(
            &self.plugin_db,
            &artifact.sha256,
            ctx.project_id,
            ctx.actor_user_id,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to query cross-project renames: {e}")))?;

        if suggestions.is_empty() {
            return Ok(ToolResult {
                kind: ToolOutputKind::InlineJson,
                output_json: json!({
                    "suggestions": [],
                    "sha256": artifact.sha256,
                    "note": "No renames found from other projects for this binary.",
                }),
                stdout: None,
                stderr: None,
                produced_artifacts: vec![],
                primary_artifact: None,
                evidence: vec![EvidenceRef::Artifact(artifact.id)],
            });
        }

        let suggestion_list: Vec<Value> = suggestions
            .iter()
            .map(|s| {
                let mut obj = json!({
                    "old_name": s.old_name,
                    "new_name": s.new_name,
                    "source_projects": s.project_names,
                    "source_count": s.source_count,
                });
                if let Some(ref addr) = s.address {
                    obj["address"] = json!(addr);
                }
                obj
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "suggestions": suggestion_list,
                "sha256": artifact.sha256,
                "total": suggestions.len(),
                "note": "Renames discovered from other non-NDA projects. Use ghidra.rename to apply desired suggestions.",
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}
