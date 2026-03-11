use af_core::{ChatMessage, ChatRole};
use serde::{Deserialize, Serialize};

/// Estimate the number of tokens for a set of messages and tool descriptions.
///
/// Uses a conservative heuristic (~4 chars per token) to avoid exceeding context limits.
/// Slightly over-estimates, which is safer than under-estimating (triggers compaction
/// a bit early rather than hitting an API error).
pub fn estimate_tokens(messages: &[ChatMessage], tools: &[ToolDescription]) -> u32 {
    let mut total: u32 = 0;

    for msg in messages {
        // ~4 chars per token + message framing overhead (images ~516 tokens each)
        total += msg.estimate_content_tokens() + 4;

        // Tool calls in assistant messages contribute their serialized arguments
        for tc in &msg.tool_calls {
            // tool call name + id overhead
            total += (tc.name.len() as u32) / 4 + 4;
            let args_str = serde_json::to_string(&tc.arguments).unwrap_or_default();
            total += (args_str.len() as u32) / 4;
        }

        // Tool name for tool-result messages
        if msg.role == ChatRole::Tool {
            if let Some(ref name) = msg.name {
                total += (name.len() as u32) / 4 + 2;
            }
        }
    }

    // Tool descriptions contribute their schema definitions
    for tool in tools {
        total += (tool.name.len() as u32) / 4 + 2;
        total += (tool.description.len() as u32) / 4;
        let params_str = serde_json::to_string(&tool.parameters).unwrap_or_default();
        total += (params_str.len() as u32) / 4;
        total += 10; // per-tool framing overhead
    }

    total
}

/// A completion request sent to an LLM backend.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDescription>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// Description of a tool for native tool calling (Mode B).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescription {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call returned by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResponse {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The finish reason for a completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    ToolUse,
    Length,
    Unknown(String),
}

/// A non-streaming completion response.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallResponse>,
    pub finish_reason: FinishReason,
    pub usage: Option<UsageInfo>,
}

/// Token usage information.
#[derive(Debug, Clone, Default)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Cached input tokens (OpenAI: cached_tokens, Anthropic: cache_read_input_tokens, Vertex: cachedContentTokenCount).
    pub cached_read_tokens: u32,
    /// Cache creation tokens (Anthropic only: cache_creation_input_tokens).
    pub cache_creation_tokens: u32,
}

impl UsageInfo {
    /// Merge another UsageInfo additively into this one (for accumulating across streaming chunks).
    pub fn merge(&mut self, other: &UsageInfo) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.cached_read_tokens += other.cached_read_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
    }
}

/// A chunk from a streaming response.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    Token(String),
    /// Chain-of-thought reasoning (e.g. OpenAI "reasoning" field).
    Reasoning(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, arguments_delta: String },
    Done(FinishReason),
    Usage(UsageInfo),
}
