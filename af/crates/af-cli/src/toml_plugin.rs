use af_core::{
    AgentConfig, EvidenceResolverRegistry, Plugin, PluginDb, PostToolHook, ToolExecutorRegistry,
    ToolRendererRegistry, ToolSpecRegistry, WorkflowDef, WorkflowStepDef,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A plugin defined entirely by TOML files — no Rust code needed.
///
/// Directory layout:
/// ```text
/// my-plugin/
/// ├── plugin.toml         # manifest (name, description)
/// ├── tools/              # tool definitions (same format as ~/.af/tools/)
/// │   └── my_tool.toml
/// ├── agents/             # agent definitions (same format as ~/.af/agents/)
/// │   └── my_agent.toml
/// └── workflows/          # workflow definitions (same format as ~/.af/workflows/)
///     └── my_workflow.toml
/// ```
pub struct TomlPlugin {
    manifest: PluginManifest,
    tools: Vec<crate::local_tools::LocalToolDef>,
    agents: Vec<AgentConfig>,
    workflows: Vec<WorkflowDef>,
}

#[derive(Deserialize)]
struct PluginToml {
    plugin: PluginManifest,
}

#[derive(Deserialize, Clone)]
struct PluginManifest {
    name: String,
    #[allow(dead_code)]
    description: Option<String>,
}

#[derive(Debug)]
pub enum TomlPluginError {
    Io(std::io::Error),
    Parse(String),
    InvalidName(String),
}

impl std::fmt::Display for TomlPluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Parse(e) => write!(f, "parse error: {e}"),
            Self::InvalidName(n) => write!(f, "invalid plugin name: {n}"),
        }
    }
}

impl TomlPlugin {
    /// Load a TOML plugin from a directory containing `plugin.toml`.
    pub fn load(dir: &Path) -> Result<Self, TomlPluginError> {
        // Read manifest
        let manifest_path = dir.join("plugin.toml");
        let manifest_str = std::fs::read_to_string(&manifest_path).map_err(TomlPluginError::Io)?;
        let plugin_toml: PluginToml = toml::from_str(&manifest_str)
            .map_err(|e| TomlPluginError::Parse(format!("{}: {e}", manifest_path.display())))?;

        let manifest = plugin_toml.plugin;

        // Validate name: lowercase, alphanumeric + hyphens
        if manifest.name.is_empty()
            || !manifest
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(TomlPluginError::InvalidName(manifest.name.clone()));
        }

        // Load tools
        let tools_dir = dir.join("tools");
        let tools = if tools_dir.is_dir() {
            crate::local_tools::load_local_tools(&tools_dir)
                .into_iter()
                .filter_map(|r| match r {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!(
                            "[af] WARNING: plugin '{}': failed to load tool: {e}",
                            manifest.name
                        );
                        None
                    }
                })
                .collect()
        } else {
            vec![]
        };

        // Load agents
        let agents_dir = dir.join("agents");
        let agents = if agents_dir.is_dir() {
            crate::local_agents::load_local_agents(&agents_dir)
                .into_iter()
                .filter_map(|r| match r {
                    Ok(a) => Some(a.config),
                    Err(e) => {
                        eprintln!(
                            "[af] WARNING: plugin '{}': failed to load agent: {e}",
                            manifest.name
                        );
                        None
                    }
                })
                .collect()
        } else {
            vec![]
        };

        // Load workflows
        let workflows_dir = dir.join("workflows");
        let workflows = if workflows_dir.is_dir() {
            load_workflows_from_dir(&workflows_dir, &manifest.name)
        } else {
            vec![]
        };

        eprintln!(
            "[af] TOML plugin '{}' loaded: {} tools, {} agents, {} workflows",
            manifest.name,
            tools.len(),
            agents.len(),
            workflows.len()
        );

        Ok(Self {
            manifest,
            tools,
            agents,
            workflows,
        })
    }
}

impl TomlPlugin {
    /// Plugin name (accessible without going through the trait).
    pub fn plugin_name(&self) -> &str {
        &self.manifest.name
    }
}

