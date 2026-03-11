use crate::{
    AgentConfig, EvidenceResolverRegistry, Migration, PluginDb, PluginDbError, PostToolHook,
    ToolExecutorRegistry, ToolRendererRegistry, ToolSpecRegistry,
};
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;

/// Lifecycle trait for domain plugins.
///
/// No async methods — keeps af-core free of tokio in trait surface.
/// Migrations run via the `PluginRunner` (which is async).
pub trait Plugin: Send + Sync {
    /// Short identifier, e.g. "re", "vt".
    fn name(&self) -> &str;

    /// DB schema prefix. May differ from name() — e.g. VtPlugin has name="vt" but schema="re"
    /// because it shares the RE schema (`re.vt_cache` table).
    fn schema(&self) -> &str;

    /// Plugin-specific DB migrations.
    fn migrations(&self) -> Vec<Migration> {
        vec![]
    }

    /// Register tool specs (pure, no runtime deps).
    fn declare(&self, specs: &mut ToolSpecRegistry);

    /// Register executors, evidence resolvers, and renderers.
    fn wire(
        &self,
        executors: &mut ToolExecutorRegistry,
        evidence: &mut EvidenceResolverRegistry,
        renderers: &mut ToolRendererRegistry,
        plugin_db: Arc<dyn PluginDb>,
    );

    /// Agent presets provided by this plugin.
    fn agent_configs(&self) -> Vec<AgentConfig> {
        vec![]
    }

    /// Workflow definitions provided by this plugin.
    fn workflows(&self) -> Vec<WorkflowDef> {
        vec![]
    }

    /// Post-tool hooks (e.g. IOC extraction).
    fn post_tool_hooks(&self, _plugin_db: Arc<dyn PluginDb>) -> Vec<Arc<dyn PostToolHook>> {
        vec![]
    }

    /// Pre-execution hooks to enrich tool_config with per-invocation data.
    fn tool_config_hooks(&self, _plugin_db: Arc<dyn PluginDb>) -> Vec<Arc<dyn crate::ToolConfigHook>> {
        vec![]
    }

    /// Plugin-specific metadata (flows to CliConfig). E.g. `{"ghidra_cache_dir": "..."}`.
    fn metadata(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

/// A workflow definition provided by a plugin.
#[derive(Debug, Clone)]
pub struct WorkflowDef {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<WorkflowStepDef>,
}

/// A single step in a workflow definition.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepDef {
    pub agent: String,
    pub group: u32,
    pub prompt: String,
    #[serde(skip_serializing_if = "is_true")]
    pub can_repivot: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Combines multiple PostToolHooks into one.
pub struct CompositePostToolHook {
    hooks: Vec<Arc<dyn PostToolHook>>,
}

impl CompositePostToolHook {
    pub fn new(hooks: Vec<Arc<dyn PostToolHook>>) -> Self {
        Self { hooks }
    }
}

#[async_trait]
impl PostToolHook for CompositePostToolHook {
    async fn on_tool_result(
        &self,
        tool_name: &str,
        output_json: &serde_json::Value,
        project_id: uuid::Uuid,
        user_id: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        let mut errors = Vec::new();
        for hook in &self.hooks {
            if let Err(e) = hook
                .on_tool_result(tool_name, output_json, project_id, user_id)
                .await
            {
                errors.push(e);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// No-op PluginDb for when DB is unavailable.
/// Tools still work, but DB-backed features (IOC storage, family tags) are no-ops.
pub struct NoopPluginDb;

#[async_trait]
impl PluginDb for NoopPluginDb {
    async fn query_json(
        &self,
        _sql: &str,
        _params: Vec<serde_json::Value>,
        _user_id: Option<uuid::Uuid>,
    ) -> Result<Vec<serde_json::Value>, PluginDbError> {
        Ok(vec![])
    }

    async fn execute_json(
        &self,
        _sql: &str,
        _params: Vec<serde_json::Value>,
        _user_id: Option<uuid::Uuid>,
    ) -> Result<u64, PluginDbError> {
        Ok(0)
    }

    async fn migrate(&self, _migrations: &[Migration]) -> Result<(), PluginDbError> {
        Ok(())
    }

    fn schema(&self) -> &str {
        "noop"
    }

    async fn audit_log(
        &self,
        _event_type: &str,
        _actor_user_id: Option<uuid::Uuid>,
        _detail: Option<&serde_json::Value>,
    ) -> Result<(), PluginDbError> {
        Ok(())
    }
}
