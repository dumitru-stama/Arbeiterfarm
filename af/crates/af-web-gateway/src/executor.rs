use async_trait::async_trait;
use af_core::{ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn tool_err(code: &str, msg: String, retryable: bool) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable,
        details: Value::Null,
    }
}

// ---------------------------------------------------------------------------
// web.fetch executor
// ---------------------------------------------------------------------------

pub struct WebFetchExecutor {
    pub gateway_socket: PathBuf,
}

#[async_trait]
impl ToolExecutor for WebFetchExecutor {
    fn tool_name(&self) -> &str {
        "web.fetch"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or("'url' is required")?;
        if url.trim().is_empty() {
            return Err("'url' must not be empty".into());
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("'url' must start with http:// or https://".into());
        }
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let url = input["url"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'url' is required".into(), false))?;

        let request = json!({
            "action": "fetch",
            "url": url,
            "user_id": ctx.actor_user_id.map(|u| u.to_string()),
            "project_id": ctx.project_id.to_string(),
        });

        let response = gateway_call(&self.gateway_socket, &request).await?;

        if !response["ok"].as_bool().unwrap_or(false) {
            let error_code = response["error"].as_str().unwrap_or("unknown");
            let message = response["message"]
                .as_str()
                .unwrap_or("gateway error")
                .to_string();
            let retryable = matches!(error_code, "rate_limited" | "fetch_error" | "dns_error");
            return Err(tool_err(error_code, message, retryable));
        }

        let data = response["data"].clone();
        let cached = response["cached"].as_bool().unwrap_or(false);

        let mut output = data.clone();
        if let Some(obj) = output.as_object_mut() {
            obj.insert("cached".into(), json!(cached));
        }

        // Extract links if requested
        let extract_links = input
            .get("extract_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if extract_links {
            if let Some(body) = data["body"].as_str() {
                let links = extract_urls_from_text(body);
                if let Some(obj) = output.as_object_mut() {
                    obj.insert("links".into(), json!(links));
                }
            }
        }

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

// ---------------------------------------------------------------------------
// web.search executor
// ---------------------------------------------------------------------------

pub struct WebSearchExecutor {
    pub gateway_socket: PathBuf,
}

#[async_trait]
impl ToolExecutor for WebSearchExecutor {
    fn tool_name(&self) -> &str {
        "web.search"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("'query' is required")?;
        if query.trim().is_empty() {
            return Err("'query' must not be empty".into());
        }
        if query.len() > 200 {
            return Err("'query' must be at most 200 characters".into());
        }
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'query' is required".into(), false))?;

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_i64())
            .unwrap_or(10);

        let request = json!({
            "action": "search",
            "query": query,
            "user_id": ctx.actor_user_id.map(|u| u.to_string()),
        });

        let response = gateway_call(&self.gateway_socket, &request).await?;

        if !response["ok"].as_bool().unwrap_or(false) {
            let error_code = response["error"].as_str().unwrap_or("unknown");
            let message = response["message"]
                .as_str()
                .unwrap_or("gateway error")
                .to_string();
            let retryable = matches!(error_code, "rate_limited" | "search_error");
            return Err(tool_err(error_code, message, retryable));
        }

        let mut data = response["data"].clone();

        // Truncate results if max_results is specified
        let truncated_len = if let Some(results) = data.get_mut("results").and_then(|v| v.as_array_mut()) {
            results.truncate(max_results as usize);
            Some(results.len())
        } else {
            None
        };
        if let Some(len) = truncated_len {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("total".into(), json!(len));
            }
        }

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: data,
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Gateway UDS helper
// ---------------------------------------------------------------------------

async fn gateway_call(socket_path: &PathBuf, request: &Value) -> Result<Value, ToolError> {
    let stream = UnixStream::connect(socket_path).await.map_err(|e| {
        tool_err(
            "gateway_unavailable",
            format!("cannot connect to web gateway: {e}"),
            true,
        )
    })?;

    let (reader, mut writer) = stream.into_split();

    let json = serde_json::to_string(request).map_err(|e| {
        tool_err("serialize_error", format!("JSON encode failed: {e}"), false)
    })?;

    writer.write_all(json.as_bytes()).await.map_err(|e| {
        tool_err("io_error", format!("socket write failed: {e}"), true)
    })?;
    writer.write_all(b"\n").await.map_err(|e| {
        tool_err("io_error", format!("socket write failed: {e}"), true)
    })?;
    writer.shutdown().await.map_err(|e| {
        tool_err("io_error", format!("socket shutdown failed: {e}"), true)
    })?;

    let mut reader = BufReader::new(reader);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await.map_err(|e| {
        tool_err("io_error", format!("socket read failed: {e}"), true)
    })?;

    serde_json::from_str(response_line.trim()).map_err(|e| {
        tool_err(
            "parse_error",
            format!("gateway response parse failed: {e}"),
            false,
        )
    })
}

/// Simple URL extraction from text.
fn extract_urls_from_text(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for word in text.split_whitespace() {
        if (word.starts_with("http://") || word.starts_with("https://"))
            && word.len() > 10
        {
            // Clean trailing punctuation
            let cleaned = word.trim_end_matches(|c: char| c == '.' || c == ',' || c == ')' || c == ']');
            if !urls.contains(&cleaned.to_string()) {
                urls.push(cleaned.to_string());
            }
        }
    }
    urls
}