impl Plugin for TomlPlugin {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn schema(&self) -> &str {
        // TOML plugins don't have DB schemas
        "noop"
    }

    fn declare(&self, specs: &mut ToolSpecRegistry) {
        for tool in &self.tools {
            if let Err(e) = specs.register(tool.spec.clone()) {
                eprintln!(
                    "[af] WARNING: plugin '{}': failed to register tool '{}': {e}",
                    self.manifest.name, tool.spec.name
                );
            }
        }
    }

    fn wire(
        &self,
        executors: &mut ToolExecutorRegistry,
        _evidence: &mut EvidenceResolverRegistry,
        _renderers: &mut ToolRendererRegistry,
        _plugin_db: Arc<dyn PluginDb>,
    ) {
        for tool in &self.tools {
            if let Err(e) = executors.register_oop(tool.spawn_config.clone()) {
                eprintln!(
                    "[af] WARNING: plugin '{}': failed to wire tool '{}': {e}",
                    self.manifest.name, tool.spec.name
                );
            }
        }
    }

    fn agent_configs(&self) -> Vec<AgentConfig> {
        self.agents.clone()
    }

    fn workflows(&self) -> Vec<WorkflowDef> {
        self.workflows.clone()
    }

    fn post_tool_hooks(&self, _plugin_db: Arc<dyn PluginDb>) -> Vec<Arc<dyn PostToolHook>> {
        vec![]
    }
}

/// Load all TOML plugins from a directory. Each subdirectory with a `plugin.toml` is a plugin.
pub fn load_toml_plugins(dir: &Path) -> Vec<TomlPlugin> {
    if !dir.is_dir() {
        return vec![];
    }

    let mut plugins = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[af] WARNING: cannot read plugins dir {}: {e}", dir.display());
            return vec![];
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("plugin.toml").exists() {
            match TomlPlugin::load(&path) {
                Ok(p) => plugins.push(p),
                Err(e) => eprintln!(
                    "[af] WARNING: failed to load plugin from {}: {e}",
                    path.display()
                ),
            }
        }
    }

    plugins
}

/// Default plugins directory.
pub fn default_plugins_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AF_PLUGINS_DIR") {
        PathBuf::from(dir)
    } else {
        dirs_home().join(".af").join("plugins")
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// Workflow loading (simplified — reuses the TOML format from local_workflows)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WfToml {
    workflow: WfSection,
}

#[derive(Deserialize)]
struct WfSection {
    name: String,
    description: Option<String>,
    steps: Vec<WfStep>,
}

#[derive(Deserialize)]
struct WfStep {
    agent: String,
    group: u32,
    prompt: String,
    #[serde(default = "default_true")]
    can_repivot: bool,
    timeout_secs: Option<u32>,
}

fn default_true() -> bool {
    true
}

fn load_workflows_from_dir(dir: &Path, plugin_name: &str) -> Vec<WorkflowDef> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut workflows = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "toml") {
            match std::fs::read_to_string(&path) {
                Ok(content) => match toml::from_str::<WfToml>(&content) {
                    Ok(wf) => {
                        workflows.push(WorkflowDef {
                            name: wf.workflow.name,
                            description: wf.workflow.description,
                            steps: wf
                                .workflow
                                .steps
                                .into_iter()
                                .map(|s| WorkflowStepDef {
                                    agent: s.agent,
                                    group: s.group,
                                    prompt: s.prompt,
                                    can_repivot: s.can_repivot,
                                    timeout_secs: s.timeout_secs,
                                })
                                .collect(),
                        });
                    }
                    Err(e) => eprintln!(
                        "[af] WARNING: plugin '{plugin_name}': bad workflow {}: {e}",
                        path.display()
                    ),
                },
                Err(e) => eprintln!(
                    "[af] WARNING: plugin '{plugin_name}': cannot read {}: {e}",
                    path.display()
                ),
            }
        }
    }
    workflows
}
