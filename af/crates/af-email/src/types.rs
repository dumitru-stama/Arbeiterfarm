use serde::{Deserialize, Serialize};

/// Outgoing email message (provider-agnostic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailMessage {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub thread_id: Option<String>,
}

/// Result of a successful send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    pub provider_message_id: String,
    pub provider: String,
}

/// Result of creating a draft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftResult {
    pub draft_id: String,
    pub provider: String,
}

/// Summary of an email in inbox/search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailSummary {
    pub message_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub snippet: String,
    pub date: String,
    pub is_unread: bool,
    pub labels: Vec<String>,
    pub thread_id: Option<String>,
}

/// Full email content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailFull {
    pub message_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub date: String,
    pub labels: Vec<String>,
    pub thread_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Attachment metadata (content not included in v1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

/// Parameters for listing inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListInboxParams {
    pub max_results: u32,
    pub label: Option<String>,
    pub unread_only: bool,
    pub since: Option<String>,
}

/// Email module errors.
#[derive(Debug)]
pub enum EmailError {
    /// No credentials configured for this user/provider.
    NoCredentials(String),
    /// Provider API error.
    ProviderError(String),
    /// Recipient blocked by rules.
    RecipientBlocked(String),
    /// Rate limit exceeded.
    RateLimited(String),
    /// Invalid input.
    InvalidInput(String),
    /// Database error.
    DbError(String),
}

impl std::fmt::Display for EmailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmailError::NoCredentials(msg) => write!(f, "no credentials: {msg}"),
            EmailError::ProviderError(msg) => write!(f, "provider error: {msg}"),
            EmailError::RecipientBlocked(msg) => write!(f, "recipient blocked: {msg}"),
            EmailError::RateLimited(msg) => write!(f, "rate limited: {msg}"),
            EmailError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            EmailError::DbError(msg) => write!(f, "database error: {msg}"),
        }
    }
}

impl std::error::Error for EmailError {}
