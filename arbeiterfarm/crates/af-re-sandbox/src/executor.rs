use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn tool_err(code: &str, msg: String, retryable: bool) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable,
        details: Value::Null,
    }
}

/// Helper to send a request to the sandbox gateway over UDS and get a response.
async fn gateway_call(
    socket_path: &std::path::Path,
    request: Value,
) -> Result<Value, ToolError> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|e| {
            tool_err(
                "gateway_unavailable",
                format!(
                    "Sandbox gateway not running at {}: {e}",
                    socket_path.display()
                ),
                true,
            )
        })?;

    let (reader, mut writer) = stream.into_split();

    let mut req_bytes = serde_json::to_vec(&request)
        .map_err(|e| tool_err("serialize_error", format!("serialize: {e}"), false))?;
    req_bytes.push(b'\n');

    writer.write_all(&req_bytes).await.map_err(|e| {
        tool_err("io_error", format!("write to gateway: {e}"), true)
    })?;
    writer.shutdown().await.map_err(|e| {
        tool_err("io_error", format!("shutdown write: {e}"), false)
    })?;

    let mut buf_reader = BufReader::new(reader);
    let mut response_line = String::new();
    buf_reader
        .read_line(&mut response_line)
        .await
        .map_err(|e| {
            tool_err("io_error", format!("read gateway response: {e}"), true)
        })?;

    let resp: Value = serde_json::from_str(&response_line)
        .map_err(|e| tool_err("parse_error", format!("parse gateway response: {e}"), false))?;

    let ok = resp["ok"].as_bool().unwrap_or(false);
    if !ok {
        let error = resp["error"].as_str().unwrap_or("unknown");
        let message = resp["message"].as_str().unwrap_or("gateway error");
        return Err(tool_err(
            error,
            message.to_string(),
            error == "vm_error" || error == "agent_error",
        ));
    }

    Ok(resp)
}

// ---------------------------------------------------------------------------
// sandbox.trace executor
// ---------------------------------------------------------------------------

pub struct SandboxTraceExecutor {
    pub gateway_socket: PathBuf,
}

#[async_trait]
impl ToolExecutor for SandboxTraceExecutor {
    fn tool_name(&self) -> &str {
        "sandbox.trace"
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
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into(), false))?;

        let sample_bytes = tokio::fs::read(&artifact.storage_path)
            .await
            .map_err(|e| {
                tool_err("io_error", format!("read artifact: {e}"), false)
            })?;

        let sample_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &sample_bytes,
        );

        let timeout_secs = input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let args: Option<Vec<String>> = input
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let request = json!({
            "action": "trace",
            "sample_b64": sample_b64,
            "timeout_secs": timeout_secs,
            "args": args,
        });

        let resp = gateway_call(&self.gateway_socket, request).await?;
        let data = resp.get("data").cloned().unwrap_or(Value::Null);

        let trace = data.get("trace").cloned().unwrap_or(json!([]));
        let process_tree = data.get("process_tree").cloned().unwrap_or(json!([]));
        let errors = data.get("errors").cloned().unwrap_or(json!([]));

        let full_result = json!({
            "trace": trace,
            "process_tree": process_tree,
            "errors": errors,
        });

        // Store full trace as artifact via output_store
        let trace_bytes = serde_json::to_vec_pretty(&full_result).unwrap_or_default();
        let mut produced_artifacts = Vec::new();
        match ctx
            .output_store
            .store_with_description(
                "trace.json",
                &trace_bytes,
                Some("application/json"),
                &format!("API trace from sandbox execution (timeout={}s)", timeout_secs),
            )
            .await
        {
            Ok(artifact_id) => {
                produced_artifacts.push(artifact_id);
            }
            Err(e) => {
                tracing::warn!("[sandbox.trace] failed to store trace artifact: {}", e.message);
            }
        }

        // Build compact summary
        let trace_arr = trace.as_array();
        let trace_count = trace_arr.map(|a| a.len()).unwrap_or(0);

        let mut api_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        if let Some(entries) = trace_arr {
            for entry in entries {
                if let Some(api) = entry.get("api").and_then(|v| v.as_str()) {
                    *api_counts.entry(api.to_string()).or_default() += 1;
                }
            }
        }
        let mut sorted_apis: Vec<(String, usize)> = api_counts.into_iter().collect();
        sorted_apis.sort_by(|a, b| b.1.cmp(&a.1));
        let top_apis: Vec<Value> = sorted_apis
            .iter()
            .take(20)
            .map(|(name, count)| json!({"api": name, "count": count}))
            .collect();

        let summary = json!({
            "total_api_calls": trace_count,
            "unique_apis": sorted_apis.len(),
            "top_apis": top_apis,
            "process_tree": process_tree,
            "error_count": errors.as_array().map(|a| a.len()).unwrap_or(0),
            "hint": "Full API trace stored as artifact. Use file.grep to search for specific APIs.",
        });

        let primary = produced_artifacts.first().copied();
        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: summary,
            stdout: None,
            stderr: None,
            produced_artifacts,
            primary_artifact: primary,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}

