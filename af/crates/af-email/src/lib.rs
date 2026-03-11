pub mod executor;
pub mod providers;
pub mod rate_limiter;
pub mod recipient_rules;
pub mod scheduler;
pub mod specs;
pub mod tone;
pub mod types;

use af_core::{AgentConfig, LlmRoute, ToolExecutorRegistry, ToolSpecRegistry};
use dashmap::DashMap;
use sqlx::PgPool;
use std::sync::Arc;

/// Phase 1: register tool specs (pure, no deps).
pub fn declare(registry: &mut ToolSpecRegistry) {
    for spec in specs::all_specs() {
        registry
            .register(spec)
            .expect("failed to register email tool spec");
    }
}

/// Phase 2: wire in-process executors.
pub fn wire(executors: &mut ToolExecutorRegistry, pool: PgPool) {
    let global_rpm: u32 = std::env::var("AF_EMAIL_RATE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let per_user_rpm: u32 = std::env::var("AF_EMAIL_PER_USER_RPM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let max_recipients: usize = std::env::var("AF_EMAIL_MAX_RECIPIENTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let max_body_bytes: usize = std::env::var("AF_EMAIL_MAX_BODY_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_048_576);

    let state = Arc::new(executor::EmailExecutorState {
        pool,
        gmail: providers::gmail::GmailProvider::new(),
        protonmail: providers::protonmail::ProtonMailProvider::new(),
        global_limiter: Arc::new(rate_limiter::RateLimiter::new(global_rpm)),
        per_user_limiters: DashMap::new(),
        per_user_rpm,
        max_recipients,
        max_body_bytes,
    });

    executors
        .register(Box::new(executor::EmailSendExecutor { state: state.clone() }))
        .expect("failed to register email.send executor");
    executors
        .register(Box::new(executor::EmailDraftExecutor { state: state.clone() }))
        .expect("failed to register email.draft executor");
    executors
        .register(Box::new(executor::EmailScheduleExecutor { state: state.clone() }))
        .expect("failed to register email.schedule executor");
    executors
        .register(Box::new(executor::EmailListInboxExecutor { state: state.clone() }))
        .expect("failed to register email.list_inbox executor");
    executors
        .register(Box::new(executor::EmailReadExecutor { state: state.clone() }))
        .expect("failed to register email.read executor");
    executors
        .register(Box::new(executor::EmailReplyExecutor { state: state.clone() }))
        .expect("failed to register email.reply executor");
    executors
        .register(Box::new(executor::EmailSearchExecutor { state }))
        .expect("failed to register email.search executor");
}

/// Build the email-composer agent configuration.
pub fn email_composer_agent() -> AgentConfig {
    AgentConfig {
        name: "email-composer".to_string(),
        system_prompt: concat!(
            "You are an email composition assistant. You help users draft, send, search, ",
            "and manage emails via Gmail and ProtonMail.\n\n",
            "Workflow:\n",
            "1. Understand the user's needs (who to contact, what to say, tone)\n",
            "2. Compose the email respecting the requested tone preset\n",
            "3. Prefer email.draft first for review, then email.send after confirmation\n",
            "4. For reading/searching, use email.list_inbox, email.read, email.search\n\n",
            "Available tone presets: brief, formal, informal, technical, executive_summary, ",
            "friendly, urgent, diplomatic\n\n",
            "Rules:\n",
            "- Always confirm with the user before sending (prefer drafts first)\n",
            "- Never send to addresses the user hasn't explicitly approved\n",
            "- If a send is blocked by recipient rules, report it clearly\n",
            "- For sensitive emails, suggest the 'diplomatic' tone\n",
            "- Always include clear subject lines\n",
        )
        .to_string(),
        allowed_tools: vec!["email.*".into()],
        default_route: LlmRoute::Auto,
        metadata: serde_json::Value::Null,
        tool_call_budget: Some(20),
        timeout_secs: Some(300),
    }
}
