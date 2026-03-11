use serde::Serialize;
use std::collections::HashMap;

/// Tracks the origin plugin/source for every tool, agent, and workflow.
///
/// This is purely display metadata — core types like `ToolSpec` and `AgentConfig`
/// are NOT modified. The source map flows from plugin_runner → bootstrap → CliConfig → AppState → API.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub tools: HashMap<String, String>,
    pub agents: HashMap<String, String>,
    pub workflows: HashMap<String, String>,
}

/// Inventory of tools/agents/workflows provided by a single plugin.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PluginInventory {
    pub tools: Vec<String>,
    pub agents: Vec<String>,
    pub workflows: Vec<String>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Invert the maps: group tools/agents/workflows by their source label.
    pub fn by_source(&self) -> HashMap<String, PluginInventory> {
        let mut map: HashMap<String, PluginInventory> = HashMap::new();

        for (tool, source) in &self.tools {
            map.entry(source.clone())
                .or_default()
                .tools
                .push(tool.clone());
        }
        for (agent, source) in &self.agents {
            map.entry(source.clone())
                .or_default()
                .agents
                .push(agent.clone());
        }
        for (workflow, source) in &self.workflows {
            map.entry(source.clone())
                .or_default()
                .workflows
                .push(workflow.clone());
        }

        // Sort within each inventory for stable output
        for inv in map.values_mut() {
            inv.tools.sort();
            inv.agents.sort();
            inv.workflows.sort();
        }

        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_source_map() {
        let sm = SourceMap::new();
        let by_src = sm.by_source();
        assert!(by_src.is_empty());
    }

    #[test]
    fn test_by_source_groups_correctly() {
        let mut sm = SourceMap::new();
        sm.tools.insert("file.info".into(), "builtin".into());
        sm.tools.insert("file.read_range".into(), "builtin".into());
        sm.tools.insert("rizin.bininfo".into(), "re".into());
        sm.agents.insert("surface".into(), "re".into());
        sm.agents.insert("default".into(), "builtin".into());
        sm.workflows.insert("full-analysis".into(), "re".into());

        let by_src = sm.by_source();
        assert_eq!(by_src.len(), 2);

        let builtin = &by_src["builtin"];
        assert_eq!(builtin.tools, vec!["file.info", "file.read_range"]);
        assert_eq!(builtin.agents, vec!["default"]);
        assert!(builtin.workflows.is_empty());

        let re = &by_src["re"];
        assert_eq!(re.tools, vec!["rizin.bininfo"]);
        assert_eq!(re.agents, vec!["surface"]);
        assert_eq!(re.workflows, vec!["full-analysis"]);
    }
}
