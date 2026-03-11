use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

pub struct RizinDisasmExecutor {
    pub rizin_path: PathBuf,
}

use crate::common::{is_valid_hex_address, parse_last_json};

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

#[async_trait]
impl ToolExecutor for RizinDisasmExecutor {
    fn tool_name(&self) -> &str {
        "rizin.disasm"
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
        let length = input["length"]
            .as_u64()
            .ok_or_else(|| tool_err("invalid_input", "length is required".into()))?;

        // pdj = disassemble as JSON
        // aa = basic analysis (needed for function detection)
        let cmd_str = format!("aa;pdj {} @ {}", length, address);

        let output = Command::new(&self.rizin_path)
            .arg("-q")
            .arg("-c")
            .arg(&cmd_str)
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

        // Parse the JSON output (last JSON line — aa may produce text output)
        let output_json = parse_last_json(&stdout);

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
