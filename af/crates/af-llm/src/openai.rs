use async_trait::async_trait;
use af_core::BackendCapabilities;
use futures::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::backend::LlmBackend;
use crate::error::LlmError;
use crate::request::{
    CompletionRequest, CompletionResponse, FinishReason, StreamChunk, ToolCallResponse, UsageInfo,
};

/// OpenAI requires function names to match `^[a-zA-Z0-9_-]+$`.
/// Our tools use dots (e.g. `file.info`, `rizin.disasm`), so we sanitize outgoing
/// and reverse-map incoming.  We use `___` as separator since tool names never contain triple underscores.
fn sanitize_tool_name(name: &str) -> String {
    name.replace('.', "___")
}

fn build_name_map(request: &CompletionRequest) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for t in &request.tools {
        let sanitized = sanitize_tool_name(&t.name);
        if sanitized != t.name {
            map.insert(sanitized, t.name.clone());
        }
    }
    for msg in &request.messages {
        for tc in &msg.tool_calls {
            let sanitized = sanitize_tool_name(&tc.name);
            if sanitized != tc.name {
                map.insert(sanitized, tc.name.clone());
            }
        }
    }
    map
}

fn unsanitize_tool_name(name: &str, name_map: &HashMap<String, String>) -> String {
    name_map.get(name).cloned().unwrap_or_else(|| name.to_string())
}

/// Cloud OpenAI backend — for api.openai.com and other strict OpenAI-compatible APIs.
/// Sanitizes tool names (dot→___), sends `stream_options`, uses `max_completion_tokens`.
pub struct OpenAiBackend {
    name: String,
    endpoint: String,
    api_key: Option<String>,
    model: String,
    client: reqwest::Client,
}

impl OpenAiBackend {
    pub fn new(
        name: String,
        endpoint: String,
        api_key: Option<String>,
        model: String,
    ) -> Self {
        Self {
            name,
            endpoint,
            api_key,
            model,
            client: reqwest::Client::builder()
                .read_timeout(std::time::Duration::from_secs(300))
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn build_messages(&self, request: &CompletionRequest) -> Vec<serde_json::Value> {
        request
            .messages
            .iter()
            .map(|msg| {
                let content_value = if msg.has_parts() {
                    let parts: Vec<serde_json::Value> = msg.content_parts.as_ref().unwrap()
                        .iter()
                        .map(|p| match p {
                            af_core::ContentPart::Text { text } => json!({
                                "type": "text",
                                "text": text,
                            }),
                            af_core::ContentPart::Image { data, media_type } => json!({
                                "type": "image_url",
                                "image_url": {
                                    "url": format!("data:{media_type};base64,{data}"),
                                },
                            }),
                        })
                        .collect();
                    json!(parts)
                } else {
                    json!(&msg.content)
                };
                let mut m = json!({
                    "role": msg.role.as_str(),
                    "content": content_value,
                });
                if let Some(ref id) = msg.tool_call_id {
                    m["tool_call_id"] = json!(id);
                }
                if let Some(ref name) = msg.name {
                    m["name"] = json!(name);
                }
                if !msg.tool_calls.is_empty() {
                    let tool_calls: Vec<serde_json::Value> = msg
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": &tc.id,
                                "type": "function",
                                "function": {
                                    "name": sanitize_tool_name(&tc.name),
                                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                                }
                            })
                        })
                        .collect();
                    m["tool_calls"] = json!(tool_calls);
                }
                m
            })
            .collect()
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
                        "type": "function",
                        "function": {
                            "name": sanitize_tool_name(&t.name),
                            "description": &t.description,
                            "parameters": &t.parameters,
                        }
                    })
                })
                .collect(),
        )
    }

    fn build_body(&self, request: &CompletionRequest, stream: bool) -> serde_json::Value {
        let messages = self.build_messages(request);
        let mut body = json!({
            "model": &self.model,
            "messages": messages,
        });

        if stream {
            body["stream"] = json!(true);
            body["stream_options"] = json!({"include_usage": true});
        }
        if let Some(tools) = self.build_tools(request) {
            body["tools"] = json!(tools);
        }
        if let Some(max_tokens) = request.max_tokens {
            body["max_completion_tokens"] = json!(max_tokens);
        }
        if let Some(temperature) = request.temperature {
            let spec = crate::model_catalog::lookup(&self.model);
            match spec.and_then(|s| s.temperature_range) {
                Some((min, max)) if (max - min).abs() < f32::EPSILON => {
                    // Fixed temperature (e.g. gpt-5-nano, o3) — omit parameter
                }
                Some((min, max)) => {
                    body["temperature"] = json!(temperature.clamp(min, max));
                }
                None => {
                    body["temperature"] = json!(temperature);
                }
            }
        }
        body
    }
}