// ---------------------------------------------------------------------------
// sandbox.hook executor
// ---------------------------------------------------------------------------

pub struct SandboxHookExecutor {
    pub gateway_socket: PathBuf,
}

#[async_trait]
impl ToolExecutor for SandboxHookExecutor {
    fn tool_name(&self) -> &str {
        "sandbox.hook"
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
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into(), false))?;

        let hook_script = input
            .get("hook_script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tool_err("invalid_input", "hook_script is required".into(), false))?;

        let sample_bytes = tokio::fs::read(&artifact.storage_path)
            .await
            .map_err(|e| {
                tool_err("io_error", format!("read artifact: {e}"), false)
            })?;

        let sample_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &sample_bytes,
        );

        let timeout_secs = input
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let args: Option<Vec<String>> = input
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let request = json!({
            "action": "hook",
            "sample_b64": sample_b64,
            "hook_script": hook_script,
            "timeout_secs": timeout_secs,
            "args": args,
        });

        let resp = gateway_call(&self.gateway_socket, request).await?;
        let data = resp.get("data").cloned().unwrap_or(Value::Null);

        let full_result = json!({
            "trace": data.get("trace").cloned().unwrap_or(json!([])),
            "process_tree": data.get("process_tree").cloned().unwrap_or(json!([])),
            "errors": data.get("errors").cloned().unwrap_or(json!([])),
            "data": data.get("data").cloned().unwrap_or(Value::Null),
        });

        let result_bytes = serde_json::to_vec_pretty(&full_result).unwrap_or_default();
        let mut produced_artifacts = Vec::new();
        match ctx
            .output_store
            .store_with_description(
                "hook_results.json",
                &result_bytes,
                Some("application/json"),
                "Custom Frida hook results from sandbox execution",
            )
            .await
        {
            Ok(artifact_id) => {
                produced_artifacts.push(artifact_id);
            }
            Err(e) => {
                tracing::warn!("[sandbox.hook] failed to store hook results artifact: {}", e.message);
            }
        }

        let trace_count = full_result["trace"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let error_count = full_result["errors"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);

        let summary = json!({
            "trace_entries": trace_count,
            "error_count": error_count,
            "has_custom_data": !full_result["data"].is_null(),
            "hint": "Full hook results stored as artifact. Use file.read_range to inspect.",
        });

        let primary = produced_artifacts.first().copied();
        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: summary,
            stdout: None,
            stderr: None,
            produced_artifacts,
            primary_artifact: primary,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}

// ---------------------------------------------------------------------------
// sandbox.screenshot executor
// ---------------------------------------------------------------------------

pub struct SandboxScreenshotExecutor {
    pub gateway_socket: PathBuf,
}

#[async_trait]
impl ToolExecutor for SandboxScreenshotExecutor {
    fn tool_name(&self) -> &str {
        "sandbox.screenshot"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        _ctx: ToolContext,
        _input: Value,
    ) -> Result<ToolResult, ToolError> {
        let request = json!({ "action": "screenshot" });
        let resp = gateway_call(&self.gateway_socket, request).await?;
        let data = resp.get("data").cloned().unwrap_or(Value::Null);

        let output = json!({
            "format": data.get("format").and_then(|v| v.as_str()).unwrap_or("ppm"),
            "image_b64": data.get("image_b64").and_then(|v| v.as_str()).unwrap_or(""),
            "size_bytes": data.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
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
