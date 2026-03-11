use af_core::AgentConfig;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::commands::agent_file::AgentToml;
use crate::local_agents::agent_toml_to_config;

/// TOML schema for a local workflow definition.
#[derive(Deserialize)]
struct LocalWorkflowToml {
    workflow: WorkflowSection,
}

#[derive(Deserialize)]
struct WorkflowSection {
    name: String,
    description: Option<String>,
    steps: Vec<StepToml>,
    agents: Option<Vec<AgentToml>>,
}

#[derive(Deserialize)]
struct StepToml {
    agent: String,
    group: u32,
    prompt: String,
    #[serde(default = "default_true")]
    can_repivot: bool,
    timeout_secs: Option<u32>,
    #[serde(default)]
    parallel: bool,
}

fn default_true() -> bool {
    true
}

/// Parsed result from a TOML file: a workflow definition with optional inline agents.
#[derive(Debug)]
pub struct LocalWorkflowDef {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<af_db::workflows::WorkflowStep>,
    pub inline_agents: Vec<AgentConfig>,
    pub source_file: PathBuf,
}

/// Errors that can occur when loading a local workflow TOML file.
#[derive(Debug)]
pub enum LocalWorkflowError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    Validation(PathBuf, String),
}

impl std::fmt::Display for LocalWorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "local workflow {}: I/O error: {e}", path.display()),
            Self::Parse(path, e) => {
                write!(f, "local workflow {}: parse error: {e}", path.display())
            }
            Self::Validation(path, msg) => {
                write!(f, "local workflow {}: {msg}", path.display())
            }
        }
    }
}

/// Default workflows directory: `~/.af/workflows/`, overridable via `AF_WORKFLOWS_DIR`.
pub fn default_workflows_dir() -> PathBuf {
    std::env::var("AF_WORKFLOWS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".af")
                .join("workflows")
        })
}

/// Scan a directory for `*.toml` files and attempt to load each as a local workflow.
/// Returns one Result per file found.
pub fn load_local_workflows(dir: &Path) -> Vec<Result<LocalWorkflowDef, LocalWorkflowError>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return vec![];
            }
            return vec![Err(LocalWorkflowError::Io(dir.to_path_buf(), e))];
        }
    };

    let mut results = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                results.push(Err(LocalWorkflowError::Io(dir.to_path_buf(), e)));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            results.push(load_single_workflow(&path));
        }
    }
    results
}

fn load_single_workflow(path: &Path) -> Result<LocalWorkflowDef, LocalWorkflowError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| LocalWorkflowError::Io(path.to_path_buf(), e))?;
    let toml_doc: LocalWorkflowToml = toml::from_str(&contents)
        .map_err(|e| LocalWorkflowError::Parse(path.to_path_buf(), e))?;
    let wf = toml_doc.workflow;

    // Validate name
    validate_workflow_name(&wf.name, path)?;

    // Must have at least one step
    if wf.steps.is_empty() {
        return Err(LocalWorkflowError::Validation(
            path.to_path_buf(),
            "workflow must have at least one step".into(),
        ));
    }

    // Validate each step
    for (i, step) in wf.steps.iter().enumerate() {
        if step.agent.trim().is_empty() {
            return Err(LocalWorkflowError::Validation(
                path.to_path_buf(),
                format!("step {}: agent cannot be empty", i + 1),
            ));
        }
        if step.prompt.trim().is_empty() {
            return Err(LocalWorkflowError::Validation(
                path.to_path_buf(),
                format!("step {}: prompt cannot be empty", i + 1),
            ));
        }
    }

    // Convert steps to WorkflowStep
    let steps: Vec<af_db::workflows::WorkflowStep> = wf
        .steps
        .iter()
        .map(|s| af_db::workflows::WorkflowStep {
            agent: s.agent.clone(),
            group: s.group,
            prompt: s.prompt.clone(),
            can_repivot: s.can_repivot,
            timeout_secs: s.timeout_secs,
            parallel: s.parallel,
        })
        .collect();

    // Convert inline agents
    let mut inline_agents = Vec::new();
    if let Some(agents) = &wf.agents {
        for (i, agent) in agents.iter().enumerate() {
            if agent.name.is_empty() {
                return Err(LocalWorkflowError::Validation(
                    path.to_path_buf(),
                    format!("inline agent {}: name cannot be empty", i + 1),
                ));
            }
            if agent.prompt.text.trim().is_empty() {
                return Err(LocalWorkflowError::Validation(
                    path.to_path_buf(),
                    format!(
                        "inline agent {}: prompt.text cannot be empty",
                        i + 1
                    ),
                ));
            }
            inline_agents.push(agent_toml_to_config(agent));
        }
    }

    Ok(LocalWorkflowDef {
        name: wf.name,
        description: wf.description,
        steps,
        inline_agents,
        source_file: path.to_path_buf(),
    })
}

