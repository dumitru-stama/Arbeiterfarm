use async_trait::async_trait;
use af_plugin_api::{PluginDb, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

pub struct ArtifactSearchExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for ArtifactSearchExecutor {
    fn tool_name(&self) -> &str {
        "artifact.search"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("'query' is required and must be a string")?;
        if query.is_empty() {
            return Err("'query' must not be empty".into());
        }
        if query.len() > 200 {
            return Err(format!(
                "'query' too long ({} chars, max 200)",
                query.len()
            ));
        }

        if let Some(field) = input.get("field").and_then(|v| v.as_str()) {
            match field {
                "any" | "filename" | "description" | "sha256" | "mime_type" => {}
                _ => return Err(format!("invalid field '{field}': must be one of any, filename, description, sha256, mime_type")),
            }
        }

        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tool_err("invalid_input", "'query' is required".into()))?;

        let field = input
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or("any");

        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20)
            .clamp(1, 100);

        let pattern = format!("%{query}%");

        let where_clause = match field {
            "filename" => "filename ILIKE $1",
            "description" => "description ILIKE $1",
            "sha256" => "sha256 ILIKE $1",
            "mime_type" => "mime_type ILIKE $1",
            _ => "filename ILIKE $1 OR description ILIKE $1 OR sha256 ILIKE $1 OR mime_type ILIKE $1",
        };

        let sql = format!(
            "SELECT id, project_id, filename, description, sha256, mime_type, created_at \
             FROM artifacts WHERE ({where_clause}) \
             AND project_id IN (SELECT af_shareable_projects()) \
             ORDER BY created_at DESC LIMIT $2"
        );

        let rows = self
            .plugin_db
            .query_json(
                &sql,
                vec![json!(pattern), json!(limit)],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| tool_err("db_error", format!("artifact search failed: {e}")))?;

        let total = rows.len();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "total": total,
                "results": rows,
                "query": query,
                "field": field,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}
