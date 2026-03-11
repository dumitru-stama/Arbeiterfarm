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

pub struct DedupPriorAnalysisExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for DedupPriorAnalysisExecutor {
    fn tool_name(&self) -> &str {
        "dedup.prior_analysis"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let aid = input
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .ok_or("'artifact_id' is required and must be a string")?;
        if aid.is_empty() {
            return Err("'artifact_id' must not be empty".into());
        }
        // Basic UUID format check (36 chars with hyphens)
        if aid.len() != 36 || aid.chars().filter(|c| *c == '-').count() != 4 {
            return Err(format!("'artifact_id' does not look like a UUID: {aid}"));
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact_id = input["artifact_id"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'artifact_id' is required".into()))?;

        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(5)
            .clamp(1, 20);

        // Step 1: Get artifact's SHA256 from current project
        let sha_rows = self
            .plugin_db
            .query_json(
                "SELECT sha256 FROM artifacts WHERE id = $1::uuid AND project_id = $2::uuid",
                vec![json!(artifact_id), json!(ctx.project_id.to_string())],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| tool_err("db_error", format!("failed to look up artifact: {e}")))?;

        let sha256 = sha_rows
            .first()
            .and_then(|r| r.get("sha256"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| tool_err("not_found", format!("artifact {artifact_id} not found in current project")))?
            .to_string();

        // Step 2: Find matching artifacts in other shareable projects with family tags and thread counts.
        // Single query with correlated subqueries eliminates N+1 pattern.
        let matches = self
            .plugin_db
            .query_json(
                "SELECT a.id, a.project_id, a.filename, a.description, p.name as project_name, \
                   COALESCE( \
                     (SELECT json_agg(json_build_object('family_name', af.family_name, 'confidence', af.confidence)) \
                      FROM artifact_families af \
                      WHERE af.artifact_id = a.id AND af.project_id = a.project_id), \
                     '[]'::json \
                   ) as families, \
                   (SELECT COUNT(*)::int FROM threads t WHERE t.project_id = a.project_id) as project_thread_count \
                 FROM artifacts a \
                 JOIN projects p ON p.id = a.project_id \
                 WHERE a.sha256 = $1 \
                   AND a.project_id <> $2::uuid \
                   AND a.project_id IN (SELECT af_shareable_projects()) \
                 ORDER BY a.created_at DESC LIMIT $3",
                vec![json!(sha256), json!(ctx.project_id.to_string()), json!(limit)],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| tool_err("db_error", format!("cross-project lookup failed: {e}")))?;

        if matches.is_empty() {
            return Ok(ToolResult {
                kind: ToolOutputKind::InlineJson,
                output_json: json!({
                    "sha256": sha256,
                    "prior_analyses": [],
                    "message": "No prior analysis found for this binary in other accessible projects."
                }),
                stdout: None,
                stderr: None,
                produced_artifacts: vec![],
                primary_artifact: None,
                evidence: vec![],
            });
        }

        // Step 3: Build response from the single query results
        let prior_analyses: Vec<Value> = matches
            .iter()
            .map(|m| {
                json!({
                    "artifact_id": m.get("id"),
                    "project_id": m.get("project_id"),
                    "project_name": m.get("project_name"),
                    "filename": m.get("filename"),
                    "description": m.get("description"),
                    "families": m.get("families").unwrap_or(&json!([])),
                    "project_thread_count": m.get("project_thread_count").and_then(|v| v.as_i64()).unwrap_or(0),
                })
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "sha256": sha256,
                "prior_analyses": prior_analyses,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}
