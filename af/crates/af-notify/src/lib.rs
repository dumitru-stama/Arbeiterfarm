pub mod channels;
pub mod executor;
pub mod listener;
pub mod queue;
pub mod specs;

use af_core::{AgentConfig, LlmRoute, ToolExecutorRegistry, ToolSpecRegistry};
use sqlx::PgPool;
use std::sync::Arc;

/// Phase 1: register tool specs (pure, no deps).
pub fn declare(registry: &mut ToolSpecRegistry) {
    for spec in specs::all_specs() {
        registry
            .register(spec)
            .expect("failed to register notify tool spec");
    }
}

/// Phase 2: wire in-process executors.
pub fn wire(executors: &mut ToolExecutorRegistry, pool: PgPool) {
    let state = Arc::new(executor::NotifyState { pool });

    executors
        .register(Box::new(executor::NotifySendExecutor {
            state: state.clone(),
        }))
        .expect("failed to register notify.send executor");
    executors
        .register(Box::new(executor::NotifyUploadExecutor {
            state: state.clone(),
        }))
        .expect("failed to register notify.upload executor");
    executors
        .register(Box::new(executor::NotifyListExecutor {
            state: state.clone(),
        }))
        .expect("failed to register notify.list executor");
    executors
        .register(Box::new(executor::NotifyTestExecutor { state }))
        .expect("failed to register notify.test executor");
}

/// Build the notifier agent configuration.
pub fn notifier_agent() -> AgentConfig {
    AgentConfig {
        name: "notifier".to_string(),
        system_prompt: concat!(
            "You are a notification agent. You can send notifications to pre-configured channels.\n\n",
            "Use notify.list to see available channels for the project.\n",
            "Use notify.send to send text notifications (webhook, email, matrix).\n",
            "Use notify.upload to upload files to WebDAV channels.\n\n",
            "Always confirm channel availability before sending. Include relevant context ",
            "in notification bodies.",
        )
        .to_string(),
        allowed_tools: vec!["notify.*".into()],
        default_route: LlmRoute::Auto,
        metadata: serde_json::Value::Null,
        tool_call_budget: Some(10),
        timeout_secs: Some(120),
    }
}
