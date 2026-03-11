use af_api::SourceMap;
use af_core::{
    AgentConfig, CompositePostToolHook, EvidenceResolverRegistry, NoopPluginDb, Plugin, PluginDb,
    PostToolHook, ToolConfigHook, ToolExecutorRegistry, ToolRendererRegistry, ToolSpecRegistry,
    WorkflowDef,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Output from running all plugins through their lifecycle.
pub struct PluginRunnerOutput {
    pub agent_configs: Vec<AgentConfig>,
    pub workflows: Vec<WorkflowDef>,
    pub post_tool_hook: Option<Arc<dyn PostToolHook>>,
    pub tool_config_hooks: Vec<Arc<dyn ToolConfigHook>>,
    pub metadata: serde_json::Value,
    pub source_map: SourceMap,
}

/// Run all plugins through their lifecycle: migrations → declare → wire → collect outputs.
///
/// Takes `&[&dyn Plugin]` (not `Box<dyn Plugin>`) so the binary retains ownership —
/// needed for post-run operations like starting the VT gateway.
pub async fn run_plugins(
    plugins: &[&dyn Plugin],
    pool: Option<&sqlx::PgPool>,
    specs: &mut ToolSpecRegistry,
    executors: &mut ToolExecutorRegistry,
    evidence: &mut EvidenceResolverRegistry,
    renderers: &mut ToolRendererRegistry,
) -> anyhow::Result<PluginRunnerOutput> {
    // Cache ScopedPluginDb per schema (multiple plugins may share a schema)
    let mut schema_dbs: HashMap<String, Arc<dyn PluginDb>> = HashMap::new();

    let mut all_agents = Vec::new();
    let mut all_workflows = Vec::new();
    let mut all_hooks: Vec<Arc<dyn PostToolHook>> = Vec::new();
    let mut all_config_hooks: Vec<Arc<dyn ToolConfigHook>> = Vec::new();
    let mut merged_metadata = serde_json::json!({});
    let mut source_map = SourceMap::new();

    for plugin in plugins {
        let schema = plugin.schema().to_string();
        let plugin_name = plugin.name().to_string();

        // Get or create ScopedPluginDb for this schema
        let plugin_db: Arc<dyn PluginDb> = if let Some(db) = schema_dbs.get(&schema) {
            Arc::clone(db)
        } else {
            let db: Arc<dyn PluginDb> = match pool {
                Some(p) => Arc::new(af_db::ScopedPluginDb::new(p.clone(), &schema)),
                None => Arc::new(NoopPluginDb),
            };
            schema_dbs.insert(schema, Arc::clone(&db));
            db
        };

        // Run migrations
        let migrations = plugin.migrations();
        if !migrations.is_empty() {
            if let Err(e) = plugin_db.migrate(&migrations).await {
                eprintln!(
                    "[af] WARNING: migration failed for plugin '{}': {e}",
                    plugin.name()
                );
            }
        }

        // Snapshot tool names before declare
        let before: HashSet<String> = specs.list().into_iter().map(|s| s.to_string()).collect();

        // Declare tool specs
        plugin.declare(specs);

        // Diff: new tools added by this plugin
        for name in specs.list() {
            if !before.contains(name) {
                source_map.tools.insert(name.to_string(), plugin_name.clone());
            }
        }

        // Wire executors, evidence resolvers, renderers
        plugin.wire(executors, evidence, renderers, Arc::clone(&plugin_db));

        // Collect agent configs — tag with plugin source
        let agents = plugin.agent_configs();
        for a in &agents {
            source_map.agents.insert(a.name.clone(), plugin_name.clone());
        }
        all_agents.extend(agents);

        // Collect workflows — tag with plugin source
        let workflows = plugin.workflows();
        for w in &workflows {
            source_map.workflows.insert(w.name.clone(), plugin_name.clone());
        }
        all_workflows.extend(workflows);

        // Collect post-tool hooks
        let hooks = plugin.post_tool_hooks(Arc::clone(&plugin_db));
        all_hooks.extend(hooks);

        // Collect tool config hooks
        let config_hooks = plugin.tool_config_hooks(Arc::clone(&plugin_db));
        all_config_hooks.extend(config_hooks);

        // Merge metadata
        let meta = plugin.metadata();
        if let serde_json::Value::Object(map) = meta {
            if let serde_json::Value::Object(ref mut merged) = merged_metadata {
                merged.extend(map);
            }
        }
    }

    // Build composite hook
    let post_tool_hook: Option<Arc<dyn PostToolHook>> = match all_hooks.len() {
        0 => None,
        1 => Some(all_hooks.into_iter().next().unwrap()),
        _ => Some(Arc::new(CompositePostToolHook::new(all_hooks))),
    };

    Ok(PluginRunnerOutput {
        agent_configs: all_agents,
        workflows: all_workflows,
        post_tool_hook,
        tool_config_hooks: all_config_hooks,
        metadata: merged_metadata,
        source_map,
    })
}
