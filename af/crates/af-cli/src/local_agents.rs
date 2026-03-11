use af_core::{AgentConfig, LlmRoute};
use std::path::{Path, PathBuf};

use crate::commands::agent_file::AgentToml;

/// Parsed result from a TOML file: an AgentConfig ready for registration.
#[derive(Debug)]
pub struct LocalAgentDef {
    pub config: AgentConfig,
    pub source_file: PathBuf,
}

/// Errors that can occur when loading a local agent TOML file.
#[derive(Debug)]
pub enum LocalAgentError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    Validation(PathBuf, String),
}

impl std::fmt::Display for LocalAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "local agent {}: I/O error: {e}", path.display()),
            Self::Parse(path, e) => write!(f, "local agent {}: parse error: {e}", path.display()),
            Self::Validation(path, msg) => {
                write!(f, "local agent {}: {msg}", path.display())
            }
        }
    }
}

/// Default agents directory: `~/.af/agents/`, overridable via `AF_AGENTS_DIR`.
pub fn default_agents_dir() -> PathBuf {
    std::env::var("AF_AGENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".af")
                .join("agents")
        })
}

/// Scan a directory for `*.toml` files and attempt to load each as a local agent.
/// Returns one Result per file found.
pub fn load_local_agents(dir: &Path) -> Vec<Result<LocalAgentDef, LocalAgentError>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return vec![];
            }
            return vec![Err(LocalAgentError::Io(dir.to_path_buf(), e))];
        }
    };

    let mut results = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                results.push(Err(LocalAgentError::Io(dir.to_path_buf(), e)));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            results.push(load_single_agent(&path));
        }
    }
    results
}

/// Convert an AgentToml into an AgentConfig using the same logic as `load_agent_from_file`.
pub fn agent_toml_to_config(agent: &AgentToml) -> AgentConfig {
    let metadata = match &agent.metadata {
        Some(v) => serde_json::to_value(v).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };

    AgentConfig {
        name: agent.name.clone(),
        system_prompt: agent.prompt.text.clone(),
        allowed_tools: agent.tools.clone(),
        default_route: LlmRoute::from_str(&agent.route),
        metadata,
        tool_call_budget: None,
        timeout_secs: agent.timeout_secs,
    }
}

fn load_single_agent(path: &Path) -> Result<LocalAgentDef, LocalAgentError> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| LocalAgentError::Io(path.to_path_buf(), e))?;
    let agent: AgentToml =
        toml::from_str(&contents).map_err(|e| LocalAgentError::Parse(path.to_path_buf(), e))?;

    // Validation
    if agent.name.is_empty() {
        return Err(LocalAgentError::Validation(
            path.to_path_buf(),
            "agent name cannot be empty".into(),
        ));
    }
    if agent.prompt.text.trim().is_empty() {
        return Err(LocalAgentError::Validation(
            path.to_path_buf(),
            "agent prompt.text cannot be empty".into(),
        ));
    }

    let config = agent_toml_to_config(&agent);
    Ok(LocalAgentDef {
        config,
        source_file: path.to_path_buf(),
    })
}

/// Load and register all local TOML agents into the given agent configs and DB.
///
/// Pass `None` for `dir` to use the default (`~/.af/agents/` or `AF_AGENTS_DIR`).
/// Returns the list of successfully registered agent names.
pub async fn register_local_agents(
    pool: &sqlx::PgPool,
    agent_configs: &mut Vec<AgentConfig>,
    dir: Option<&Path>,
) -> Vec<String> {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => default_agents_dir(),
    };

    let mut registered = Vec::new();

    for result in load_local_agents(&dir) {
        match result {
            Ok(def) => {
                let name = def.config.name.clone();
                let tools_json = def.config.allowed_tools_json();
                let route = def.config.default_route.to_db_string();
                let metadata = if def.config.metadata.is_null() {
                    serde_json::json!({})
                } else {
                    def.config.metadata.clone()
                };

                if let Err(e) = af_db::agents::upsert(
                    pool,
                    &name,
                    &def.config.system_prompt,
                    &tools_json,
                    &route,
                    &metadata,
                    false, // is_builtin = false for local agents
                    Some("local"),
                    def.config.timeout_secs.map(|s| s as i32),
                )
                .await
                {
                    eprintln!("[af] WARNING: local agent '{name}': DB upsert failed: {e}");
                    continue;
                }

                eprintln!(
                    "[af] Local agent '{name}' loaded from {}",
                    def.source_file.display()
                );
                agent_configs.push(def.config);
                registered.push(name);
            }
            Err(e) => eprintln!("[af] WARNING: {e}"),
        }
    }

    if !registered.is_empty() {
        eprintln!(
            "[af] {} local agent(s) loaded from {}",
            registered.len(),
            dir.display()
        );
    }

    registered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_toml(dir: &Path, filename: &str, contents: &str) -> PathBuf {
        let path = dir.join(filename);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_load_nonexistent_dir() {
        let results = load_local_agents(Path::new("/nonexistent/dir/for/af/agents"));
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_empty_dir() {
        let dir = TempDir::new().unwrap();
        let results = load_local_agents(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_valid_agent() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
name = "my-agent"
route = "auto"
tools = ["file.*", "rizin.*"]

[prompt]
text = "You are a specialized analysis agent."
"#;
        write_toml(dir.path(), "my-agent.toml", toml_content);
        let results = load_local_agents(dir.path());
        assert_eq!(results.len(), 1);

        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.config.name, "my-agent");
        assert_eq!(def.config.system_prompt, "You are a specialized analysis agent.");
        assert_eq!(def.config.allowed_tools, vec!["file.*", "rizin.*"]);
    }

    #[test]
    fn test_load_agent_with_metadata() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
name = "meta-agent"
tools = ["file.*"]

[metadata]
version = "1.0"

[prompt]
text = "Agent with metadata."
"#;
        write_toml(dir.path(), "meta-agent.toml", toml_content);
        let results = load_local_agents(dir.path());
        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.config.name, "meta-agent");
        assert_eq!(def.config.metadata["version"], "1.0");
    }

    #[test]
    fn test_load_invalid_agent_empty_name() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
name = ""
tools = []
[prompt]
text = "Some prompt."
"#;
        write_toml(dir.path(), "bad.toml", toml_content);
        let results = load_local_agents(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalAgentError::Validation(_, _)));
    }

    #[test]
    fn test_load_invalid_agent_empty_prompt() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
name = "bad-agent"
tools = []
[prompt]
text = "   "
"#;
        write_toml(dir.path(), "bad.toml", toml_content);
        let results = load_local_agents(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    #[test]
    fn test_load_invalid_toml() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "bad.toml", "not valid {{ toml");
        let results = load_local_agents(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalAgentError::Parse(_, _)));
    }

    #[test]
    fn test_skips_non_toml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not an agent").unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();
        let results = load_local_agents(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_agent_toml_to_config() {
        let agent = AgentToml {
            name: "test-agent".to_string(),
            route: "local".to_string(),
            tools: vec!["file.*".to_string()],
            metadata: None,
            prompt: crate::commands::agent_file::PromptSection {
                text: "Test prompt.".to_string(),
            },
            timeout_secs: None,
        };
        let config = agent_toml_to_config(&agent);
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.system_prompt, "Test prompt.");
        assert!(matches!(config.default_route, LlmRoute::Local));
    }
}
