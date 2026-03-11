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

/// Normalize a family name: trim whitespace and lowercase.
pub fn normalize_family(name: &str) -> String {
    name.trim().to_lowercase()
}

// --- family.tag ---

pub struct FamilyTagExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for FamilyTagExecutor {
    fn tool_name(&self) -> &str {
        "family.tag"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let name = input
            .get("family_name")
            .and_then(|v| v.as_str())
            .ok_or("'family_name' is required")?;
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("'family_name' must not be empty".into());
        }
        if trimmed.len() > 100 {
            return Err("'family_name' must be 100 characters or less".into());
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
        let family_name = normalize_family(
            input["family_name"]
                .as_str()
                .ok_or_else(|| tool_err("invalid_input", "'family_name' is required".into()))?,
        );
        let confidence = input
            .get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        let notes = input.get("notes").and_then(|v| v.as_str());

        let rows = self
            .plugin_db
            .query_json(
                "INSERT INTO artifact_families (project_id, artifact_id, family_name, confidence, notes) \
                 VALUES ($1::uuid, $2::uuid, $3, $4, $5) \
                 ON CONFLICT (project_id, artifact_id, family_name) \
                 DO UPDATE SET confidence = EXCLUDED.confidence, notes = EXCLUDED.notes \
                 RETURNING id, family_name, confidence, notes, created_at",
                vec![
                    json!(ctx.project_id.to_string()),
                    json!(artifact_id),
                    json!(family_name),
                    json!(confidence),
                    json!(notes),
                ],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| tool_err("db_error", format!("family tag failed: {e}")))?;

        let row = rows.into_iter().next().unwrap_or(json!({}));

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "status": "tagged",
                "artifact_id": artifact_id,
                "family": row,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// --- family.list ---

pub struct FamilyListExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for FamilyListExecutor {
    fn tool_name(&self) -> &str {
        "family.list"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let family_name = input.get("family_name").and_then(|v| v.as_str()).map(normalize_family);
        let artifact_id = input.get("artifact_id").and_then(|v| v.as_str());
        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100);

        let rows = match (family_name.as_deref(), artifact_id) {
            (Some(fam), Some(aid)) => {
                self.plugin_db
                    .query_json(
                        "SELECT id, artifact_id, family_name, confidence, notes, tagged_by_agent, created_at \
                         FROM artifact_families \
                         WHERE project_id = $1::uuid AND family_name = $2 AND artifact_id = $3::uuid \
                         ORDER BY created_at DESC LIMIT $4",
                        vec![
                            json!(ctx.project_id.to_string()),
                            json!(fam),
                            json!(aid),
                            json!(limit),
                        ],
                        ctx.actor_user_id,
                    )
                    .await
            }
            (Some(fam), None) => {
                self.plugin_db
                    .query_json(
                        "SELECT id, artifact_id, family_name, confidence, notes, tagged_by_agent, created_at \
                         FROM artifact_families \
                         WHERE project_id = $1::uuid AND family_name = $2 \
                         ORDER BY created_at DESC LIMIT $3",
                        vec![
                            json!(ctx.project_id.to_string()),
                            json!(fam),
                            json!(limit),
                        ],
                        ctx.actor_user_id,
                    )
                    .await
            }
            (None, Some(aid)) => {
                self.plugin_db
                    .query_json(
                        "SELECT id, artifact_id, family_name, confidence, notes, tagged_by_agent, created_at \
                         FROM artifact_families \
                         WHERE project_id = $1::uuid AND artifact_id = $2::uuid \
                         ORDER BY created_at DESC LIMIT $3",
                        vec![
                            json!(ctx.project_id.to_string()),
                            json!(aid),
                            json!(limit),
                        ],
                        ctx.actor_user_id,
                    )
                    .await
            }
            (None, None) => {
                self.plugin_db
                    .query_json(
                        "SELECT id, artifact_id, family_name, confidence, notes, tagged_by_agent, created_at \
                         FROM artifact_families \
                         WHERE project_id = $1::uuid \
                         ORDER BY created_at DESC LIMIT $2",
                        vec![
                            json!(ctx.project_id.to_string()),
                            json!(limit),
                        ],
                        ctx.actor_user_id,
                    )
                    .await
            }
        }
        .map_err(|e| tool_err("db_error", format!("family list query failed: {e}")))?;

        let total = rows.len();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "total": total,
                "families": rows,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// --- family.search ---

pub struct FamilySearchExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for FamilySearchExecutor {
    fn tool_name(&self) -> &str {
        "family.search"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let name = input
            .get("family_name")
            .and_then(|v| v.as_str())
            .ok_or("'family_name' is required")?;
        if name.trim().is_empty() {
            return Err("'family_name' must not be empty".into());
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let family_name = normalize_family(
            input["family_name"]
                .as_str()
                .ok_or_else(|| tool_err("invalid_input", "'family_name' is required".into()))?,
        );
        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20);

        // Cross-project search — af_shareable_projects() enforces RLS + NDA + exclude_from_search.
        let rows = self
            .plugin_db
            .query_json(
                "SELECT af.id, af.project_id, af.artifact_id, af.family_name, af.confidence, af.notes, af.tagged_by_agent, af.created_at \
                 FROM artifact_families af \
                 WHERE af.family_name = $1 \
                   AND af.project_id IN (SELECT af_shareable_projects()) \
                 ORDER BY af.created_at DESC LIMIT $2",
                vec![json!(family_name), json!(limit)],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| tool_err("db_error", format!("family search failed: {e}")))?;

        let total = rows.len();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "search_family": family_name,
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

// --- family.untag ---

pub struct FamilyUntagExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolExecutor for FamilyUntagExecutor {
    fn tool_name(&self) -> &str {
        "family.untag"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let name = input
            .get("family_name")
            .and_then(|v| v.as_str())
            .ok_or("'family_name' is required")?;
        if name.trim().is_empty() {
            return Err("'family_name' must not be empty".into());
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
        let family_name = normalize_family(
            input["family_name"]
                .as_str()
                .ok_or_else(|| tool_err("invalid_input", "'family_name' is required".into()))?,
        );

        let affected = self
            .plugin_db
            .execute_json(
                "DELETE FROM artifact_families \
                 WHERE project_id = $1::uuid AND artifact_id = $2::uuid AND family_name = $3",
                vec![
                    json!(ctx.project_id.to_string()),
                    json!(artifact_id),
                    json!(family_name),
                ],
                ctx.actor_user_id,
            )
            .await
            .map_err(|e| tool_err("db_error", format!("family untag failed: {e}")))?;

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "status": if affected > 0 { "removed" } else { "not_found" },
                "artifact_id": artifact_id,
                "family_name": family_name,
                "rows_deleted": affected,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_family_lowercase() {
        assert_eq!(normalize_family("Emotet"), "emotet");
    }

    #[test]
    fn test_normalize_family_trim() {
        assert_eq!(normalize_family("  TrickBot  "), "trickbot");
    }

    #[test]
    fn test_normalize_family_mixed() {
        assert_eq!(normalize_family("  Cobalt Strike "), "cobalt strike");
    }

    #[test]
    fn test_normalize_family_already_normal() {
        assert_eq!(normalize_family("apt29"), "apt29");
    }

    #[test]
    fn test_normalize_family_empty() {
        assert_eq!(normalize_family(""), "");
    }
}