/// Validate workflow name: lowercase alphanumeric + hyphens/underscores.
/// Pattern: `[a-z][a-z0-9_-]*` — no double hyphens, no leading/trailing hyphens.
fn validate_workflow_name(name: &str, path: &Path) -> Result<(), LocalWorkflowError> {
    if name.is_empty() {
        return Err(LocalWorkflowError::Validation(
            path.to_path_buf(),
            "workflow name cannot be empty".into(),
        ));
    }

    let first = name.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(LocalWorkflowError::Validation(
            path.to_path_buf(),
            format!("workflow name must start with a-z: \"{name}\""),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(LocalWorkflowError::Validation(
            path.to_path_buf(),
            format!(
                "workflow name must contain only a-z, 0-9, hyphens, underscores: \"{name}\""
            ),
        ));
    }

    if name.contains("--") {
        return Err(LocalWorkflowError::Validation(
            path.to_path_buf(),
            format!("workflow name must not contain double hyphens: \"{name}\""),
        ));
    }

    if name.ends_with('-') || name.ends_with('_') {
        return Err(LocalWorkflowError::Validation(
            path.to_path_buf(),
            format!("workflow name must not end with hyphen or underscore: \"{name}\""),
        ));
    }

    Ok(())
}

/// Load and register all local TOML workflows into the DB and agent configs.
///
/// Pass `None` for `dir` to use the default (`~/.af/workflows/` or `AF_WORKFLOWS_DIR`).
/// Returns the list of successfully registered workflow names.
pub async fn register_local_workflows(
    pool: &sqlx::PgPool,
    agent_configs: &mut Vec<AgentConfig>,
    dir: Option<&Path>,
) -> Vec<String> {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => default_workflows_dir(),
    };

    let mut registered = Vec::new();

    for result in load_local_workflows(&dir) {
        match result {
            Ok(def) => {
                let name = def.name.clone();

                // Register inline agents first
                for agent_config in &def.inline_agents {
                    let tools_json = agent_config.allowed_tools_json();
                    let route = agent_config.default_route.to_db_string();
                    let metadata = if agent_config.metadata.is_null() {
                        serde_json::json!({})
                    } else {
                        agent_config.metadata.clone()
                    };

                    if let Err(e) = af_db::agents::upsert(
                        pool,
                        &agent_config.name,
                        &agent_config.system_prompt,
                        &tools_json,
                        &route,
                        &metadata,
                        false,
                        Some("local"),
                        agent_config.timeout_secs.map(|s| s as i32),
                    )
                    .await
                    {
                        eprintln!(
                            "[af] WARNING: inline agent '{}' for workflow '{name}': DB upsert failed: {e}",
                            agent_config.name
                        );
                        continue;
                    }
                    agent_configs.push(agent_config.clone());
                }

                // Serialize steps and upsert workflow
                let steps_json = serde_json::to_value(&def.steps).unwrap_or_default();
                if let Err(e) = af_db::workflows::upsert(
                    pool,
                    &name,
                    def.description.as_deref(),
                    &steps_json,
                    false,
                    Some("local"),
                )
                .await
                {
                    eprintln!("[af] WARNING: local workflow '{name}': DB upsert failed: {e}");
                    continue;
                }

                eprintln!(
                    "[af] Local workflow '{name}' loaded from {}",
                    def.source_file.display()
                );
                registered.push(name);
            }
            Err(e) => eprintln!("[af] WARNING: {e}"),
        }
    }

    if !registered.is_empty() {
        eprintln!(
            "[af] {} local workflow(s) loaded from {}",
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
        let results = load_local_workflows(Path::new("/nonexistent/dir/for/af/workflows"));
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_empty_dir() {
        let dir = TempDir::new().unwrap();
        let results = load_local_workflows(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_valid_workflow() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "my-analysis"
description = "Custom RE pipeline"

[[workflow.steps]]
agent = "surface"
group = 1
prompt = "Perform quick surface triage."

[[workflow.steps]]
agent = "reporter"
group = 2
prompt = "Write final report."
"#;
        write_toml(dir.path(), "my-analysis.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);

        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.name, "my-analysis");
        assert_eq!(def.description.as_deref(), Some("Custom RE pipeline"));
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.steps[0].agent, "surface");
        assert_eq!(def.steps[0].group, 1);
        assert_eq!(def.steps[0].prompt, "Perform quick surface triage.");
        assert_eq!(def.steps[1].agent, "reporter");
        assert_eq!(def.steps[1].group, 2);
        assert!(def.inline_agents.is_empty());
    }

    #[test]
    fn test_load_workflow_with_inline_agents() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "with-agents"
description = "Workflow with inline agents"

[[workflow.steps]]
agent = "my-custom-agent"
group = 1
prompt = "Analyze something."

[[workflow.agents]]
name = "my-custom-agent"
route = "auto"
tools = ["ghidra.analyze", "ghidra.decompile"]

[workflow.agents.prompt]
text = "You are a specialized function analyst."
"#;
        write_toml(dir.path(), "with-agents.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);

        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.name, "with-agents");
        assert_eq!(def.inline_agents.len(), 1);
        assert_eq!(def.inline_agents[0].name, "my-custom-agent");
        assert_eq!(
            def.inline_agents[0].allowed_tools,
            vec!["ghidra.analyze", "ghidra.decompile"]
        );
    }

    #[test]
    fn test_step_defaults_can_repivot() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "defaults-test"

[[workflow.steps]]
agent = "surface"
group = 1
prompt = "Triage."

[[workflow.steps]]
agent = "reporter"
group = 2
prompt = "Report."
can_repivot = false
"#;
        write_toml(dir.path(), "defaults.toml", toml_content);
        let results = load_local_workflows(dir.path());
        let def = results.into_iter().next().unwrap().unwrap();
        assert!(def.steps[0].can_repivot); // default true
        assert!(!def.steps[1].can_repivot); // explicit false
    }

    #[test]
    fn test_step_parallel_flag() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "parallel-test"

[[workflow.steps]]
agent = "surface"
group = 1
prompt = "Triage."
parallel = true

[[workflow.steps]]
agent = "intel"
group = 1
prompt = "VT lookup."
parallel = true

[[workflow.steps]]
agent = "decompiler"
group = 1
prompt = "Decompile."
"#;
        write_toml(dir.path(), "parallel.toml", toml_content);
        let results = load_local_workflows(dir.path());
        let def = results.into_iter().next().unwrap().unwrap();
        assert!(def.steps[0].parallel);  // explicit true
        assert!(def.steps[1].parallel);  // explicit true
        assert!(!def.steps[2].parallel); // default false
    }

    #[test]
    fn test_validate_name_valid() {
        let p = PathBuf::from("/test");
        assert!(validate_workflow_name("my-analysis", &p).is_ok());
        assert!(validate_workflow_name("full-analysis", &p).is_ok());
        assert!(validate_workflow_name("simple", &p).is_ok());
        assert!(validate_workflow_name("a1b2c3", &p).is_ok());
        assert!(validate_workflow_name("under_score", &p).is_ok());
        assert!(validate_workflow_name("mix-and_match", &p).is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        let p = PathBuf::from("/test");
        // Empty
        assert!(validate_workflow_name("", &p).is_err());
        // Uppercase
        assert!(validate_workflow_name("MyAnalysis", &p).is_err());
        // Starts with digit
        assert!(validate_workflow_name("1analysis", &p).is_err());
        // Double hyphen
        assert!(validate_workflow_name("my--analysis", &p).is_err());
        // Trailing hyphen
        assert!(validate_workflow_name("analysis-", &p).is_err());
        // Trailing underscore
        assert!(validate_workflow_name("analysis_", &p).is_err());
        // Special chars
        assert!(validate_workflow_name("my.analysis", &p).is_err());
    }

    #[test]
    fn test_empty_steps_rejected() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "empty-steps"
"#;
        write_toml(dir.path(), "empty.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);
        // This will be a parse error since steps is required
        assert!(results[0].is_err());
    }

    #[test]
    fn test_empty_step_agent_rejected() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "bad-step"

[[workflow.steps]]
agent = ""
group = 1
prompt = "Do something."
"#;
        write_toml(dir.path(), "bad-step.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalWorkflowError::Validation(_, _)));
    }

    #[test]
    fn test_empty_step_prompt_rejected() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "bad-prompt"

[[workflow.steps]]
agent = "surface"
group = 1
prompt = "   "
"#;
        write_toml(dir.path(), "bad-prompt.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    #[test]
    fn test_skips_non_toml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a workflow").unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();
        let results = load_local_workflows(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_invalid_toml() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "bad.toml", "not valid {{ toml");
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalWorkflowError::Parse(_, _)));
    }

    #[test]
    fn test_inline_agent_empty_name_rejected() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "bad-inline"

[[workflow.steps]]
agent = "x"
group = 1
prompt = "Do something."

[[workflow.agents]]
name = ""
tools = []
[workflow.agents.prompt]
text = "Some prompt."
"#;
        write_toml(dir.path(), "bad-inline.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    #[test]
    fn test_inline_agent_empty_prompt_rejected() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[workflow]
name = "bad-inline-prompt"

[[workflow.steps]]
agent = "x"
group = 1
prompt = "Do something."

[[workflow.agents]]
name = "bad-agent"
tools = []
[workflow.agents.prompt]
text = "   "
"#;
        write_toml(dir.path(), "bad-inline-prompt.toml", toml_content);
        let results = load_local_workflows(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }
}
