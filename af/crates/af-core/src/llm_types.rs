use serde::{Deserialize, Serialize};

/// Role in a chat conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

impl ChatRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// Info about a tool call made by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A part of multi-modal message content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image {
        data: String,       // base64-encoded
        media_type: String,  // e.g. "image/png"
    },
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Tool call ID this message is responding to (for role=tool).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name (for role=tool).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls made by the assistant (for role=assistant with tool use).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallInfo>,
    /// Multi-modal content parts. When present, backends use this instead of `content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<ContentPart>>,
}

impl ChatMessage {
    /// Returns true if this message has non-empty content_parts.
    pub fn has_parts(&self) -> bool {
        self.content_parts.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// Estimate tokens for this message's content.
    /// Text: ~4 chars/token. Images: ~516 tokens each (~4 tiles x 129 tokens).
    pub fn estimate_content_tokens(&self) -> u32 {
        if let Some(ref parts) = self.content_parts {
            parts.iter().map(|p| match p {
                ContentPart::Text { text } => (text.len() as u32) / 4,
                ContentPart::Image { .. } => 516,
            }).sum()
        } else {
            (self.content.len() as u32) / 4
        }
    }
}

/// Capabilities reported by an LLM backend.
#[derive(Debug, Clone, Default)]
pub struct BackendCapabilities {
    /// Backend supports native tool/function calling.
    pub supports_tool_calls: bool,
    /// Backend supports streaming responses.
    pub supports_streaming: bool,
    /// Backend is a local model (no network required).
    pub is_local: bool,
    /// Context window size in tokens (None if unknown).
    pub context_window: Option<u32>,
    /// Maximum output tokens (None if unknown).
    pub max_output_tokens: Option<u32>,
    /// Cost per million input tokens in USD (None if unknown/free).
    pub cost_per_mtok_input: Option<f64>,
    /// Cost per million output tokens in USD (None if unknown/free).
    pub cost_per_mtok_output: Option<f64>,
    /// Whether the model supports vision/image inputs (None if unknown).
    pub supports_vision: Option<bool>,
    /// Knowledge cutoff date string, e.g. "2025-03" (None if unknown).
    pub knowledge_cutoff: Option<String>,
}
