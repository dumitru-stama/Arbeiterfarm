use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM error: {0}")]
    Llm(#[from] af_llm::LlmError),

    #[error("database error: {0}")]
    Db(String),

    #[error("tool error: {tool_name}: {message}")]
    ToolFailed { tool_name: String, message: String },

    #[error("tool not allowed: {0}")]
    ToolNotAllowed(String),

    #[error("max tool calls exceeded ({0})")]
    MaxToolCalls(usize),

    #[error("no LLM backend configured")]
    NoBackend,

    #[error("{0}")]
    Other(String),
}
