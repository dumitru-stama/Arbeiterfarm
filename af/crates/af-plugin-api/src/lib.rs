// Re-exports from af-core for plugins.
// Plugins depend ONLY on this crate, never on af-db/af-llm/af-agents/af-jobs.

pub use af_core::{
    // Types
    ArtifactRef, ExecutorEntry, OutputRedirectPolicy, SandboxProfile, SpawnConfig, ToolError,
    ToolOutputKind, ToolPolicy, ToolRequest, ToolResult, ToolSpec,
    // Context
    Clock, CoreConfig, OutputStore, ToolContext, UtcClock,
    // Executor
    ToolConfigHook, ToolExecutor, ToolInvoker,
    // Registry
    ToolExecutorRegistry, ToolSpecRegistry, validate_registries,
    // Identity
    ActorContext, Identity,
    // Evidence
    EvidenceRef, EvidenceResolver, EvidenceResolverRegistry,
    // Agent
    AgentConfig, LlmRoute,
    // Agent events
    AgentEvent,
    // LLM types
    BackendCapabilities, ChatMessage, ChatRole,
    // Renderer
    DefaultJsonRenderer, ToolRenderer, ToolRendererRegistry,
    // Plugin DB
    Migration, PluginDb, PluginDbError,
    // Plugin trait
    CompositePostToolHook, NoopPluginDb, Plugin, PostToolHook, WorkflowDef, WorkflowStepDef,
};
