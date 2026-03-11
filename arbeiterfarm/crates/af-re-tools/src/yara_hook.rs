//! Post-tool hook for YARA: persists generated rules and scan results to DB.

use af_plugin_api::{PluginDb, PostToolHook};
use serde_json::json;
use std::sync::Arc;

pub struct YaraPostToolHook {
    pub plugin_db: Arc<dyn PluginDb>,
}

#[async_trait::async_trait]
impl PostToolHook for YaraPostToolHook {
    async fn on_tool_result(
        &self,
        tool_name: &str,
        output_json: &serde_json::Value,
        project_id: uuid::Uuid,
        user_id: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        match tool_name {
            "yara.generate" => self.persist_rule(output_json, project_id, user_id).await,
            "yara.scan" => self.persist_scan_results(output_json, project_id, user_id).await,
            _ => Ok(()),
        }
    }
}

impl YaraPostToolHook {
    /// After yara.generate: upsert rule into re.yara_rules
    async fn persist_rule(
        &self,
        output: &serde_json::Value,
        project_id: uuid::Uuid,
        user_id: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        let valid = output["valid"].as_bool().unwrap_or(false);
        if !valid {
            return Ok(());
        }

        let rule_name = match output.get("rule_name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return Ok(()),
        };

        let rule_text = match output.get("rule_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return Ok(()),
        };

        let description = output
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tags: Vec<String> = output
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let _ = self
            .plugin_db
            .query_json(
                "INSERT INTO yara_rules (name, source, description, tags, project_id, created_by) \
                 VALUES ($1, $2, $3, $4::text[], $5::uuid, $6::uuid) \
                 ON CONFLICT (name, project_id) WHERE project_id IS NOT NULL \
                 DO UPDATE SET source = EXCLUDED.source, description = EXCLUDED.description, \
                 tags = EXCLUDED.tags, updated_at = now() \
                 RETURNING id",
                vec![
                    json!(rule_name),
                    json!(rule_text),
                    json!(description),
                    json!(tags),
                    json!(project_id.to_string()),
                    json!(user_id.map(|u| u.to_string())),
                ],
                user_id,
            )
            .await
            .map_err(|e| format!("failed to persist YARA rule: {e}"))?;

        let detail = json!({
            "project_id": project_id,
            "rule_name": rule_name,
        });
        let _ = self
            .plugin_db
            .audit_log("yara_rule_saved", user_id, Some(&detail))
            .await;

        Ok(())
    }

    /// After yara.scan: upsert matches into re.yara_scan_results
    async fn persist_scan_results(
        &self,
        output: &serde_json::Value,
        _project_id: uuid::Uuid,
        user_id: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        // Scan output has top-level artifact_id and rules array
        let artifact_id = match output.get("artifact_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return Ok(()), // No artifact_id in output, skip
        };

        let rules = match output.get("rules").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return Ok(()),
        };

        for rule in rules {
            let rule_name = match rule.get("rule").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => continue,
            };
            let match_count = rule
                .get("match_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as i64;
            let match_data = rule.get("matches").cloned().unwrap_or(json!([]));

            let _ = self
                .plugin_db
                .query_json(
                    "INSERT INTO yara_scan_results (artifact_id, rule_name, match_count, match_data) \
                     VALUES ($1::uuid, $2, $3, $4::jsonb) \
                     ON CONFLICT (artifact_id, rule_name) \
                     DO UPDATE SET match_count = EXCLUDED.match_count, \
                     match_data = EXCLUDED.match_data, matched_at = now() \
                     RETURNING id",
                    vec![
                        json!(artifact_id),
                        json!(rule_name),
                        json!(match_count),
                        json!(match_data),
                    ],
                    user_id,
                )
                .await
                .map_err(|e| {
                    eprintln!(
                        "[yara-hook] WARNING: failed to persist scan result for {rule_name}: {e}"
                    );
                    // Don't fail the whole hook for individual result persistence
                })
                .ok();
        }

        Ok(())
    }
}
