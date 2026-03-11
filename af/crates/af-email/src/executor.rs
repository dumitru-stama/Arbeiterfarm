use async_trait::async_trait;
use af_core::{ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use dashmap::DashMap;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::providers::gmail::GmailProvider;
use crate::providers::protonmail::ProtonMailProvider;
use crate::providers::EmailProvider;
use crate::rate_limiter::RateLimiter;
use crate::recipient_rules;
use crate::types::*;

fn tool_err(code: &str, msg: String, retryable: bool) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable,
        details: Value::Null,
    }
}

/// Shared state for all email executors.
pub struct EmailExecutorState {
    pub pool: PgPool,
    pub gmail: GmailProvider,
    pub protonmail: ProtonMailProvider,
    pub global_limiter: Arc<RateLimiter>,
    pub per_user_limiters: DashMap<Uuid, (Arc<RateLimiter>, Instant)>,
    pub per_user_rpm: u32,
    pub max_recipients: usize,
    pub max_body_bytes: usize,
}

/// Maximum number of per-user rate limiter entries before pruning.
const MAX_PER_USER_LIMITERS: usize = 1000;
/// Evict per-user limiters not used in the last 30 minutes.
const LIMITER_TTL_SECS: u64 = 1800;

impl EmailExecutorState {
    fn get_provider(&self, name: &str) -> Result<&dyn EmailProvider, ToolError> {
        match name {
            "gmail" => Ok(&self.gmail),
            "protonmail" => Ok(&self.protonmail),
            _ => Err(tool_err(
                "invalid_provider",
                format!("unknown provider '{name}', expected 'gmail' or 'protonmail'"),
                false,
            )),
        }
    }

    async fn get_credentials(
        &self,
        user_id: Option<Uuid>,
        provider: Option<&str>,
    ) -> Result<af_db::email::EmailCredentialRow, ToolError> {
        let uid = user_id.ok_or_else(|| {
            tool_err(
                "no_user",
                "email tools require an authenticated user".into(),
                false,
            )
        })?;

        af_db::email::get_default_credential(&self.pool, uid, provider)
            .await
            .map_err(|e| {
                tracing::warn!("failed to load email credentials: {e}");
                tool_err("db_error", "failed to load credentials".into(), true)
            })?
            .ok_or_else(|| {
                let prov_msg = provider
                    .map(|p| format!(" for provider '{p}'"))
                    .unwrap_or_default();
                tool_err(
                    "no_credentials",
                    format!("no email credentials configured{prov_msg}. Use 'af email setup' to configure."),
                    false,
                )
            })
    }

    async fn check_rate_limit(&self, user_id: Option<Uuid>) -> Result<(), ToolError> {
        // Global rate limit
        self.global_limiter
            .acquire(Duration::from_secs(5))
            .await
            .map_err(|e| tool_err("rate_limited", format!("global {e}"), true))?;

        // Per-user rate limit
        if let Some(uid) = user_id {
            // Prune stale entries if the map is too large
            if self.per_user_limiters.len() > MAX_PER_USER_LIMITERS {
                let cutoff = Instant::now() - Duration::from_secs(LIMITER_TTL_SECS);
                self.per_user_limiters.retain(|_, (_, last_used)| *last_used > cutoff);
            }

            let limiter = {
                let mut entry = self
                    .per_user_limiters
                    .entry(uid)
                    .or_insert_with(|| (Arc::new(RateLimiter::new(self.per_user_rpm)), Instant::now()));
                entry.1 = Instant::now();
                entry.0.clone()
            };
            limiter
                .acquire(Duration::from_secs(5))
                .await
                .map_err(|e| tool_err("rate_limited", format!("per-user {e}"), true))?;
        }
        Ok(())
    }

