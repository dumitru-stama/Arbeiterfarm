//! YARA list executor — InProcess (Trusted, needs DB).
//!
//! Lists available YARA rules from three sources:
//! 1. Filesystem: .yar/.yara files from the rules directory
//! 2. DB: re.yara_rules for the current project + global rules
//! 3. Artifacts: .yar extension artifacts in the project

use async_trait::async_trait;
use af_plugin_api::{PluginDb, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

pub struct YaraListExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
    pub yara_rules_dir: Option<PathBuf>,
}

#[async_trait]
impl ToolExecutor for YaraListExecutor {
    fn tool_name(&self) -> &str {
        "yara.list"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let filter = input.get("filter").and_then(|v| v.as_str()).unwrap_or("");
        let source = input
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        let mut all_rules: Vec<Value> = Vec::new();

        // 1. Filesystem rules
        if source == "all" || source == "filesystem" {
            let fs_rules = list_rules_from_dir(self.yara_rules_dir.as_deref());
            for rule in fs_rules {
                if filter.is_empty()
                    || rule["name"]
                        .as_str()
                        .map_or(false, |n| n.contains(filter))
                {
                    all_rules.push(rule);
                }
            }
        }

        // 2. DB rules (project + global)
        if source == "all" || source == "db" {
            let rows = self
                .plugin_db
                .query_json(
                    "SELECT id, name, description, tags, project_id, created_at \
                     FROM yara_rules \
                     WHERE (project_id = $1::uuid OR project_id IS NULL) \
                     ORDER BY name",
                    vec![json!(ctx.project_id.to_string())],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("failed to query yara_rules: {e}")))?;

            for row in rows {
                let name = row["name"].as_str().unwrap_or("?");
                if filter.is_empty() || name.contains(filter) {
                    all_rules.push(json!({
                        "name": name,
                        "source": "db",
                        "description": row.get("description").and_then(|v| v.as_str()),
                        "tags": row.get("tags"),
                        "scope": if row.get("project_id").and_then(|v| v.as_str()).is_some() {
                            "project"
                        } else {
                            "global"
                        },
                        "created_at": row.get("created_at").and_then(|v| v.as_str()),
                    }));
                }
            }
        }

        // 3. Artifact rules (.yar files in project)
        if source == "all" || source == "artifacts" {
            let rows = self
                .plugin_db
                .query_json(
                    "SELECT id, filename, description, created_at \
                     FROM artifacts \
                     WHERE project_id = $1::uuid \
                     AND (filename LIKE '%.yar' OR filename LIKE '%.yara') \
                     ORDER BY filename",
                    vec![json!(ctx.project_id.to_string())],
                    ctx.actor_user_id,
                )
                .await
                .map_err(|e| tool_err("db_error", format!("failed to query artifacts: {e}")))?;

            for row in rows {
                let filename = row["filename"].as_str().unwrap_or("?");
                let name = filename
                    .strip_suffix(".yar")
                    .or_else(|| filename.strip_suffix(".yara"))
                    .unwrap_or(filename);
                if filter.is_empty() || name.contains(filter) {
                    all_rules.push(json!({
                        "name": name,
                        "source": "artifact",
                        "artifact_id": row.get("id").and_then(|v| v.as_str()),
                        "filename": filename,
                        "description": row.get("description").and_then(|v| v.as_str()),
                        "created_at": row.get("created_at").and_then(|v| v.as_str()),
                    }));
                }
            }
        }

        let output = json!({
            "total": all_rules.len(),
            "filter": if filter.is_empty() { None } else { Some(filter) },
            "source": source,
            "rules": all_rules,
        });

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: output,
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

/// List YARA rules from a directory, returning metadata for each file.
pub fn list_rules_from_dir(dir: Option<&Path>) -> Vec<Value> {
    let dir = match dir {
        Some(d) if d.is_dir() => d,
        _ => return vec![],
    };

    let mut rules = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ext != "yar" && ext != "yara" {
                continue;
            }

            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            let name = filename
                .strip_suffix(".yar")
                .or_else(|| filename.strip_suffix(".yara"))
                .unwrap_or(&filename)
                .to_string();

            let rule_count = path
                .as_path()
                .to_str()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|content| count_rules_in_file(&content))
                .unwrap_or(0);

            rules.push(json!({
                "name": name,
                "source": "filesystem",
                "filename": filename,
                "path": path.to_string_lossy(),
                "rule_count": rule_count,
            }));
        }
    }
    rules.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    rules
}

/// Count `rule <name>` declarations in a file, ignoring comments.
pub fn count_rules_in_file(content: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            // Skip comments
            if trimmed.starts_with("//") || trimmed.starts_with("/*") {
                return false;
            }
            trimmed.starts_with("rule ")
                || trimmed.starts_with("private rule ")
                || trimmed.starts_with("global rule ")
        })
        .count()
}

/// Filter rules by substring match on name.
pub fn filter_rules(rules: &[Value], filter: &str) -> Vec<Value> {
    if filter.is_empty() {
        return rules.to_vec();
    }
    rules
        .iter()
        .filter(|r| {
            r["name"]
                .as_str()
                .map_or(false, |n| n.contains(filter))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_rules_in_file_multiple() {
        let content = "\
rule one { condition: true }
rule two { condition: true }
private rule three { condition: true }
";
        assert_eq!(count_rules_in_file(content), 3);
    }

    #[test]
    fn test_count_rules_in_file_comments() {
        let content = "\
// rule fake_comment { condition: true }
rule real { condition: true }
/* rule another_fake { condition: true } */
";
        assert_eq!(count_rules_in_file(content), 1);
    }

    #[test]
    fn test_count_rules_in_file_empty() {
        assert_eq!(count_rules_in_file(""), 0);
        assert_eq!(count_rules_in_file("   \n\n  "), 0);
    }

    #[test]
    fn test_filter_rules_substring() {
        let rules = vec![
            json!({"name": "detect_emotet", "source": "fs"}),
            json!({"name": "detect_trickbot", "source": "fs"}),
            json!({"name": "packer_upx", "source": "db"}),
        ];
        let filtered = filter_rules(&rules, "detect");
        assert_eq!(filtered.len(), 2);

        let filtered2 = filter_rules(&rules, "upx");
        assert_eq!(filtered2.len(), 1);

        let filtered3 = filter_rules(&rules, "");
        assert_eq!(filtered3.len(), 3);
    }
}
