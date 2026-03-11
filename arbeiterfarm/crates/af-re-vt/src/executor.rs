use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct VtFileReportExecutor {
    pub gateway_socket: PathBuf,
}

fn tool_err(code: &str, msg: String, retryable: bool) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable,
        details: Value::Null,
    }
}

#[async_trait]
impl ToolExecutor for VtFileReportExecutor {
    fn tool_name(&self) -> &str {
        "vt.file_report"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        _input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact = ctx
            .artifacts
            .first()
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into(), false))?;

        let sha256 = &artifact.sha256;

        // Connect to gateway via UDS
        let stream = tokio::net::UnixStream::connect(&self.gateway_socket)
            .await
            .map_err(|e| {
                tool_err(
                    "gateway_unavailable",
                    format!("VT gateway not running at {}: {e}", self.gateway_socket.display()),
                    true,
                )
            })?;

        let (reader, mut writer) = stream.into_split();

        // Send request (include user_id for per-user rate limiting in the gateway)
        let request = json!({
            "action": "file_report",
            "sha256": sha256,
            "user_id": ctx.actor_user_id.map(|u| u.to_string()),
        });
        let mut req_bytes = serde_json::to_vec(&request)
            .map_err(|e| tool_err("serialize_error", format!("failed to serialize request: {e}"), false))?;
        req_bytes.push(b'\n');

        writer.write_all(&req_bytes).await.map_err(|e| {
            tool_err("io_error", format!("failed to write to gateway: {e}"), true)
        })?;
        writer.shutdown().await.map_err(|e| {
            tool_err("io_error", format!("failed to shutdown write: {e}"), false)
        })?;

        // Read response
        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::new();
        buf_reader.read_line(&mut response_line).await.map_err(|e| {
            tool_err("io_error", format!("failed to read gateway response: {e}"), true)
        })?;

        let resp: Value = serde_json::from_str(&response_line)
            .map_err(|e| tool_err("parse_error", format!("failed to parse gateway response: {e}"), false))?;

        // Check response
        let ok = resp["ok"].as_bool().unwrap_or(false);
        if !ok {
            let error = resp["error"].as_str().unwrap_or("unknown");
            let message = resp["message"].as_str().unwrap_or("gateway error");
            return Err(tool_err(error, message.to_string(), error == "rate_limited" || error == "upstream_error"));
        }

        // Build result
        let data = resp.get("data").cloned().unwrap_or(Value::Null);
        let cached = resp["cached"].as_bool().unwrap_or(false);

        let output = if data.is_null() {
            json!({
                "found": false,
                "message": resp["message"].as_str().unwrap_or("Hash not found in VirusTotal database"),
                "sha256": sha256,
            })
        } else {
            json!({
                "found": true,
                "cached": cached,
                "report": data,
            })
        };

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: output,
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}
