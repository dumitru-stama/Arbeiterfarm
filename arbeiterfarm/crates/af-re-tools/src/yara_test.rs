//! YARA test executor — InProcess (Trusted, needs DB + exec).
//!
//! Tests a YARA rule against multiple artifacts by scope:
//! - project: all uploaded artifacts
//! - artifact: single artifact by ID
//! - artifact_type: filter by MIME type pattern

use async_trait::async_trait;
use af_plugin_api::{PluginDb, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

pub struct YaraTestExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
    pub yara_path: PathBuf,
    pub yara_rules_dir: Option<PathBuf>,
}

#[async_trait]
impl ToolExecutor for YaraTestExecutor {
    fn tool_name(&self) -> &str {
        "yara.test"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        validate_test_input(input)
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let scope = input["scope"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'scope' is required".into()))?;

        let has_rule_text = input.get("rule_text").and_then(|v| v.as_str()).is_some();
        let has_rule_artifact = input
            .get("rule_artifact_id")
            .and_then(|v| v.as_str())
            .is_some();

        // Get rule text — either inline or from artifact
        let rule_text = if has_rule_text {
            input["rule_text"].as_str().unwrap().to_string()
        } else if has_rule_artifact {
            let rule_artifact_id = input["rule_artifact_id"].as_str().unwrap();
            // Read the rule artifact from storage
            let rows = self
                .plugin_db
                .query_json(
                    "SELECT storage_path FROM artifacts WHERE id = $1::uuid AND project_id = $2::uuid",
                    vec![json!(rule_artifact_id), json!(ctx.project_id.to_string())],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("failed to query artifact: {e}")))?;

            let row = rows
                .first()
                .ok_or_else(|| tool_err("not_found", "rule artifact not found".into()))?;
            let storage_path = row["storage_path"]
                .as_str()
                .ok_or_else(|| tool_err("db_error", "missing storage_path".into()))?;
            std::fs::read_to_string(storage_path)
                .map_err(|e| tool_err("io_error", format!("failed to read rule file: {e}")))?
        } else {
            return Err(tool_err(
                "invalid_input",
                "provide either 'rule_text' or 'rule_artifact_id'".into(),
            ));
        };

        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as i64;

        // Query target artifacts by scope
        let artifacts = match scope {
            "project" => {
                self.plugin_db
                    .query_json(
                        "SELECT id, filename, storage_path, mime_type \
                         FROM artifacts \
                         WHERE project_id = $1::uuid AND source_tool_run_id IS NULL \
                         ORDER BY created_at DESC LIMIT $2",
                        vec![json!(ctx.project_id.to_string()), json!(limit)],
                        ctx.actor_user_id,
                    )
                    .await
                    .map_err(|e| tool_err("db_error", format!("failed to query artifacts: {e}")))?
            }
            "artifact" => {
                let artifact_id = input["artifact_id"]
                    .as_str()
                    .ok_or_else(|| tool_err("invalid_input", "'artifact_id' required for scope='artifact'".into()))?;
                self.plugin_db
                    .query_json(
                        "SELECT id, filename, storage_path, mime_type \
                         FROM artifacts \
                         WHERE id = $1::uuid AND project_id = $2::uuid",
                        vec![json!(artifact_id), json!(ctx.project_id.to_string())],
                        ctx.actor_user_id,
                    )
                    .await
                    .map_err(|e| tool_err("db_error", format!("failed to query artifact: {e}")))?
            }
            "artifact_type" => {
                let mime_pattern = input["mime_pattern"]
                    .as_str()
                    .ok_or_else(|| tool_err("invalid_input", "'mime_pattern' required for scope='artifact_type'".into()))?;
                self.plugin_db
                    .query_json(
                        "SELECT id, filename, storage_path, mime_type \
                         FROM artifacts \
                         WHERE project_id = $1::uuid AND source_tool_run_id IS NULL \
                         AND mime_type ILIKE $2 \
                         ORDER BY created_at DESC LIMIT $3",
                        vec![
                            json!(ctx.project_id.to_string()),
                            json!(format!("%{mime_pattern}%")),
                            json!(limit),
                        ],
                        ctx.actor_user_id,
                    )
                    .await
                    .map_err(|e| tool_err("db_error", format!("failed to query artifacts: {e}")))?
            }
            _ => {
                return Err(tool_err(
                    "invalid_input",
                    format!("invalid scope: {scope}"),
                ))
            }
        };

        if artifacts.is_empty() {
            return Ok(ToolResult {
                kind: ToolOutputKind::InlineJson,
                output_json: json!({
                    "scope": scope,
                    "artifacts_tested": 0,
                    "message": "no artifacts matched the scope criteria",
                }),
                stdout: None,
                stderr: None,
                produced_artifacts: vec![],
                primary_artifact: None,
                evidence: vec![],
            });
        }

        // Write rule to temp file
        let rule_path = ctx.scratch_dir.join("test_rule.yar");
        std::fs::write(&rule_path, &rule_text)
            .map_err(|e| tool_err("io_error", format!("failed to write rule file: {e}")))?;

        // Test against each artifact
        let mut results: Vec<Value> = Vec::new();
        let mut total_matched = 0usize;

        for art in &artifacts {
            let art_id = art["id"].as_str().unwrap_or("?");
            let filename = art["filename"].as_str().unwrap_or("?");
            let storage_path = match art["storage_path"].as_str() {
                Some(p) => p,
                None => continue,
            };

            let output = std::process::Command::new(&self.yara_path)
                .arg("-s")
                .arg("--timeout=30")
                .arg(&rule_path)
                .arg(storage_path)
                .output();

            let (matched, rule_matches) = match output {
                Ok(o) => {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    if stdout.trim().is_empty() {
                        (false, json!(null))
                    } else {
                        let parsed = crate::yara_scan::parse_yara_output(&stdout);
                        let count = parsed["total_rules_matched"].as_u64().unwrap_or(0);
                        (count > 0, parsed)
                    }
                }
                Err(e) => {
                    results.push(json!({
                        "artifact_id": art_id,
                        "filename": filename,
                        "matched": false,
                        "error": format!("yara execution failed: {e}"),
                    }));
                    continue;
                }
            };

            if matched {
                total_matched += 1;
            }
            results.push(json!({
                "artifact_id": art_id,
                "filename": filename,
                "matched": matched,
                "details": rule_matches,
            }));
        }

        // Build inline summary with per-artifact results
        let matched_files: Vec<&str> = results.iter()
            .filter(|r| r["matched"].as_bool().unwrap_or(false))
            .filter_map(|r| r["filename"].as_str())
            .collect();
        let unmatched_files: Vec<&str> = results.iter()
            .filter(|r| !r["matched"].as_bool().unwrap_or(false))
            .filter_map(|r| r["filename"].as_str())
            .take(10)
            .collect();

        let summary = json!({
            "scope": scope,
            "artifacts_tested": results.len(),
            "artifacts_matched": total_matched,
            "match_rate": if results.is_empty() {
                "0%".to_string()
            } else {
                format!("{}%", total_matched * 100 / results.len())
            },
            "matched_files": matched_files,
            "unmatched_files": unmatched_files,
        });

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: summary,
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

/// Validate yara.test input parameters.
pub fn validate_test_input(input: &Value) -> Result<(), String> {
    let scope = input
        .get("scope")
        .and_then(|v| v.as_str())
        .ok_or("'scope' is required")?;

    let has_rule_text = input.get("rule_text").and_then(|v| v.as_str()).is_some();
    let has_rule_artifact = input
        .get("rule_artifact_id")
        .and_then(|v| v.as_str())
        .is_some();

    if !has_rule_text && !has_rule_artifact {
        return Err("provide either 'rule_text' or 'rule_artifact_id'".into());
    }
    if has_rule_text && has_rule_artifact {
        return Err("provide 'rule_text' OR 'rule_artifact_id', not both".into());
    }

    match scope {
        "artifact" => {
            if input.get("artifact_id").and_then(|v| v.as_str()).is_none() {
                return Err("'artifact_id' is required when scope='artifact'".into());
            }
        }
        "artifact_type" => {
            if input
                .get("mime_pattern")
                .and_then(|v| v.as_str())
                .is_none()
            {
                return Err("'mime_pattern' is required when scope='artifact_type'".into());
            }
        }
        "project" => {}
        _ => return Err(format!("invalid scope: {scope}")),
    }

    Ok(())
}

/// Build a match matrix from test results (used in tests).
pub fn build_match_matrix(results: &[Value]) -> Value {
    json!(results
        .iter()
        .map(|r| {
            json!({
                "artifact_id": r["artifact_id"],
                "filename": r["filename"],
                "matched": r["matched"],
            })
        })
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_scope_requires_rule_source() {
        let input = json!({"scope": "project"});
        assert!(validate_test_input(&input).is_err());
        assert!(validate_test_input(&input)
            .unwrap_err()
            .contains("rule_text"));
    }

    #[test]
    fn test_validate_mutual_exclusion() {
        let input = json!({
            "scope": "project",
            "rule_text": "rule test { condition: true }",
            "rule_artifact_id": "00000000-0000-0000-0000-000000000001",
        });
        assert!(validate_test_input(&input).is_err());
        assert!(validate_test_input(&input).unwrap_err().contains("not both"));
    }

    #[test]
    fn test_validate_artifact_scope_requires_id() {
        let input = json!({
            "scope": "artifact",
            "rule_text": "rule test { condition: true }",
        });
        assert!(validate_test_input(&input).is_err());
        assert!(validate_test_input(&input)
            .unwrap_err()
            .contains("artifact_id"));
    }

    #[test]
    fn test_validate_type_scope_requires_pattern() {
        let input = json!({
            "scope": "artifact_type",
            "rule_text": "rule test { condition: true }",
        });
        assert!(validate_test_input(&input).is_err());
        assert!(validate_test_input(&input)
            .unwrap_err()
            .contains("mime_pattern"));
    }

    #[test]
    fn test_build_match_matrix() {
        let results = vec![
            json!({"artifact_id": "aaa", "filename": "sample1.exe", "matched": true, "details": null}),
            json!({"artifact_id": "bbb", "filename": "sample2.dll", "matched": false, "details": null}),
            json!({"artifact_id": "ccc", "filename": "sample3.bin", "matched": true, "details": null}),
        ];
        let matrix = build_match_matrix(&results);
        let arr = matrix.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["matched"], true);
        assert_eq!(arr[1]["matched"], false);
        assert_eq!(arr[2]["matched"], true);
    }
}