#[async_trait]
impl LlmBackend for OpenAiBackend {
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
        let url = format!("{}/v1/chat/completions", self.endpoint.trim_end_matches('/'));
        let body = self.build_body(&request, false);

        let mut req_builder = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        let resp_json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::JsonParse(e.to_string()))?;

        let name_map = build_name_map(&request);
        parse_response(&resp_json, &name_map)
    }

    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        tx: mpsc::Sender<Result<StreamChunk, LlmError>>,
    ) -> Result<(), LlmError> {
        let url = format!("{}/v1/chat/completions", self.endpoint.trim_end_matches('/'));
        let body = self.build_body(&request, true);

        log_request(&self.model, &self.endpoint, &request, &body);

        let name_map = build_name_map(&request);

        let mut req_builder = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        eprintln!("[llm-response] HTTP status={status}");
        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            eprintln!("[llm-response] ERROR status={status} body={}", text.chars().take(1000).collect::<String>());
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        stream_sse(resp, tx, &name_map).await
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (used by both OpenAiBackend and OllamaBackend)
// ---------------------------------------------------------------------------

pub(crate) fn log_request(
    model: &str,
    endpoint: &str,
    request: &CompletionRequest,
    body: &serde_json::Value,
) {
    let msg_count = request.messages.len();
    let tool_count = request.tools.len();
    let body_str = serde_json::to_string(body).unwrap_or_default();
    eprintln!(
        "[llm-request] model={} endpoint={} messages={} tools={} body_bytes={}",
        model, endpoint, msg_count, tool_count, body_str.len()
    );

    if let Some(sys) = request.messages.first() {
        let sys_preview: String = sys.content.chars().take(500).collect();
        eprintln!("[llm-request] system_prompt (first 500 chars): {sys_preview}");
    }

    let start = if msg_count > 3 { msg_count - 3 } else { 0 };
    for (i, msg) in request.messages[start..].iter().enumerate() {
        let content_preview: String = msg.content.chars().take(300).collect();
        let tc_info = if msg.tool_calls.is_empty() {
            String::new()
        } else {
            format!(
                " tool_calls={:?}",
                msg.tool_calls.iter().map(|tc| &tc.name).collect::<Vec<_>>()
            )
        };
        let tc_id_info = msg
            .tool_call_id
            .as_ref()
            .map(|id| format!(" tool_call_id={id}"))
            .unwrap_or_default();
        eprintln!(
            "[llm-request] msg[{}] role={}{}{} content({}): {}",
            start + i,
            msg.role.as_str(),
            tc_info,
            tc_id_info,
            msg.content.len(),
            content_preview
        );
    }

    if !request.tools.is_empty() {
        let names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
        eprintln!("[llm-request] tools: {:?}", names);
    }

    let dump_path = "/tmp/af_llm_last_request.json";
    if let Ok(pretty) = serde_json::to_string_pretty(body) {
        let _ = std::fs::write(dump_path, &pretty);
        eprintln!("[llm-request] full body dumped to {dump_path}");
    }
}

/// Stream SSE events from a response, parsing tool calls with an optional name_map for unsanitization.
pub(crate) async fn stream_sse(
    resp: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, LlmError>>,
    name_map: &HashMap<String, String>,
) -> Result<(), LlmError> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut active_tool_calls: HashMap<usize, (String, String)> = HashMap::new();
    let mut in_think = false;

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = chunk_result.map_err(|e| LlmError::Http(e.to_string()))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(());
                }

                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                    let chunks =
                        parse_sse_event(&event, &mut active_tool_calls, &mut in_think, name_map);
                    for sc in chunks {
                        if tx.send(Ok(sc)).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Parse a single SSE event from an OpenAI-compatible streaming response.
pub(crate) fn parse_sse_event(
    event: &serde_json::Value,
    active_tool_calls: &mut HashMap<usize, (String, String)>,
    in_think: &mut bool,
    name_map: &HashMap<String, String>,
) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();

    let Some(choice) = event.get("choices").and_then(|c| c.get(0)) else {
        if let Some(usage) = event.get("usage") {
            chunks.push(StreamChunk::Usage(parse_usage(usage)));
        }
        return chunks;
    };

    let delta = choice.get("delta").unwrap_or(choice);

    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
        if !content.is_empty() {
            classify_think_content(content, in_think, &mut chunks);
        }
    }
    if let Some(reasoning) = delta.get("reasoning").and_then(|v| v.as_str()) {
        if !reasoning.is_empty() {
            chunks.push(StreamChunk::Reasoning(reasoning.to_string()));
        }
    }

    if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            if let Some(function) = tc.get("function") {
                if let Some(raw_name) = function.get("name").and_then(|v| v.as_str()) {
                    let id = tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = unsanitize_tool_name(raw_name, name_map);
                    active_tool_calls.insert(index, (id.clone(), String::new()));
                    chunks.push(StreamChunk::ToolCallStart {
                        id: id.clone(),
                        name,
                    });
                }

                if let Some(args_frag) = function.get("arguments").and_then(|v| v.as_str()) {
                    if !args_frag.is_empty() {
                        if let Some((id, _acc)) = active_tool_calls.get(&index) {
                            chunks.push(StreamChunk::ToolCallDelta {
                                id: id.clone(),
                                arguments_delta: args_frag.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    if let Some(finish) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        let reason = match finish {
            "stop" => FinishReason::Stop,
            "tool_calls" => FinishReason::ToolUse,
            "length" => FinishReason::Length,
            other => FinishReason::Unknown(other.to_string()),
        };
        chunks.push(StreamChunk::Done(reason));
    }

    if let Some(usage) = event.get("usage") {
        let info = parse_usage(usage);
        if info.prompt_tokens > 0 || info.completion_tokens > 0 {
            chunks.push(StreamChunk::Usage(info));
        }
    }

    chunks
}

/// Classify content that may contain `<think>...</think>` inline reasoning (Qwen3-style).
pub(crate) fn classify_think_content(content: &str, in_think: &mut bool, chunks: &mut Vec<StreamChunk>) {
    let mut remaining = content;

    while !remaining.is_empty() {
        if *in_think {
            if let Some(end_pos) = remaining.find("</think>") {
                let reasoning = &remaining[..end_pos];
                if !reasoning.is_empty() {
                    chunks.push(StreamChunk::Reasoning(reasoning.to_string()));
                }
                *in_think = false;
                remaining = &remaining[(end_pos + 8)..];
            } else {
                chunks.push(StreamChunk::Reasoning(remaining.to_string()));
                break;
            }
        } else {
            if let Some(start_pos) = remaining.find("<think>") {
                let text = &remaining[..start_pos];
                if !text.is_empty() {
                    chunks.push(StreamChunk::Token(text.to_string()));
                }
                *in_think = true;
                remaining = &remaining[(start_pos + 7)..];
            } else {
                chunks.push(StreamChunk::Token(remaining.to_string()));
                break;
            }
        }
    }
}

pub(crate) fn parse_usage(usage: &serde_json::Value) -> UsageInfo {
    UsageInfo {
        prompt_tokens: usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        completion_tokens: usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cached_read_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cache_creation_tokens: 0,
    }
}

fn parse_response(resp: &serde_json::Value, name_map: &HashMap<String, String>) -> Result<CompletionResponse, LlmError> {
    let choice = resp
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| LlmError::JsonParse("no choices in response".into()))?;

    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::JsonParse("no message in choice".into()))?;

    let content_field = message
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let reasoning_field = message
        .get("reasoning")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = if content_field.is_empty() && !reasoning_field.is_empty() {
        reasoning_field.to_string()
    } else {
        content_field.to_string()
    };

    let finish_reason = match choice.get("finish_reason").and_then(|v| v.as_str()) {
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") => FinishReason::ToolUse,
        Some("length") => FinishReason::Length,
        Some(other) => FinishReason::Unknown(other.to_string()),
        None => FinishReason::Stop,
    };

    let mut tool_calls = Vec::new();
    if let Some(tcs) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            let id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let function = tc.get("function").unwrap_or(tc);
            let raw_name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = unsanitize_tool_name(raw_name, name_map);
            let arguments_str = function
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let arguments: serde_json::Value =
                serde_json::from_str(arguments_str).unwrap_or(json!({}));
            tool_calls.push(ToolCallResponse {
                id,
                name,
                arguments,
            });
        }
    }

    let usage = resp.get("usage").map(|u| parse_usage(u));

    Ok(CompletionResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
    })
}