    async fn check_recipients(
        &self,
        to: &[String],
        cc: &[String],
        bcc: &[String],
        project_id: Uuid,
    ) -> Result<(), ToolError> {
        let total = to.len() + cc.len() + bcc.len();
        if total > self.max_recipients {
            return Err(tool_err(
                "too_many_recipients",
                format!(
                    "total recipients ({total}) exceeds limit ({})",
                    self.max_recipients
                ),
                false,
            ));
        }

        // Load rules (global + project-scoped) — fail closed on error
        let rules = af_db::email::list_recipient_rules(&self.pool, None, Some(project_id))
            .await
            .map_err(|e| {
                tracing::error!("failed to load recipient rules: {e}");
                tool_err(
                    "rule_error",
                    "failed to load recipient rules — send blocked for safety".into(),
                    true,
                )
            })?;

        recipient_rules::evaluate_all_recipients(to, cc, bcc, &rules).map_err(|reason| {
            tracing::warn!("recipient blocked: {reason}");
            tool_err("recipient_blocked", reason, false)
        })
    }

    async fn validate_tone(&self, tone: Option<&str>) -> Result<(), ToolError> {
        if let Some(name) = tone {
            crate::tone::validate_tone(&self.pool, name)
                .await
                .map_err(|e| tool_err("invalid_tone", e.to_string(), false))?;
        }
        Ok(())
    }

    fn extract_addresses(input: &Value, field: &str) -> Vec<String> {
        input
            .get(field)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn check_body_size(&self, body: &str) -> Result<(), ToolError> {
        if body.len() > self.max_body_bytes {
            return Err(tool_err(
                "body_too_large",
                format!("body exceeds {} bytes", self.max_body_bytes),
                false,
            ));
        }
        Ok(())
    }

    /// Reject addresses containing control characters (CR, LF, NUL).
    /// Prevents RFC 2822 header injection at the input layer.
    fn validate_addresses(addresses: &[String]) -> Result<(), ToolError> {
        for addr in addresses {
            if addr.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
                return Err(tool_err(
                    "invalid_input",
                    format!("email address contains control characters: {}", addr.chars().take(50).collect::<String>()),
                    false,
                ));
            }
        }
        Ok(())
    }

    fn check_to_not_empty(to: &[String]) -> Result<(), ToolError> {
        if to.is_empty() {
            return Err(tool_err(
                "invalid_input",
                "'to' must contain at least one recipient".into(),
                false,
            ));
        }
        Ok(())
    }

