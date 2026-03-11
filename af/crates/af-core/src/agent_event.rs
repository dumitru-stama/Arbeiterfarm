use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Events emitted during agent message processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// A streaming token.
    Token(String),
    /// Chain-of-thought reasoning (displayed differently from content).
    Reasoning(String),
    /// Agent is starting a tool call.
    ToolCallStart {
        tool_name: String,
        tool_input: serde_json::Value,
    },
    /// Tool call completed with a result.
    ToolCallResult {
        tool_name: String,
        success: bool,
        summary: String,
    },
    /// Evidence reference found and verified.
    Evidence {
        ref_type: String,
        ref_id: Uuid,
    },
    /// Agent finished with a final response.
    Done {
        message_id: Uuid,
        content: String,
    },
    /// LLM usage info for a single API call.
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        cached_read_tokens: u32,
        cache_creation_tokens: u32,
        route: String,
        context_window: u32,
    },
    /// Context was compacted to fit within the model's context window.
    ContextCompacted {
        estimated_tokens: u32,
        messages_compacted: usize,
        context_window: u32,
    },
    /// Agent encountered an error.
    Error(String),
}

/// Events emitted during workflow orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrchestratorEvent {
    /// An event from an individual agent within the workflow.
    AgentEvent {
        agent_name: String,
        event: AgentEvent,
    },
    /// All agents in a group completed.
    GroupComplete {
        group: u32,
        agents: Vec<String>,
    },
    /// The entire workflow completed.
    WorkflowComplete {
        workflow_name: String,
    },
    /// A signal from one agent was applied to the workflow routing.
    SignalApplied {
        kind: String,
        target_agent: String,
        reason: String,
        source_agent: String,
    },
    /// A repivot was detected: a tool produced a replacement artifact.
    /// Eligible completed agents are re-queued to analyze the new artifact.
    RepivotApplied {
        original_artifact_id: String,
        new_artifact_id: String,
        new_filename: String,
        requeued_agents: Vec<String>,
    },
    /// Fan-out started: child threads spawned for extracted artifacts.
    FanOutStarted {
        parent_artifact_id: String,
        child_count: usize,
        child_thread_ids: Vec<Uuid>,
    },
    /// Fan-out completed: all child threads finished.
    FanOutComplete {
        parent_thread_id: Uuid,
        child_thread_ids: Vec<Uuid>,
        completed: usize,
        failed: usize,
    },
    /// Error during orchestration.
    Error(String),
}
