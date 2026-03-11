use af_core::{AgentConfig, LlmRoute};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
pub struct AgentToml {
    pub name: String,
    #[serde(default = "default_route")]
    pub route: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub metadata: Option<toml::Value>,
    pub prompt: PromptSection,
    pub timeout_secs: Option<u32>,
}

#[derive(Deserialize)]
pub struct PromptSection {
    pub text: String,
}

fn default_route() -> String {
    "auto".to_string()
}

pub fn load_agent_from_file(path: &str) -> anyhow::Result<AgentConfig> {
    let path = Path::new(path);
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read agent file '{}': {e}", path.display()))?;

    let agent: AgentToml = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("invalid TOML in '{}': {e}", path.display()))?;

    if agent.name.is_empty() {
        anyhow::bail!("agent name cannot be empty in '{}'", path.display());
    }
    if agent.prompt.text.trim().is_empty() {
        anyhow::bail!("agent prompt.text cannot be empty in '{}'", path.display());
    }

    let metadata = match agent.metadata {
        Some(v) => serde_json::to_value(&v)?,
        None => serde_json::json!({}),
    };

    Ok(AgentConfig {
        name: agent.name,
        system_prompt: agent.prompt.text,
        allowed_tools: agent.tools,
        default_route: LlmRoute::from_str(&agent.route),
        metadata,
        tool_call_budget: None,
        timeout_secs: agent.timeout_secs,
    })
}
