use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

pub struct RizinXrefsExecutor {
    pub rizin_path: PathBuf,
}

use crate::common::is_valid_hex_address;

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

#[async_trait]
impl ToolExecutor for RizinXrefsExecutor {
    fn tool_name(&self) -> &str {
        "rizin.xrefs"
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

        let address = input["address"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "address is required".into()))?;
        // Validate address format to prevent rizin command injection
        if !is_valid_hex_address(address) {
            return Err(tool_err(
                "invalid_input",
                format!("invalid address format: {address} — expected 0x[0-9a-fA-F]+"),
            ));
        }
        let direction = input
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("both");

        // axtj = xrefs TO this address (JSON)
        // axfj = xrefs FROM this address (JSON)
        let xref_cmd = match direction {
            "to" => format!("axtj @ {address}"),
            "from" => format!("axfj @ {address}"),
            _ => format!("axtj @ {address};axfj @ {address}"),
        };

        // aa = basic analysis (needed for xref detection)
        let full_cmd = format!("aa;{xref_cmd}");

        let output = Command::new(&self.rizin_path)
            .arg("-q")
            .arg("-c")
            .arg(&full_cmd)
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

        let output_json = parse_xrefs_output(&stdout, direction);

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json,
            stdout: Some(stdout.into_owned()),
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

/// Parse xrefs output. Analysis commands (aa) produce text before JSON results.
/// Only considers lines starting with `{` or `[` as JSON candidates.
pub fn parse_xrefs_output(stdout: &str, direction: &str) -> Value {
    let json_lines: Vec<Value> = stdout
        .lines()
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
        .collect();

    match direction {
        "to" => json!({ "xrefs_to": json_lines.first().cloned().unwrap_or(Value::Null) }),
        "from" => json!({ "xrefs_from": json_lines.first().cloned().unwrap_or(Value::Null) }),
        _ => json!({
            "xrefs_to": json_lines.first().cloned().unwrap_or(Value::Null),
            "xrefs_from": json_lines.get(1).cloned().unwrap_or(Value::Null),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_xrefs_to() {
        let stdout = "Analyzing...\n[{\"from\":4096,\"to\":8192}]\n";
        let result = parse_xrefs_output(stdout, "to");
        assert!(result["xrefs_to"].is_array());
        assert!(result.get("xrefs_from").is_none());
    }

    #[test]
    fn test_parse_xrefs_both() {
        let stdout = "aa done\n[{\"from\":1}]\n[{\"to\":2}]\n";
        let result = parse_xrefs_output(stdout, "both");
        assert!(result["xrefs_to"].is_array());
        assert!(result["xrefs_from"].is_array());
    }

    #[test]
    fn test_parse_xrefs_empty() {
        let result = parse_xrefs_output("no json", "both");
        assert!(result["xrefs_to"].is_null());
        assert!(result["xrefs_from"].is_null());
    }
}
