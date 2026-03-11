use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("JSON parse error: {0}")]
    JsonParse(String),

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("no backend configured for route")]
    NoBackend,

    #[error("backend not found: {0}")]
    BackendNotFound(String),

    #[error("streaming not supported by this backend")]
    StreamingNotSupported,

    #[error("{0}")]
    Other(String),
}
