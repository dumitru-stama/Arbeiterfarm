//! Backend for local LLM servers: Ollama, llama.cpp, vLLM, etc.
//!
//! Key differences from the cloud OpenAI backend:
//! - No tool name sanitization (dots are fine: `file.info`, `ghidra.decompile`)
//! - Uses `max_tokens` instead of `max_completion_tokens`
//! - No `stream_options` (local servers don't support it)
//! - `supports_tool_calls` is model-dependent (checked via model catalog)
//! - Always `is_local: true`

use async_trait::async_trait;
use af_core::BackendCapabilities;
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::backend::LlmBackend;
use crate::error::LlmError;
use crate::openai::{log_request, parse_usage, stream_sse};
use crate::request::{
    CompletionRequest, CompletionResponse, FinishReason, StreamChunk, ToolCallResponse,
};

/// Local LLM backend — for Ollama, llama.cpp, vLLM, and other local servers.
/// No name sanitization, no OpenAI-specific parameters.
pub struct LocalLlmBackend {
    name: String,
    endpoint: String,
    api_key: Option<String>,
    model: String,
    client: reqwest::Client,
}

impl LocalLlmBackend {
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
                // Local models can be very slow — generous timeouts
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
                    let parts: Vec<serde_json::Value> = msg
                        .content_parts
                        .as_ref()
                        .unwrap()
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
                // Tool calls — no name sanitization for local models
                if !msg.tool_calls.is_empty() {
                    let tool_calls: Vec<serde_json::Value> = msg
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": &tc.id,
                                "type": "function",
                                "function": {
                                    "name": &tc.name,
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
        // No name sanitization — local servers handle dots fine
        Some(
            request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": &t.name,
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
            // No stream_options — local servers don't support it
        }
        if let Some(tools) = self.build_tools(request) {
            body["tools"] = json!(tools);
        }
        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(temperature) = request.temperature {
            let spec = crate::model_catalog::lookup(&self.model);
            match spec.and_then(|s| s.temperature_range) {
                Some((min, max)) if (max - min).abs() < f32::EPSILON => {
                    // Fixed temperature — omit parameter
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
impl LlmBackend for LocalLlmBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> BackendCapabilities {
        let meta = crate::model_catalog::lookup(&self.model);
        let tool_calls = meta
            .and_then(|m| m.supports_tool_calls)
            .unwrap_or(true); // default: assume native tool calls
        BackendCapabilities {
            supports_tool_calls: tool_calls,
            supports_streaming: true,
            is_local: true,
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

        parse_local_response(&resp_json)
    }

    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        tx: mpsc::Sender<Result<StreamChunk, LlmError>>,
    ) -> Result<(), LlmError> {
        let url = format!("{}/v1/chat/completions", self.endpoint.trim_end_matches('/'));
        let body = self.build_body(&request, true);

        log_request(&self.model, &self.endpoint, &request, &body);

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
            eprintln!(
                "[llm-response] ERROR status={status} body={}",
                text.chars().take(1000).collect::<String>()
            );
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        // No name_map for local — tool names pass through as-is
        let empty_map = HashMap::new();
        stream_sse(resp, tx, &empty_map).await
    }
}

/// Parse a non-streaming response from a local LLM server. No name unsanitization needed.
fn parse_local_response(resp: &serde_json::Value) -> Result<CompletionResponse, LlmError> {
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
            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
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
