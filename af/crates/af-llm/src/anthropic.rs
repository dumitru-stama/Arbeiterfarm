use async_trait::async_trait;
use af_core::{BackendCapabilities, ChatRole};
use futures::StreamExt;
use serde_json::json;
use tokio::sync::mpsc;

use crate::backend::LlmBackend;
use crate::error::LlmError;
use crate::request::{
    CompletionRequest, CompletionResponse, FinishReason, StreamChunk, ToolCallResponse, UsageInfo,
};

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Adaptive request pacer: starts fast, backs off on 429s.
/// Stores (next_allowed_time, current_interval).
/// Interval starts at 1s, doubles on each 429, resets to 1s on success.
static PACER_STATE: Mutex<Option<(Instant, u64)>> = Mutex::new(None);

/// Wait until our rate-limit slot, then reserve the next slot.
async fn pace_request() {
    let sleep_until = {
        let mut state = PACER_STATE.lock().unwrap();
        let now = Instant::now();
        let (allowed, interval_secs) = state.unwrap_or((now, 1));
        let interval = Duration::from_secs(interval_secs);
        if allowed > now {
            *state = Some((allowed + interval, interval_secs));
            allowed
        } else {
            *state = Some((now + interval, interval_secs));
            now
        }
    };

    let now = Instant::now();
    if sleep_until > now {
        let wait = sleep_until - now;
        eprintln!("[anthropic] pacing: waiting {:.1}s before request", wait.as_secs_f64());
        tokio::time::sleep(wait).await;
    }
}

/// Called on 429: double the pacer interval (capped at 60s).
fn pace_backoff() {
    let mut state = PACER_STATE.lock().unwrap();
    let (_, interval_secs) = state.unwrap_or((Instant::now(), 1));
    let new_interval = (interval_secs * 2).min(60);
    eprintln!("[anthropic] pacer backoff: interval now {}s", new_interval);
    *state = Some((Instant::now() + Duration::from_secs(new_interval), new_interval));
}

/// Called on success: reset the pacer interval to 1s.
fn pace_reset() {
    let mut state = PACER_STATE.lock().unwrap();
    if let Some((_, interval_secs)) = *state {
        if interval_secs > 1 {
            eprintln!("[anthropic] pacer reset: interval back to 1s");
        }
    }
    *state = Some((Instant::now(), 1));
}

/// Anthropic requires tool names to match `^[a-zA-Z0-9_-]{1,128}`.
/// Our tools use dots (e.g. `file.info`, `file.read_range`), so we sanitize outgoing
/// and reverse-map incoming.  We use `___` as separator since tool names never contain triple underscores.
fn sanitize_tool_name(name: &str) -> String {
    name.replace('.', "___")
}

/// Build a mapping from sanitized name → original name for all tools in the request.
fn build_name_map(request: &CompletionRequest) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for t in &request.tools {
        let sanitized = sanitize_tool_name(&t.name);
        if sanitized != t.name {
            map.insert(sanitized, t.name.clone());
        }
    }
    map
}

