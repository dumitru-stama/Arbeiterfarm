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

/// Google Vertex AI (Gemini) backend.
///
/// Auth: pass an OAuth2 access token (from `gcloud auth print-access-token`).
/// Endpoint: `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}`
pub struct VertexAiBackend {
    name: String,
    /// Full base URL up to and including the model name.
    /// e.g. `https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.0-flash`
    endpoint: String,
    access_token: String,
    client: reqwest::Client,
}

impl VertexAiBackend {
    pub fn new(name: String, endpoint: String, access_token: String) -> Self {
        Self {
            name,
            endpoint,
            access_token,
            client: reqwest::Client::builder()
                .read_timeout(std::time::Duration::from_secs(300))
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn build_contents(&self, request: &CompletionRequest) -> (Option<String>, Vec<serde_json::Value>) {
        let mut system_instruction = None;
        let mut contents = Vec::new();

        for msg in &request.messages {
            match msg.role {
                ChatRole::System => {
                    system_instruction = Some(msg.content.clone());
                }
                ChatRole::User => {
                    // Multi-modal: serialize content_parts as parts array when present
                    if msg.has_parts() {
                        let parts: Vec<serde_json::Value> = msg.content_parts.as_ref().unwrap()
                            .iter()
                            .map(|p| match p {
                                af_core::ContentPart::Text { text } => json!({"text": text}),
                                af_core::ContentPart::Image { data, media_type } => json!({
                                    "inlineData": {
                                        "mimeType": media_type,
                                        "data": data,
                                    },
                                }),
                            })
                            .collect();
                        contents.push(json!({
                            "role": "user",
                            "parts": parts,
                        }));
                    } else {
                        contents.push(json!({
                            "role": "user",
                            "parts": [{"text": &msg.content}]
                        }));
                    }
                }
                ChatRole::Assistant => {
                    if msg.tool_calls.is_empty() {
                        contents.push(json!({
                            "role": "model",
                            "parts": [{"text": &msg.content}]
                        }));
                    } else {
                        // Build parts: optional text + functionCall parts
                        let mut parts = Vec::new();
                        if !msg.content.is_empty() {
                            parts.push(json!({"text": &msg.content}));
                        }
                        for tc in &msg.tool_calls {
                            parts.push(json!({
                                "functionCall": {
                                    "name": &tc.name,
                                    "args": &tc.arguments,
                                }
                            }));
                        }
                        contents.push(json!({
                            "role": "model",
                            "parts": parts,
                        }));
                    }
                }
                ChatRole::Tool => {
                    // Gemini expects function responses as user role with functionResponse part
                    let name = msg.name.as_deref().unwrap_or("tool");
                    // Try to parse content as JSON for structured response
                    let response_val: serde_json::Value =
                        serde_json::from_str(&msg.content).unwrap_or(json!({"result": &msg.content}));
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": name,
                                "response": response_val
                            }
                        }]
                    }));
                }
            }
        }

        // Merge consecutive user messages (Gemini rejects consecutive same-role)
        let mut merged: Vec<serde_json::Value> = Vec::new();
        for msg in contents {
            if msg["role"].as_str() == Some("user") {
                if let Some(last) = merged.last_mut() {
                    if last["role"].as_str() == Some("user") {
                        // Merge parts arrays
                        let existing_parts = last["parts"].take();
                        let new_parts = msg["parts"].clone();
                        let mut parts = match existing_parts {
                            serde_json::Value::Array(arr) => arr,
                            other => vec![other],
                        };
                        match new_parts {
                            serde_json::Value::Array(arr) => parts.extend(arr),
                            other => parts.push(other),
                        }
                        last["parts"] = serde_json::Value::Array(parts);
                        continue;
                    }
                }
            }
            merged.push(msg);
        }

        (system_instruction, merged)
    }

    fn build_tools(&self, request: &CompletionRequest) -> Option<serde_json::Value> {
        if request.tools.is_empty() {
            return None;
        }
        let function_declarations: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": &t.name,
                    "description": &t.description,
                    "parameters": &t.parameters,
                })
            })
            .collect();

        Some(json!([{
            "functionDeclarations": function_declarations,
        }]))
    }

    fn build_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let (system_instruction, contents) = self.build_contents(request);

        let mut body = json!({
            "contents": contents,
        });

        if let Some(system) = &system_instruction {
            body["systemInstruction"] = json!({
                "parts": [{"text": system}]
            });
        }

        if let Some(tools) = self.build_tools(request) {
            body["tools"] = tools;
        }

        let mut gen_config = json!({});
        if let Some(max_tokens) = request.max_tokens {
            gen_config["maxOutputTokens"] = json!(max_tokens);
        }
        if let Some(temperature) = request.temperature {
            gen_config["temperature"] = json!(temperature);
        }
        if gen_config.as_object().map_or(false, |o| !o.is_empty()) {
            body["generationConfig"] = gen_config;
        }

        body
    }
}

