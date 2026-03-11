use async_trait::async_trait;
use af_core::{ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub struct NotifyState {
    pub pool: PgPool,
}

const MAX_CHANNEL_NAME_LEN: usize = 100;
const MAX_SUBJECT_LEN: usize = 500;
const MAX_BODY_LEN: usize = 100_000; // 100 KB

fn validate_channel_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() || name.len() > MAX_CHANNEL_NAME_LEN {
        return Err(tool_err(
            "invalid_input",
            format!("channel name must be 1-{MAX_CHANNEL_NAME_LEN} characters"),
            false,
        ));
    }
    Ok(())
}

fn validate_subject(subject: &str) -> Result<(), ToolError> {
    if subject.is_empty() || subject.len() > MAX_SUBJECT_LEN {
        return Err(tool_err(
            "invalid_input",
            format!("subject must be 1-{MAX_SUBJECT_LEN} characters"),
            false,
        ));
    }
    Ok(())
}

fn validate_body(body: &str) -> Result<(), ToolError> {
    if body.len() > MAX_BODY_LEN {
        return Err(tool_err(
            "invalid_input",
            format!("body must not exceed {MAX_BODY_LEN} characters"),
            false,
        ));
    }
    Ok(())
}

fn tool_err(code: &str, msg: impl Into<String>, retryable: bool) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg.into(),
        retryable,
        details: json!({}),
    }
}

fn ok_json(v: Value) -> ToolResult {
    ToolResult {
        kind: ToolOutputKind::InlineJson,
        output_json: v,
        stdout: None,
        stderr: None,
        produced_artifacts: vec![],
        primary_artifact: None,
        evidence: vec![],
    }
}

// ---------------------------------------------------------------------------
// notify.send
// ---------------------------------------------------------------------------

pub struct NotifySendExecutor {
    pub state: Arc<NotifyState>,
}

#[async_trait]
impl ToolExecutor for NotifySendExecutor {
    fn tool_name(&self) -> &str {
        "notify.send"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let channel_name = input["channel"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "channel is required", false))?;
        let subject = input["subject"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "subject is required", false))?;
        let body = input["body"].as_str().unwrap_or("");

        validate_channel_name(channel_name)?;
        validate_subject(subject)?;
        validate_body(body)?;

        let channel = af_db::notifications::get_channel_by_name(
            &self.state.pool,
            ctx.project_id,
            channel_name,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to lookup channel: {e}"), true))?
        .ok_or_else(|| {
            tool_err(
                "channel_not_found",
                format!("no channel named '{channel_name}' in this project"),
                false,
            )
        })?;

        if !channel.enabled {
            return Err(tool_err(
                "channel_disabled",
                format!("channel '{channel_name}' is disabled"),
                false,
            ));
        }

        let row = af_db::notifications::enqueue(
            &self.state.pool,
            ctx.project_id,
            channel.id,
            subject,
            body,
            None,
            ctx.actor_user_id,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to enqueue notification: {e}"), true))?;

        Ok(ok_json(json!({
            "status": "queued",
            "id": row.id.to_string(),
            "channel": channel_name,
            "channel_type": channel.channel_type,
        })))
    }
}

// ---------------------------------------------------------------------------
// notify.upload
// ---------------------------------------------------------------------------

pub struct NotifyUploadExecutor {
    pub state: Arc<NotifyState>,
}

#[async_trait]
impl ToolExecutor for NotifyUploadExecutor {
    fn tool_name(&self) -> &str {
        "notify.upload"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let channel_name = input["channel"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "channel is required", false))?;

        let artifact_id_str = input["artifact_id"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "artifact_id is required", false))?;
        let artifact_id = Uuid::parse_str(artifact_id_str)
            .map_err(|_| tool_err("invalid_input", "invalid artifact_id UUID", false))?;

        validate_channel_name(channel_name)?;

        let channel = af_db::notifications::get_channel_by_name(
            &self.state.pool,
            ctx.project_id,
            channel_name,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to lookup channel: {e}"), true))?
        .ok_or_else(|| {
            tool_err(
                "channel_not_found",
                format!("no channel named '{channel_name}' in this project"),
                false,
            )
        })?;

        if !channel.enabled {
            return Err(tool_err(
                "channel_disabled",
                format!("channel '{channel_name}' is disabled"),
                false,
            ));
        }

        if channel.channel_type != "webdav" {
            return Err(tool_err(
                "invalid_channel_type",
                "notify.upload only works with webdav channels",
                false,
            ));
        }

        let subject = format!("File upload: {}", input["filename"].as_str().unwrap_or(artifact_id_str));

        let row = af_db::notifications::enqueue(
            &self.state.pool,
            ctx.project_id,
            channel.id,
            &subject,
            "",
            Some(artifact_id),
            ctx.actor_user_id,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to enqueue upload: {e}"), true))?;

        Ok(ok_json(json!({
            "status": "queued",
            "id": row.id.to_string(),
            "channel": channel_name,
            "artifact_id": artifact_id_str,
        })))
    }
}

// ---------------------------------------------------------------------------
// notify.list
// ---------------------------------------------------------------------------

pub struct NotifyListExecutor {
    pub state: Arc<NotifyState>,
}

#[async_trait]
impl ToolExecutor for NotifyListExecutor {
    fn tool_name(&self) -> &str {
        "notify.list"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(&self, ctx: ToolContext, _input: Value) -> Result<ToolResult, ToolError> {
        let channels =
            af_db::notifications::list_channels(&self.state.pool, ctx.project_id)
                .await
                .map_err(|e| tool_err("db_error", format!("failed to list channels: {e}"), true))?;

        let list: Vec<Value> = channels
            .iter()
            .map(|ch| {
                json!({
                    "name": ch.name,
                    "type": ch.channel_type,
                    "enabled": ch.enabled,
                })
            })
            .collect();

        Ok(ok_json(json!({ "channels": list })))
    }
}

// ---------------------------------------------------------------------------
// notify.test
// ---------------------------------------------------------------------------

pub struct NotifyTestExecutor {
    pub state: Arc<NotifyState>,
}

#[async_trait]
impl ToolExecutor for NotifyTestExecutor {
    fn tool_name(&self) -> &str {
        "notify.test"
    }
    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let channel_name = input["channel"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "channel is required", false))?;

        let channel = af_db::notifications::get_channel_by_name(
            &self.state.pool,
            ctx.project_id,
            channel_name,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to lookup channel: {e}"), true))?
        .ok_or_else(|| {
            tool_err(
                "channel_not_found",
                format!("no channel named '{channel_name}' in this project"),
                false,
            )
        })?;

        if !channel.enabled {
            return Err(tool_err(
                "channel_disabled",
                format!("channel '{channel_name}' is disabled"),
                false,
            ));
        }

        let row = af_db::notifications::enqueue(
            &self.state.pool,
            ctx.project_id,
            channel.id,
            "Test notification from Arbeiterfarm",
            "This is a test notification to verify channel configuration.",
            None,
            ctx.actor_user_id,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to enqueue test: {e}"), true))?;

        Ok(ok_json(json!({
            "status": "queued",
            "id": row.id.to_string(),
            "channel": channel_name,
            "message": "Test notification queued for delivery",
        })))
    }
}