/// Anthropic Messages API backend.
pub struct AnthropicBackend {
    name: String,
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicBackend {
    pub fn new(name: String, api_key: String, model: String) -> Self {
        Self {
            name,
            api_key,
            model,
            client: reqwest::Client::builder()
                .read_timeout(std::time::Duration::from_secs(300))
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn build_messages(
        &self,
        request: &CompletionRequest,
    ) -> (Option<String>, Vec<serde_json::Value>) {
        let mut system_prompt = None;
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg.role {
                ChatRole::System => {
                    system_prompt = Some(msg.content.clone());
                }
                ChatRole::User => {
                    // Multi-modal: serialize content_parts as content array when present
                    if msg.has_parts() {
                        let parts: Vec<serde_json::Value> = msg.content_parts.as_ref().unwrap()
                            .iter()
                            .map(|p| match p {
                                af_core::ContentPart::Text { text } => json!({
                                    "type": "text",
                                    "text": text,
                                }),
                                af_core::ContentPart::Image { data, media_type } => json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": data,
                                    },
                                }),
                            })
                            .collect();
                        messages.push(json!({
                            "role": "user",
                            "content": parts,
                        }));
                    } else {
                        messages.push(json!({
                            "role": "user",
                            "content": &msg.content,
                        }));
                    }
                }
                ChatRole::Assistant => {
                    if msg.tool_calls.is_empty() {
                        messages.push(json!({
                            "role": "assistant",
                            "content": &msg.content,
                        }));
                    } else {
                        // Build content blocks: optional text + tool_use blocks
                        let mut content_blocks = Vec::new();
                        if !msg.content.is_empty() {
                            content_blocks.push(json!({
                                "type": "text",
                                "text": &msg.content,
                            }));
                        }
                        for tc in &msg.tool_calls {
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": &tc.id,
                                "name": sanitize_tool_name(&tc.name),
                                "input": &tc.arguments,
                            }));
                        }
                        messages.push(json!({
                            "role": "assistant",
                            "content": content_blocks,
                        }));
                    }
                }
                ChatRole::Tool => {
                    let tool_use_id = msg.tool_call_id.as_deref().unwrap_or("");
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": &msg.content,
                        }],
                    }));
                }
            }
        }

        // Merge consecutive user messages (required by Anthropic API)
        let mut merged: Vec<serde_json::Value> = Vec::new();
        for msg in messages {
            if msg["role"].as_str() == Some("user") {
                if let Some(last) = merged.last_mut() {
                    if last["role"].as_str() == Some("user") {
                        // Merge content into an array of blocks
                        let existing = last["content"].take();
                        let new_content = msg["content"].clone();
                        let mut blocks = match existing {
                            serde_json::Value::Array(arr) => arr,
                            serde_json::Value::String(s) => {
                                vec![json!({"type": "text", "text": s})]
                            }
                            other => vec![other],
                        };
                        match new_content {
                            serde_json::Value::Array(arr) => blocks.extend(arr),
                            serde_json::Value::String(s) => {
                                blocks.push(json!({"type": "text", "text": s}));
                            }
                            other => blocks.push(other),
                        }
                        last["content"] = serde_json::Value::Array(blocks);
                        continue;
                    }
                }
            }
            merged.push(msg);
        }

        // Fix orphaned tool_use blocks: Anthropic requires every assistant message
        // containing tool_use blocks to be immediately followed by a user message
        // containing the matching tool_result blocks.  When multiple agents share a
        // thread, messages can interleave and break this invariant.
        let merged = fix_orphaned_tool_use(merged);

        (system_prompt, merged)
    }

    fn build_tools(&self, request: &CompletionRequest) -> Option<Vec<serde_json::Value>> {
        if request.tools.is_empty() {
            return None;
        }
        Some(
            request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": sanitize_tool_name(&t.name),
                        "description": &t.description,
                        "input_schema": &t.parameters,
                    })
                })
                .collect(),
        )
    }

    fn build_body(&self, request: &CompletionRequest, stream: bool) -> serde_json::Value {
        let (system_prompt, messages) = self.build_messages(request);

        // Debug: dump message structure for troubleshooting Anthropic 400 errors
        for (i, msg) in messages.iter().enumerate() {
            let role = msg["role"].as_str().unwrap_or("?");
            let content = &msg["content"];
            if let Some(arr) = content.as_array() {
                let types: Vec<&str> = arr
                    .iter()
                    .map(|b| b.get("type").and_then(|v| v.as_str()).unwrap_or("?"))
                    .collect();
                eprintln!("[anthropic] msg[{}] role={} blocks={:?}", i, role, types);
                // For tool_result blocks, show tool_use_id
                for b in arr {
                    if b.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        eprintln!(
                            "[anthropic]   tool_result -> tool_use_id={}",
                            b.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("?")
                        );
                    }
                    if b.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        eprintln!(
                            "[anthropic]   tool_use -> id={} name={}",
                            b.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
                            b.get("name").and_then(|v| v.as_str()).unwrap_or("?")
                        );
                    }
                }
            } else {
                let preview = content
                    .as_str()
                    .map(|s| if s.len() > 80 { &s[..80] } else { s })
                    .unwrap_or("(non-string)");
                eprintln!("[anthropic] msg[{}] role={} content={}", i, role, preview);
            }
        }

        let mut body = json!({
            "model": &self.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        if stream {
            body["stream"] = json!(true);
        }
        if let Some(system) = &system_prompt {
            body["system"] = json!(system);
        }
        if let Some(tools) = self.build_tools(request) {
            body["tools"] = json!(tools);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        body
    }
}

#[async_trait]
impl LlmBackend for AnthropicBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> BackendCapabilities {
        let meta = crate::model_catalog::lookup(&self.model);
        BackendCapabilities {
            supports_tool_calls: true,
            supports_streaming: true,
            is_local: false,
            context_window: meta.map(|m| m.context_window),
            max_output_tokens: meta.map(|m| m.max_output_tokens),
            cost_per_mtok_input: meta.map(|m| m.cost_per_mtok_input),
            cost_per_mtok_output: meta.map(|m| m.cost_per_mtok_output),
            supports_vision: meta.map(|m| m.supports_vision),
            knowledge_cutoff: meta.and_then(|m| m.knowledge_cutoff.map(|s| s.to_string())),
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let name_map = build_name_map(&request);
        let body = self.build_body(&request, false);

        // Retry with exponential backoff for rate limits (429)
        let max_retries = 3u32;

        for attempt in 0..=max_retries {
            // Pace requests to avoid hitting rate limits
            pace_request().await;

            let resp = self
                .client
                .post(ANTHROPIC_API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::Http(e.to_string()))?;

            let status = resp.status().as_u16();
            if status == 429 && attempt < max_retries {
                pace_backoff();
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(15));
                eprintln!(
                    "[anthropic] rate limited (attempt {}/{}), waiting {:.0}s...",
                    attempt + 1,
                    max_retries,
                    wait.as_secs_f64()
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            if status >= 400 {
                let text = resp.text().await.unwrap_or_default();
                return Err(LlmError::Api {
                    status,
                    message: text,
                });
            }

            pace_reset();

            let resp_json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| LlmError::JsonParse(e.to_string()))?;

            let mut response = parse_anthropic_response(&resp_json)?;
            for tc in &mut response.tool_calls {
                if let Some(original) = name_map.get(&tc.name) {
                    tc.name = original.clone();
                }
            }
            return Ok(response);
        }

        Err(LlmError::Api {
            status: 429,
            message: "rate limited after max retries".to_string(),
        })
    }

    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        tx: mpsc::Sender<Result<StreamChunk, LlmError>>,
    ) -> Result<(), LlmError> {
        let name_map = build_name_map(&request);
        let body = self.build_body(&request, true);

        // Retry with backoff for rate limits (429)
        let max_retries = 3u32;
        let mut resp = None;

        for attempt in 0..=max_retries {
            // Pace requests to avoid hitting rate limits
            pace_request().await;

            let r = self
                .client
                .post(ANTHROPIC_API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::Http(e.to_string()))?;

            let status = r.status().as_u16();
            if status == 429 && attempt < max_retries {
                pace_backoff();
                let wait = r
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(15));
                eprintln!(
                    "[anthropic] rate limited (attempt {}/{}), waiting {:.0}s...",
                    attempt + 1,
                    max_retries,
                    wait.as_secs_f64()
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            if status >= 400 {
                let text = r.text().await.unwrap_or_default();
                return Err(LlmError::Api {
                    status,
                    message: text,
                });
            }

            pace_reset();
            resp = Some(r);
            break;
        }

        let resp = resp.ok_or_else(|| LlmError::Api {
            status: 429,
            message: "rate limited after max retries".to_string(),
        })?;

        let mut byte_stream = resp.bytes_stream();
        let mut buffer = String::new();

        // Current content block being streamed
        let mut current_tool_id = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result.map_err(|e| LlmError::Http(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                // Parse SSE event type and data
                if let Some(data) = line.strip_prefix("data: ") {
                    let data = data.trim();
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        let event_type = event
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        match event_type {
                            "content_block_start" => {
                                if let Some(block) = event.get("content_block") {
                                    let block_type =
                                        block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                    if block_type == "tool_use" {
                                        let id = block
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let mut name = block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        // Reverse-map sanitized name back to original
                                        if let Some(original) = name_map.get(&name) {
                                            name = original.clone();
                                        }
                                        current_tool_id = id.clone();
                                        let _ = tx
                                            .send(Ok(StreamChunk::ToolCallStart { id, name }))
                                            .await;
                                    }
                                }
                            }
                            "content_block_delta" => {
                                if let Some(delta) = event.get("delta") {
                                    let delta_type =
                                        delta.get("type").and_then(|v| v.as_str()).unwrap_or("");

                                    match delta_type {
                                        "text_delta" => {
                                            if let Some(text) =
                                                delta.get("text").and_then(|v| v.as_str())
                                            {
                                                if !text.is_empty() {
                                                    let _ = tx
                                                        .send(Ok(StreamChunk::Token(
                                                            text.to_string(),
                                                        )))
                                                        .await;
                                                }
                                            }
                                        }
                                        "input_json_delta" => {
                                            if let Some(json_frag) = delta
                                                .get("partial_json")
                                                .and_then(|v| v.as_str())
                                            {
                                                if !json_frag.is_empty() {
                                                    let _ = tx
                                                        .send(Ok(StreamChunk::ToolCallDelta {
                                                            id: current_tool_id.clone(),
                                                            arguments_delta: json_frag
                                                                .to_string(),
                                                        }))
                                                        .await;
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "message_delta" => {
                                if let Some(delta) = event.get("delta") {
                                    if let Some(stop) =
                                        delta.get("stop_reason").and_then(|v| v.as_str())
                                    {
                                        let reason = match stop {
                                            "end_turn" => FinishReason::Stop,
                                            "tool_use" => FinishReason::ToolUse,
                                            "max_tokens" => FinishReason::Length,
                                            other => FinishReason::Unknown(other.to_string()),
                                        };
                                        let _ = tx.send(Ok(StreamChunk::Done(reason))).await;
                                    }
                                }
                                // Usage in message_delta
                                if let Some(usage) = event.get("usage") {
                                    let out = usage
                                        .get("output_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                    if out > 0 {
                                        let _ = tx
                                            .send(Ok(StreamChunk::Usage(UsageInfo {
                                                prompt_tokens: 0,
                                                completion_tokens: out as u32,
                                                cached_read_tokens: 0,
                                                cache_creation_tokens: 0,
                                            })))
                                            .await;
                                    }
                                }
                            }
                            "message_start" => {
                                // Usage from message_start
                                if let Some(message) = event.get("message") {
                                    if let Some(usage) = message.get("usage") {
                                        let input = usage
                                            .get("input_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        let cache_read = usage
                                            .get("cache_read_input_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        let cache_creation = usage
                                            .get("cache_creation_input_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        if input > 0 || cache_read > 0 || cache_creation > 0 {
                                            let _ = tx
                                                .send(Ok(StreamChunk::Usage(UsageInfo {
                                                    prompt_tokens: input as u32,
                                                    completion_tokens: 0,
                                                    cached_read_tokens: cache_read as u32,
                                                    cache_creation_tokens: cache_creation as u32,
                                                })))
                                                .await;
                                        }
                                    }
                                }
                            }
                            _ => {} // content_block_stop, message_stop, ping
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn parse_anthropic_response(resp: &serde_json::Value) -> Result<CompletionResponse, LlmError> {
    let content_blocks = resp
        .get("content")
        .and_then(|v| v.as_array())
        .ok_or_else(|| LlmError::JsonParse("no content array in response".into()))?;

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in content_blocks {
        match block.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    text_parts.push(text.to_string());
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                tool_calls.push(ToolCallResponse {
                    id,
                    name,
                    arguments: input,
                });
            }
            _ => {}
        }
    }

    let finish_reason = match resp.get("stop_reason").and_then(|v| v.as_str()) {
        Some("end_turn") => FinishReason::Stop,
        Some("tool_use") => FinishReason::ToolUse,
        Some("max_tokens") => FinishReason::Length,
        Some(other) => FinishReason::Unknown(other.to_string()),
        None => FinishReason::Stop,
    };

    let usage = resp.get("usage").map(|u| UsageInfo {
        prompt_tokens: u
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        completion_tokens: u
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cached_read_tokens: u
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cache_creation_tokens: u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
    });

    Ok(CompletionResponse {
        content: text_parts.join(""),
        tool_calls,
        finish_reason,
        usage,
    })
}

/// Fix orphaned tool_use blocks in the Anthropic message history.
///
/// Anthropic requires that every assistant message containing `tool_use` blocks
/// is immediately followed by a user message containing matching `tool_result` blocks.
/// When multiple agents share a thread (parallel workflow), their messages interleave
/// and can break this invariant.
///
/// This function scans the message array and, for each assistant message with tool_use
/// blocks, ensures the next user message has matching tool_results.  Any missing
/// tool_results get synthetic entries injected.
fn fix_orphaned_tool_use(messages: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = Vec::with_capacity(messages.len());

    for msg in messages {
        // Before pushing a new message, check if the previous message was an
        // assistant with tool_use that still needs tool_results.
        if let Some(prev) = result.last() {
            if prev["role"].as_str() == Some("assistant") {
                let tool_use_ids = extract_tool_use_ids(prev);
                if !tool_use_ids.is_empty() {
                    // The next message MUST be a user with matching tool_results.
                    // Check if the current message satisfies this.
                    let provided_ids = extract_tool_result_ids(&msg);
                    let missing: Vec<&str> = tool_use_ids
                        .iter()
                        .filter(|id| !provided_ids.contains(id.as_str()))
                        .map(|s| s.as_str())
                        .collect();

                    if !missing.is_empty() {
                        // We need to inject synthetic tool_results.
                        // If current msg is a user message, merge into it.
                        // Otherwise, insert a new user message before current.
                        let synthetic_blocks: Vec<serde_json::Value> = missing
                            .iter()
                            .map(|id| {
                                json!({
                                    "type": "tool_result",
                                    "tool_use_id": id,
                                    "content": "[result unavailable — concurrent agent execution]",
                                })
                            })
                            .collect();

                        if msg["role"].as_str() == Some("user") {
                            // We'll merge synthetic blocks into the current msg below
                            let mut merged_content = synthetic_blocks;
                            match msg["content"].clone() {
                                serde_json::Value::Array(arr) => merged_content.extend(arr),
                                serde_json::Value::String(s) => {
                                    merged_content.push(json!({"type": "text", "text": s}));
                                }
                                other => merged_content.push(other),
                            }
                            result.push(json!({
                                "role": "user",
                                "content": merged_content,
                            }));
                            continue;
                        } else {
                            // Insert a synthetic user message before the current one
                            result.push(json!({
                                "role": "user",
                                "content": synthetic_blocks,
                            }));
                        }
                    }
                }
            }
        }
        result.push(msg);
    }

    // Handle trailing assistant message with tool_use at the end of the array
    if let Some(last) = result.last() {
        if last["role"].as_str() == Some("assistant") {
            let tool_use_ids = extract_tool_use_ids(last);
            if !tool_use_ids.is_empty() {
                let synthetic_blocks: Vec<serde_json::Value> = tool_use_ids
                    .iter()
                    .map(|id| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": "[result unavailable — concurrent agent execution]",
                        })
                    })
                    .collect();
                result.push(json!({
                    "role": "user",
                    "content": synthetic_blocks,
                }));
            }
        }
    }

    // PASS 2: Strip orphaned tool_results — tool_result blocks whose tool_use_id
    // does NOT appear in the immediately preceding assistant message.
    // Anthropic requires every tool_result to reference a tool_use from the previous
    // assistant turn.
    let mut cleaned: Vec<serde_json::Value> = Vec::with_capacity(result.len());
    for msg in result {
        if msg["role"].as_str() == Some("user") {
            if let Some(content) = msg["content"].as_array() {
                // Collect tool_use IDs from the preceding assistant message
                let prev_tool_use_ids: HashSet<String> = cleaned
                    .last()
                    .filter(|prev| prev["role"].as_str() == Some("assistant"))
                    .map(|prev| extract_tool_use_ids(prev).into_iter().collect())
                    .unwrap_or_default();

                // Only filter if there are tool_result blocks in this message
                let has_tool_results = content
                    .iter()
                    .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_result"));

                if has_tool_results && !prev_tool_use_ids.is_empty() {
                    // Keep only tool_results that match the previous assistant's tool_use IDs,
                    // plus all non-tool_result blocks
                    let filtered: Vec<serde_json::Value> = content
                        .iter()
                        .filter(|block| {
                            if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                                let id = block
                                    .get("tool_use_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                prev_tool_use_ids.contains(id)
                            } else {
                                true
                            }
                        })
                        .cloned()
                        .collect();

                    if filtered.is_empty() {
                        // All content was orphaned tool_results — drop the entire message
                        continue;
                    }

                    cleaned.push(json!({
                        "role": "user",
                        "content": filtered,
                    }));
                    continue;
                } else if has_tool_results && prev_tool_use_ids.is_empty() {
                    // Previous message is NOT an assistant with tool_use — all tool_results
                    // in this user message are orphaned. Keep only non-tool_result blocks.
                    let filtered: Vec<serde_json::Value> = content
                        .iter()
                        .filter(|block| {
                            block.get("type").and_then(|v| v.as_str()) != Some("tool_result")
                        })
                        .cloned()
                        .collect();

                    if filtered.is_empty() {
                        continue;
                    }

                    cleaned.push(json!({
                        "role": "user",
                        "content": filtered,
                    }));
                    continue;
                }
            }
        }
        cleaned.push(msg);
    }

    // PASS 3: Re-merge consecutive same-role messages that may have been created
    // by stripping orphaned blocks in pass 2.
    let mut final_result: Vec<serde_json::Value> = Vec::with_capacity(cleaned.len());
    for msg in cleaned {
        if let Some(last) = final_result.last_mut() {
            if last["role"].as_str() == msg["role"].as_str() {
                // Merge content arrays
                let existing = last["content"].take();
                let new_content = msg["content"].clone();
                let mut blocks = match existing {
                    serde_json::Value::Array(arr) => arr,
                    serde_json::Value::String(s) => {
                        vec![json!({"type": "text", "text": s})]
                    }
                    other => vec![other],
                };
                match new_content {
                    serde_json::Value::Array(arr) => blocks.extend(arr),
                    serde_json::Value::String(s) => {
                        blocks.push(json!({"type": "text", "text": s}));
                    }
                    other => blocks.push(other),
                }
                last["content"] = serde_json::Value::Array(blocks);
                continue;
            }
        }
        final_result.push(msg);
    }

    final_result
}

/// Extract tool_use IDs from an assistant message's content blocks.
fn extract_tool_use_ids(msg: &serde_json::Value) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                if let Some(id) = block.get("id").and_then(|v| v.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
    }
    ids
}

/// Extract tool_result tool_use_ids from a user message's content blocks.
fn extract_tool_result_ids(msg: &serde_json::Value) -> HashSet<String> {
    let mut ids = HashSet::new();
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return ids;
    }
    if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                if let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    ids
}