#[async_trait]
impl LlmBackend for VertexAiBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Extract model name from backend name (e.g. "vertex:gemini-2.0-flash" → "gemini-2.0-flash")
        let model_name = self.name.strip_prefix("vertex:").unwrap_or(&self.name);
        let meta = crate::model_catalog::lookup(model_name);
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
        let url = format!("{}:generateContent", self.endpoint);
        let body = self.build_body(&request);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
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

        parse_vertex_response(&resp_json)
    }

    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        tx: mpsc::Sender<Result<StreamChunk, LlmError>>,
    ) -> Result<(), LlmError> {
        let url = format!("{}:streamGenerateContent?alt=sse", self.endpoint);
        let body = self.build_body(&request);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
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

        let mut byte_stream = resp.bytes_stream();
        let mut buffer = String::new();

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
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        let chunks = parse_vertex_sse_event(&event);
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
}

fn parse_vertex_sse_event(event: &serde_json::Value) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();
    let mut has_tool_calls = false;

    if let Some(candidates) = event.get("candidates").and_then(|v| v.as_array()) {
        for candidate in candidates {
            if let Some(content) = candidate.get("content") {
                if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
                    for part in parts {
                        // Text part
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                chunks.push(StreamChunk::Token(text.to_string()));
                            }
                        }
                        // Function call part
                        if let Some(fc) = part.get("functionCall") {
                            has_tool_calls = true;
                            let name = fc
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = fc
                                .get("args")
                                .map(|v| serde_json::to_string(v).unwrap_or_default())
                                .unwrap_or_default();
                            let id = format!("vertex-{}", uuid::Uuid::new_v4());
                            chunks.push(StreamChunk::ToolCallStart {
                                id: id.clone(),
                                name,
                            });
                            if !args.is_empty() {
                                chunks.push(StreamChunk::ToolCallDelta {
                                    id,
                                    arguments_delta: args,
                                });
                            }
                        }
                    }
                }
            }

            // Finish reason — override to ToolUse if function calls were present
            if let Some(reason) = candidate.get("finishReason").and_then(|v| v.as_str()) {
                let finish = if has_tool_calls {
                    FinishReason::ToolUse
                } else {
                    match reason {
                        "STOP" => FinishReason::Stop,
                        "MAX_TOKENS" => FinishReason::Length,
                        "SAFETY" | "RECITATION" | "OTHER" => {
                            FinishReason::Unknown(reason.to_string())
                        }
                        other => FinishReason::Unknown(other.to_string()),
                    }
                };
                chunks.push(StreamChunk::Done(finish));
            }
        }
    }

    // Usage metadata
    if let Some(usage) = event.get("usageMetadata") {
        let prompt = usage
            .get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let completion = usage
            .get("candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cached = usage
            .get("cachedContentTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if prompt > 0 || completion > 0 {
            chunks.push(StreamChunk::Usage(UsageInfo {
                prompt_tokens: prompt as u32,
                completion_tokens: completion as u32,
                cached_read_tokens: cached as u32,
                cache_creation_tokens: 0,
            }));
        }
    }

    chunks
}

fn parse_vertex_response(resp: &serde_json::Value) -> Result<CompletionResponse, LlmError> {
    let candidate = resp
        .get("candidates")
        .and_then(|c| c.get(0))
        .ok_or_else(|| LlmError::JsonParse("no candidates in response".into()))?;

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    if let Some(content) = candidate.get("content") {
        if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    text_parts.push(text.to_string());
                }
                if let Some(fc) = part.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                    tool_calls.push(ToolCallResponse {
                        id: format!("vertex-{}", uuid::Uuid::new_v4()),
                        name,
                        arguments: args,
                    });
                }
            }
        }
    }

    let finish_reason = match candidate
        .get("finishReason")
        .and_then(|v| v.as_str())
    {
        Some("STOP") => FinishReason::Stop,
        Some("MAX_TOKENS") => FinishReason::Length,
        Some(other) => FinishReason::Unknown(other.to_string()),
        None => FinishReason::Stop,
    };

    // If we got tool calls, mark finish reason as ToolUse
    let finish_reason = if !tool_calls.is_empty() {
        FinishReason::ToolUse
    } else {
        finish_reason
    };

    let usage = resp.get("usageMetadata").map(|u| UsageInfo {
        prompt_tokens: u
            .get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        completion_tokens: u
            .get("candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cached_read_tokens: u
            .get("cachedContentTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cache_creation_tokens: 0,
    });

    Ok(CompletionResponse {
        content: text_parts.join(""),
        tool_calls,
        finish_reason,
        usage,
    })
}
