use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub default_route: LlmRoute,
    pub metadata: serde_json::Value,
    /// Maximum total tool calls for this agent in a single run.
    /// None = use default MAX_TOOL_CALLS (20).
    #[serde(default)]
    pub tool_call_budget: Option<u32>,
    /// Maximum seconds this agent may run before being timed out.
    /// None = no per-agent limit (global stream duration cap still applies).
    #[serde(default)]
    pub timeout_secs: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LlmRoute {
    Local,
    Auto,
    Backend(String),
}

impl LlmRoute {
    pub fn from_str(s: &str) -> Self {
        match s {
            "auto" => LlmRoute::Auto,
            "local" => LlmRoute::Local,
            other => {
                if let Some(name) = other.strip_prefix("backend:") {
                    LlmRoute::Backend(name.to_string())
                } else {
                    LlmRoute::Auto
                }
            }
        }
    }

    pub fn to_db_string(&self) -> String {
        match self {
            LlmRoute::Auto => "auto".to_string(),
            LlmRoute::Local => "local".to_string(),
            LlmRoute::Backend(name) => format!("backend:{name}"),
        }
    }
}

impl AgentConfig {
    /// Convert an AgentConfig to DB-compatible fields for upsert.
    pub fn allowed_tools_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.allowed_tools
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        )
    }
}
