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

// --- re-ioc.list ---

pub struct IocListExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for IocListExecutor {
    fn tool_name(&self) -> &str {
        "re-ioc.list"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let ioc_type = input
            .get("ioc_type")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100);

        let rows = if ioc_type == "all" {
            self.plugin_db
                .query_json(
                    "SELECT id, ioc_type, value, context, created_at FROM iocs \
                     WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2",
                    vec![
                        json!(ctx.project_id.to_string()),
                        json!(limit),
                    ],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("IOC query failed: {e}")))?
        } else {
            self.plugin_db
                .query_json(
                    "SELECT id, ioc_type, value, context, created_at FROM iocs \
                     WHERE project_id = $1 AND ioc_type = $3 \
                     ORDER BY created_at DESC LIMIT $2",
                    vec![
                        json!(ctx.project_id.to_string()),
                        json!(limit),
                        json!(ioc_type),
                    ],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("IOC query failed: {e}")))?
        };

        let total = rows.len();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "total": total,
                "iocs": rows,
                "filter": ioc_type,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// --- re-ioc.pivot ---

pub struct IocPivotExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for IocPivotExecutor {
    fn tool_name(&self) -> &str {
        "re-ioc.pivot"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let value = input
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or("'value' is required")?;
        if value.is_empty() {
            return Err("'value' must not be empty".into());
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let value = input["value"].as_str().unwrap_or("");
        let ioc_type = input
            .get("ioc_type")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        let rows = if ioc_type == "all" {
            self.plugin_db
                .query_json(
                    "SELECT id, ioc_type, value, source_tool_run, context, created_at \
                     FROM iocs WHERE project_id = $1 AND value = $2 \
                     ORDER BY created_at DESC",
                    vec![
                        json!(ctx.project_id.to_string()),
                        json!(value),
                    ],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("pivot query failed: {e}")))?
        } else {
            self.plugin_db
                .query_json(
                    "SELECT id, ioc_type, value, source_tool_run, context, created_at \
                     FROM iocs WHERE project_id = $1 AND value = $2 AND ioc_type = $3 \
                     ORDER BY created_at DESC",
                    vec![
                        json!(ctx.project_id.to_string()),
                        json!(value),
                        json!(ioc_type),
                    ],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("pivot query failed: {e}")))?
        };

        let total = rows.len();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "pivot_value": value,
                "pivot_type": ioc_type,
                "total_matches": total,
                "matches": rows,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// --- re-ioc.search (cross-project) ---

pub struct IocSearchExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for IocSearchExecutor {
    fn tool_name(&self) -> &str {
        "re-ioc.search"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let value = input
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or("'value' is required")?;
        if value.is_empty() {
            return Err("'value' must not be empty".into());
        }
        if value.len() > 500 {
            return Err(format!(
                "'value' too long ({} chars, max 500)",
                value.len()
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let value = input["value"].as_str().unwrap_or("");
        let ioc_type = input
            .get("ioc_type")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20)
            .clamp(1, 100);

        let rows = if ioc_type == "all" {
            self.plugin_db
                .query_json(
                    "SELECT i.id, i.project_id, i.ioc_type, i.value, i.context, i.created_at \
                     FROM iocs i \
                     WHERE i.value = $1 \
                       AND i.project_id IN (SELECT af_shareable_projects()) \
                     ORDER BY i.created_at DESC LIMIT $2",
                    vec![json!(value), json!(limit)],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("IOC search failed: {e}")))?
        } else {
            self.plugin_db
                .query_json(
                    "SELECT i.id, i.project_id, i.ioc_type, i.value, i.context, i.created_at \
                     FROM iocs i \
                     WHERE i.value = $1 AND i.ioc_type = $3 \
                       AND i.project_id IN (SELECT af_shareable_projects()) \
                     ORDER BY i.created_at DESC LIMIT $2",
                    vec![json!(value), json!(limit), json!(ioc_type)],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("IOC search failed: {e}")))?
        };

        let total = rows.len();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "search_value": value,
                "search_type": ioc_type,
                "total": total,
                "results": rows,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}
