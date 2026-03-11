use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

pub struct StringsExtractExecutor {
    pub rizin_path: PathBuf,
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
impl ToolExecutor for StringsExtractExecutor {
    fn tool_name(&self) -> &str {
        "strings.extract"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact = ctx
            .artifacts
            .first()
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into()))?;

        let min_length = input
            .get("min_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(4) as usize;
        let max_strings = input
            .get("max_strings")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000) as usize;
        let encoding = input
            .get("encoding")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        // izzj = all strings including cross-section (JSON)
        let output = Command::new(&self.rizin_path)
            .arg("-q")
            .arg("-c")
            .arg("izzj")
            .arg(&artifact.storage_path)
            .output()
            .map_err(|e| tool_err("exec_failed", format!("failed to run rizin: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(tool_err(
                "rizin_error",
                format!("rizin exited with {}: {stderr}", output.status),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);

        let output_json = filter_strings(&stdout, min_length, max_strings, encoding);

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json,
            stdout: None,
            stderr: if stderr_str.is_empty() {
                None
            } else {
                Some(stderr_str.into_owned())
            },
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}

/// Filter rizin string output by min_length, encoding, and max count.
pub fn filter_strings(stdout: &str, min_length: usize, max_strings: usize, encoding: &str) -> Value {
    // Try to parse the last JSON line (rizin may output non-JSON before it).
    // Only consider lines starting with { or [ as JSON candidates.
    let parsed: Option<Value> = stdout
        .lines()
        .rev()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
                return None;
            }
            serde_json::from_str::<Value>(trimmed).ok()
        })
        .next();

    let strings_array = match parsed {
        Some(Value::Array(arr)) => arr,
        Some(Value::Object(obj)) => {
            // rizin sometimes wraps in {"strings": [...]}
            if let Some(Value::Array(arr)) = obj.get("strings") {
                arr.clone()
            } else {
                return json!({ "strings": [], "total": 0, "note": "unexpected rizin output format" });
            }
        }
        _ => {
            return json!({ "strings": [], "total": 0, "note": "no parseable output from rizin" });
        }
    };

    let filtered: Vec<&Value> = strings_array
        .iter()
        .filter(|s| {
            let str_val = s.get("string").and_then(|v| v.as_str()).unwrap_or("");
            let str_type = s.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let len_ok = str_val.len() >= min_length;
            let enc_ok = match encoding {
                "all" => true,
                "ascii" => str_type == "ascii" || str_type.is_empty(),
                "utf8" => str_type == "utf8" || str_type == "ascii" || str_type.is_empty(),
                "utf16le" => str_type == "utf16le",
                "utf16be" => str_type == "utf16be",
                _ => true,
            };
            len_ok && enc_ok
        })
        .take(max_strings)
        .collect();

    let total = filtered.len();

    json!({
        "strings": filtered,
        "total": total,
        "truncated": strings_array.len() > max_strings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_strings_basic() {
        let stdout = r#"[{"string":"hello world","type":"ascii"},{"string":"ab","type":"ascii"}]"#;
        let result = filter_strings(stdout, 4, 100, "all");
        assert_eq!(result["total"], 1); // "ab" is too short (len 2 < 4)
        assert_eq!(result["strings"][0]["string"], "hello world");
    }

    #[test]
    fn test_filter_strings_max_limit() {
        let stdout = r#"[{"string":"aaaa","type":"ascii"},{"string":"bbbb","type":"ascii"},{"string":"cccc","type":"ascii"}]"#;
        let result = filter_strings(stdout, 1, 2, "all");
        assert_eq!(result["total"], 2);
        assert_eq!(result["truncated"], true);
    }

    #[test]
    fn test_filter_strings_encoding_filter() {
        let stdout = r#"[{"string":"hello","type":"ascii"},{"string":"world","type":"utf16le"}]"#;
        let result = filter_strings(stdout, 1, 100, "utf16le");
        assert_eq!(result["total"], 1);
        assert_eq!(result["strings"][0]["string"], "world");
    }

    #[test]
    fn test_filter_strings_wrapped_object() {
        let stdout = r#"{"strings":[{"string":"test","type":"ascii"}]}"#;
        let result = filter_strings(stdout, 1, 100, "all");
        assert_eq!(result["total"], 1);
    }

    #[test]
    fn test_filter_strings_no_output() {
        let result = filter_strings("", 4, 100, "all");
        assert_eq!(result["total"], 0);
        assert!(result["note"].is_string());
    }
}
