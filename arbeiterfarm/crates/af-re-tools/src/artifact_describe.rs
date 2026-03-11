use async_trait::async_trait;
use af_plugin_api::{PluginDb, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ArtifactDescribeExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for ArtifactDescribeExecutor {
    fn tool_name(&self) -> &str {
        "artifact.describe"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let desc = input
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or("'description' is required and must be a string")?;
        if desc.is_empty() {
            return Err("'description' must not be empty".into());
        }
        if desc.len() > 1000 {
            return Err(format!(
                "'description' too long ({} chars, max 1000)",
                desc.len()
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact_id = input
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: "invalid_input".to_string(),
                message: "'artifact_id' is required".to_string(),
                retryable: false,
                details: Value::Null,
            })?;

        let description = input["description"].as_str().unwrap_or("");

        let rows_affected = self
            .plugin_db
            .execute_json(
                "UPDATE artifacts SET description = $2 WHERE id = $1::uuid AND project_id = $3::uuid",
                vec![json!(artifact_id), json!(description), json!(ctx.project_id.to_string())],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| ToolError {
                code: "db_error".to_string(),
                message: format!("failed to update description: {e}"),
                retryable: false,
                details: Value::Null,
            })?;

        if rows_affected == 0 {
            return Err(ToolError {
                code: "not_found".to_string(),
                message: format!("artifact {artifact_id} not found"),
                retryable: false,
                details: Value::Null,
            });
        }

        // Invalidate stale embeddings for this artifact (best-effort).
        // Embeddings based on the old description are now outdated.
        let stale_deleted = self
            .plugin_db
            .execute_json(
                "DELETE FROM embeddings WHERE artifact_id = $1::uuid AND project_id = $2::uuid",
                vec![json!(artifact_id), json!(ctx.project_id.to_string())],
                ctx.actor_user_id,
            )
            .await
            .unwrap_or(0);

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "status": "ok",
                "artifact_id": artifact_id,
                "description": description,
                "stale_embeddings_deleted": stale_deleted,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}