    async fn log_action(
        &self,
        ctx: &ToolContext,
        action: &str,
        provider: &str,
        from: Option<&str>,
        to: Option<&serde_json::Value>,
        subject: Option<&str>,
        tone: Option<&str>,
        success: bool,
        error: Option<&str>,
        provider_msg_id: Option<&str>,
    ) {
        if let Err(e) = af_db::email::insert_email_log(
            &self.pool,
            Some(ctx.project_id),
            ctx.actor_user_id,
            action,
            provider,
            from,
            to,
            subject,
            tone,
            success,
            error,
            provider_msg_id,
            None,
            None,
            ctx.thread_id,
            None,
        )
        .await
        {
            tracing::warn!("failed to write email log: {e}");
        }

        // Also write to immutable audit_log for compliance
        let detail = json!({
            "action": action,
            "provider": provider,
            "from": from,
            "to": to,
            "subject": subject,
            "tone": tone,
            "success": success,
            "error": error,
            "provider_message_id": provider_msg_id,
        });
        if let Err(e) = af_db::audit_log::insert(
            &self.pool,
            &format!("email.{action}"),
            None,
            ctx.actor_user_id,
            Some(&detail),
        )
        .await
        {
            tracing::warn!("failed to write audit log for email action: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Macro to reduce boilerplate for executor structs
// ---------------------------------------------------------------------------

macro_rules! email_executor {
    ($name:ident, $tool_name:expr) => {
        pub struct $name {
            pub state: Arc<EmailExecutorState>,
        }
    };
}

// ---------------------------------------------------------------------------
// email.send
// ---------------------------------------------------------------------------

email_executor!(EmailSendExecutor, "email.send");

#[async_trait]
impl ToolExecutor for EmailSendExecutor {
    fn tool_name(&self) -> &str {
        "email.send"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, _input: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let to = EmailExecutorState::extract_addresses(&input, "to");
        let cc = EmailExecutorState::extract_addresses(&input, "cc");
        let bcc = EmailExecutorState::extract_addresses(&input, "bcc");
        let subject = input["subject"].as_str().unwrap_or_default();
        let body = input["body"].as_str().unwrap_or_default();
        let tone = input.get("tone").and_then(|v| v.as_str());
        let provider_name = input.get("provider").and_then(|v| v.as_str());
        let dry_run = input.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        EmailExecutorState::check_to_not_empty(&to)?;
        EmailExecutorState::validate_addresses(&to)?;
        EmailExecutorState::validate_addresses(&cc)?;
        EmailExecutorState::validate_addresses(&bcc)?;
        self.state.check_body_size(body)?;

        // Validate
        self.state.check_rate_limit(ctx.actor_user_id).await?;
        self.state.check_recipients(&to, &cc, &bcc, ctx.project_id).await?;
        self.state.validate_tone(tone).await?;
        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;
        let provider = self.state.get_provider(&cred.provider)?;

        if dry_run {
            let to_json = serde_json::to_value(&to).unwrap_or_default();
            self.state
                .log_action(&ctx, "send", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(subject), tone, true, None, None)
                .await;
            return Ok(ToolResult {
                kind: ToolOutputKind::InlineJson,
                output_json: json!({
                    "dry_run": true,
                    "status": "validated",
                    "provider": cred.provider,
                    "from": cred.email_address,
                    "to": to,
                    "cc": cc,
                    "bcc": bcc,
                    "subject": subject,
                    "message": "All validations passed. Set dry_run=false or omit it to send."
                }),
                stdout: None,
                stderr: None,
                produced_artifacts: vec![],
                primary_artifact: None,
                evidence: vec![],
            });
        }

        let msg = EmailMessage {
            from: cred.email_address.clone(),
            to: to.clone(),
            cc: cc.clone(),
            bcc: bcc.clone(),
            subject: subject.to_string(),
            body_text: body.to_string(),
            body_html: None,
            in_reply_to: None,
            references: None,
            thread_id: None,
        };

        let to_json = serde_json::to_value(&to).unwrap_or_default();
        match provider.send(&msg, &cred.credentials_json).await {
            Ok(result) => {
                tracing::info!(
                    provider = %cred.provider,
                    to = ?to,
                    subject = %subject,
                    "email sent successfully"
                );
                self.state
                    .log_action(&ctx, "send", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(subject), tone, true, None, Some(&result.provider_message_id))
                    .await;

                Ok(ToolResult {
                    kind: ToolOutputKind::InlineJson,
                    output_json: json!({
                        "status": "sent",
                        "provider": result.provider,
                        "message_id": result.provider_message_id,
                        "from": cred.email_address,
                        "to": to,
                    }),
                    stdout: None,
                    stderr: None,
                    produced_artifacts: vec![],
                    primary_artifact: None,
                    evidence: vec![],
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                tracing::error!(provider = %cred.provider, error = %err_msg, "email send failed");
                self.state
                    .log_action(&ctx, "send", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(subject), tone, false, Some(&err_msg), None)
                    .await;
                Err(tool_err("send_failed", err_msg, true))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// email.draft
// ---------------------------------------------------------------------------

email_executor!(EmailDraftExecutor, "email.draft");

#[async_trait]
impl ToolExecutor for EmailDraftExecutor {
    fn tool_name(&self) -> &str {
        "email.draft"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, _input: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let to = EmailExecutorState::extract_addresses(&input, "to");
        let cc = EmailExecutorState::extract_addresses(&input, "cc");
        let bcc = EmailExecutorState::extract_addresses(&input, "bcc");
        let subject = input["subject"].as_str().unwrap_or_default();
        let body = input["body"].as_str().unwrap_or_default();
        let tone = input.get("tone").and_then(|v| v.as_str());
        let provider_name = input.get("provider").and_then(|v| v.as_str());

        EmailExecutorState::check_to_not_empty(&to)?;
        EmailExecutorState::validate_addresses(&to)?;
        EmailExecutorState::validate_addresses(&cc)?;
        EmailExecutorState::validate_addresses(&bcc)?;
        self.state.check_body_size(body)?;
        self.state.check_rate_limit(ctx.actor_user_id).await?;
        self.state.check_recipients(&to, &cc, &bcc, ctx.project_id).await?;
        self.state.validate_tone(tone).await?;
        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;
        let provider = self.state.get_provider(&cred.provider)?;

        let msg = EmailMessage {
            from: cred.email_address.clone(),
            to: to.clone(),
            cc,
            bcc,
            subject: subject.to_string(),
            body_text: body.to_string(),
            body_html: None,
            in_reply_to: None,
            references: None,
            thread_id: None,
        };

        let to_json = serde_json::to_value(&to).unwrap_or_default();
        match provider.create_draft(&msg, &cred.credentials_json).await {
            Ok(result) => {
                tracing::info!(provider = %cred.provider, "draft created");
                self.state
                    .log_action(&ctx, "draft", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(subject), tone, true, None, Some(&result.draft_id))
                    .await;

                Ok(ToolResult {
                    kind: ToolOutputKind::InlineJson,
                    output_json: json!({
                        "status": "draft_created",
                        "provider": result.provider,
                        "draft_id": result.draft_id,
                        "from": cred.email_address,
                        "to": to,
                    }),
                    stdout: None,
                    stderr: None,
                    produced_artifacts: vec![],
                    primary_artifact: None,
                    evidence: vec![],
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.state
                    .log_action(&ctx, "draft", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(subject), tone, false, Some(&err_msg), None)
                    .await;
                Err(tool_err("draft_failed", err_msg, true))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// email.schedule
// ---------------------------------------------------------------------------

email_executor!(EmailScheduleExecutor, "email.schedule");

#[async_trait]
impl ToolExecutor for EmailScheduleExecutor {
    fn tool_name(&self) -> &str {
        "email.schedule"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let scheduled_at = input
            .get("scheduled_at")
            .and_then(|v| v.as_str())
            .ok_or("'scheduled_at' is required")?;
        chrono::DateTime::parse_from_rfc3339(scheduled_at)
            .map_err(|e| format!("'scheduled_at' must be ISO 8601: {e}"))?;
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let to = EmailExecutorState::extract_addresses(&input, "to");
        let cc = EmailExecutorState::extract_addresses(&input, "cc");
        let bcc = EmailExecutorState::extract_addresses(&input, "bcc");
        let subject = input["subject"].as_str().unwrap_or_default();
        let body = input["body"].as_str().unwrap_or_default();
        let tone = input.get("tone").and_then(|v| v.as_str());
        let provider_name = input.get("provider").and_then(|v| v.as_str());
        let scheduled_at_str = input["scheduled_at"].as_str().unwrap_or_default();

        let scheduled_at = chrono::DateTime::parse_from_rfc3339(scheduled_at_str)
            .map_err(|e| tool_err("invalid_input", format!("invalid scheduled_at: {e}"), false))?
            .with_timezone(&chrono::Utc);

        if scheduled_at <= chrono::Utc::now() {
            return Err(tool_err("invalid_input", "'scheduled_at' must be in the future".into(), false));
        }

        EmailExecutorState::check_to_not_empty(&to)?;
        EmailExecutorState::validate_addresses(&to)?;
        EmailExecutorState::validate_addresses(&cc)?;
        EmailExecutorState::validate_addresses(&bcc)?;
        self.state.check_body_size(body)?;
        self.state.check_rate_limit(ctx.actor_user_id).await?;

        self.state.check_recipients(&to, &cc, &bcc, ctx.project_id).await?;
        self.state.validate_tone(tone).await?;
        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;

        let to_json = serde_json::to_value(&to).unwrap_or_default();
        let cc_json = serde_json::to_value(&cc).unwrap_or_default();
        let bcc_json = serde_json::to_value(&bcc).unwrap_or_default();

        let scheduled = af_db::email::create_scheduled(
            &self.state.pool,
            ctx.project_id,
            ctx.actor_user_id,
            &cred.provider,
            &cred.email_address,
            &to_json,
            &cc_json,
            &bcc_json,
            subject,
            Some(body),
            None,
            None,
            tone,
            scheduled_at,
            ctx.thread_id,
            None,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to schedule: {e}"), true))?;

        self.state
            .log_action(&ctx, "schedule", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(subject), tone, true, None, None)
            .await;

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "status": "scheduled",
                "scheduled_id": scheduled.id.to_string(),
                "scheduled_at": scheduled_at_str,
                "provider": cred.provider,
                "from": cred.email_address,
                "to": to,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// email.list_inbox
// ---------------------------------------------------------------------------

email_executor!(EmailListInboxExecutor, "email.list_inbox");

#[async_trait]
impl ToolExecutor for EmailListInboxExecutor {
    fn tool_name(&self) -> &str {
        "email.list_inbox"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, _input: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let provider_name = input.get("provider").and_then(|v| v.as_str());
        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as u32;
        let label = input.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());
        let unread_only = input.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let since = input.get("since").and_then(|v| v.as_str()).map(|s| s.to_string());

        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;
        let provider = self.state.get_provider(&cred.provider)?;

        let params = ListInboxParams {
            max_results,
            label,
            unread_only,
            since,
        };

        match provider.list_inbox(&params, &cred.credentials_json).await {
            Ok(messages) => {
                self.state
                    .log_action(&ctx, "list_inbox", &cred.provider, Some(&cred.email_address), None, None, None, true, None, None)
                    .await;

                Ok(ToolResult {
                    kind: ToolOutputKind::InlineJson,
                    output_json: json!({
                        "provider": cred.provider,
                        "count": messages.len(),
                        "messages": messages,
                    }),
                    stdout: None,
                    stderr: None,
                    produced_artifacts: vec![],
                    primary_artifact: None,
                    evidence: vec![],
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.state
                    .log_action(&ctx, "list_inbox", &cred.provider, Some(&cred.email_address), None, None, None, false, Some(&err_msg), None)
                    .await;
                Err(tool_err("list_failed", err_msg, true))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// email.read
// ---------------------------------------------------------------------------

email_executor!(EmailReadExecutor, "email.read");

#[async_trait]
impl ToolExecutor for EmailReadExecutor {
    fn tool_name(&self) -> &str {
        "email.read"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        input
            .get("message_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("'message_id' is required")?;
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let message_id = input["message_id"].as_str().unwrap_or_default();
        let provider_name = input.get("provider").and_then(|v| v.as_str());

        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;
        let provider = self.state.get_provider(&cred.provider)?;

        match provider.read_message(message_id, &cred.credentials_json).await {
            Ok(email) => {
                self.state
                    .log_action(&ctx, "read", &cred.provider, Some(&cred.email_address), None, Some(&email.subject), None, true, None, Some(message_id))
                    .await;

                Ok(ToolResult {
                    kind: ToolOutputKind::InlineJson,
                    output_json: serde_json::to_value(&email).unwrap_or_default(),
                    stdout: None,
                    stderr: None,
                    produced_artifacts: vec![],
                    primary_artifact: None,
                    evidence: vec![],
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.state
                    .log_action(&ctx, "read", &cred.provider, Some(&cred.email_address), None, None, None, false, Some(&err_msg), Some(message_id))
                    .await;
                Err(tool_err("read_failed", err_msg, true))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// email.reply
// ---------------------------------------------------------------------------

email_executor!(EmailReplyExecutor, "email.reply");

#[async_trait]
impl ToolExecutor for EmailReplyExecutor {
    fn tool_name(&self) -> &str {
        "email.reply"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        input
            .get("message_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("'message_id' is required")?;
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let message_id = input["message_id"].as_str().unwrap_or_default();
        let body = input["body"].as_str().unwrap_or_default();
        let tone = input.get("tone").and_then(|v| v.as_str());
        let provider_name = input.get("provider").and_then(|v| v.as_str());
        let reply_all = input.get("reply_all").and_then(|v| v.as_bool()).unwrap_or(false);

        self.state.check_body_size(body)?;
        self.state.check_rate_limit(ctx.actor_user_id).await?;
        self.state.validate_tone(tone).await?;
        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;
        let provider = self.state.get_provider(&cred.provider)?;

        // Read parent to get reply addresses and threading info
        let parent = provider
            .read_message(message_id, &cred.credentials_json)
            .await
            .map_err(|e| tool_err("read_failed", format!("failed to read parent: {e}"), true))?;

        let my_address = cred.email_address.to_lowercase();
        let (to, cc) = if reply_all {
            // Reply-all: to = original sender, cc = original to + cc minus self
            let mut cc_addrs: Vec<String> = parent
                .to
                .iter()
                .chain(parent.cc.iter())
                .filter(|a| a.to_lowercase() != my_address)
                .cloned()
                .collect();
            // Deduplicate
            cc_addrs.sort();
            cc_addrs.dedup();
            // Don't include the sender in CC if they're already in To
            let sender = parent.from.clone();
            cc_addrs.retain(|a| a.to_lowercase() != sender.to_lowercase());
            (vec![sender], cc_addrs)
        } else {
            (vec![parent.from.clone()], vec![])
        };

        EmailExecutorState::validate_addresses(&to)?;
        EmailExecutorState::validate_addresses(&cc)?;
        self.state.check_recipients(&to, &cc, &[], ctx.project_id).await?;

        let msg = EmailMessage {
            from: cred.email_address.clone(),
            to: to.clone(),
            cc: cc.clone(),
            bcc: vec![],
            subject: if parent.subject.starts_with("Re: ") {
                parent.subject.clone()
            } else {
                format!("Re: {}", parent.subject)
            },
            body_text: body.to_string(),
            body_html: None,
            in_reply_to: Some(format!("<{message_id}>")),
            references: parent.references.map(|r| format!("{r} <{message_id}>")),
            thread_id: parent.thread_id,
        };

        let to_json = serde_json::to_value(&to).unwrap_or_default();
        match provider.reply(message_id, &msg, &cred.credentials_json).await {
            Ok(result) => {
                self.state
                    .log_action(&ctx, "reply", &cred.provider, Some(&cred.email_address), Some(&to_json), Some(&msg.subject), tone, true, None, Some(&result.provider_message_id))
                    .await;

                Ok(ToolResult {
                    kind: ToolOutputKind::InlineJson,
                    output_json: json!({
                        "status": "replied",
                        "provider": result.provider,
                        "message_id": result.provider_message_id,
                        "in_reply_to": message_id,
                    }),
                    stdout: None,
                    stderr: None,
                    produced_artifacts: vec![],
                    primary_artifact: None,
                    evidence: vec![],
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.state
                    .log_action(&ctx, "reply", &cred.provider, Some(&cred.email_address), Some(&to_json), None, tone, false, Some(&err_msg), None)
                    .await;
                Err(tool_err("reply_failed", err_msg, true))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// email.search
// ---------------------------------------------------------------------------

email_executor!(EmailSearchExecutor, "email.search");

#[async_trait]
impl ToolExecutor for EmailSearchExecutor {
    fn tool_name(&self) -> &str {
        "email.search"
    }
    fn tool_version(&self) -> u32 {
        1
    }
    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("'query' is required")?;
        if query.trim().is_empty() {
            return Err("'query' must not be empty".into());
        }
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let query = input["query"].as_str().unwrap_or_default();
        let provider_name = input.get("provider").and_then(|v| v.as_str());
        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as u32;

        let cred = self.state.get_credentials(ctx.actor_user_id, provider_name).await?;
        let provider = self.state.get_provider(&cred.provider)?;

        match provider
            .search(query, max_results, &cred.credentials_json)
            .await
        {
            Ok(results) => {
                self.state
                    .log_action(&ctx, "search", &cred.provider, Some(&cred.email_address), None, None, None, true, None, None)
                    .await;

                Ok(ToolResult {
                    kind: ToolOutputKind::InlineJson,
                    output_json: json!({
                        "provider": cred.provider,
                        "query": query,
                        "count": results.len(),
                        "results": results,
                    }),
                    stdout: None,
                    stderr: None,
                    produced_artifacts: vec![],
                    primary_artifact: None,
                    evidence: vec![],
                })
            }
            Err(e) => {
                let err_msg = e.to_string();
                self.state
                    .log_action(&ctx, "search", &cred.provider, Some(&cred.email_address), None, None, None, false, Some(&err_msg), None)
                    .await;
                Err(tool_err("search_failed", err_msg, true))
            }
        }
    }
}
