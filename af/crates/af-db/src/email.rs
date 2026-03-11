use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailRecipientRuleRow {
    pub id: Uuid,
    pub scope: String,
    pub project_id: Option<Uuid>,
    pub rule_type: String,
    pub pattern_type: String,
    pub pattern: String,
    pub description: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailTonePresetRow {
    pub name: String,
    pub description: Option<String>,
    pub system_instruction: String,
    pub is_builtin: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailScheduledRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub user_id: Option<Uuid>,
    pub provider: String,
    pub status: String,
    pub from_address: String,
    pub to_addresses: serde_json::Value,
    pub cc_addresses: Option<serde_json::Value>,
    pub bcc_addresses: Option<serde_json::Value>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub reply_to_msg_id: Option<String>,
    pub tone: Option<String>,
    pub scheduled_at: DateTime<Utc>,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub thread_id: Option<Uuid>,
    pub tool_run_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailLogRow {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub action: String,
    pub provider: String,
    pub from_address: Option<String>,
    pub to_addresses: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub tone: Option<String>,
    pub success: bool,
    pub error_message: Option<String>,
    pub provider_message_id: Option<String>,
    pub scheduled_email_id: Option<Uuid>,
    pub tool_run_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailCredentialRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub email_address: String,
    pub credentials_json: serde_json::Value,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Recipient rules CRUD
// ---------------------------------------------------------------------------

pub async fn list_recipient_rules<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    scope: Option<&str>,
    project_id: Option<Uuid>,
) -> Result<Vec<EmailRecipientRuleRow>, sqlx::Error> {
    if let Some(pid) = project_id {
        sqlx::query_as::<_, EmailRecipientRuleRow>(
            "SELECT * FROM email_recipient_rules WHERE (scope = 'global' OR project_id = $1) ORDER BY created_at",
        )
        .bind(pid)
        .fetch_all(db)
        .await
    } else if let Some(s) = scope {
        sqlx::query_as::<_, EmailRecipientRuleRow>(
            "SELECT * FROM email_recipient_rules WHERE scope = $1 ORDER BY created_at",
        )
        .bind(s)
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, EmailRecipientRuleRow>(
            "SELECT * FROM email_recipient_rules ORDER BY created_at",
        )
        .fetch_all(db)
        .await
    }
}

pub async fn add_recipient_rule<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    scope: &str,
    project_id: Option<Uuid>,
    rule_type: &str,
    pattern_type: &str,
    pattern: &str,
    description: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<EmailRecipientRuleRow, sqlx::Error> {
    sqlx::query_as::<_, EmailRecipientRuleRow>(
        "INSERT INTO email_recipient_rules (scope, project_id, rule_type, pattern_type, pattern, description, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *",
    )
    .bind(scope)
    .bind(project_id)
    .bind(rule_type)
    .bind(pattern_type)
    .bind(pattern)
    .bind(description)
    .bind(created_by)
    .fetch_one(db)
    .await
}

pub async fn remove_recipient_rule<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM email_recipient_rules WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Tone presets
// ---------------------------------------------------------------------------

pub async fn list_tone_presets<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
) -> Result<Vec<EmailTonePresetRow>, sqlx::Error> {
    sqlx::query_as::<_, EmailTonePresetRow>(
        "SELECT * FROM email_tone_presets ORDER BY name",
    )
    .fetch_all(db)
    .await
}

pub async fn get_tone_preset<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    name: &str,
) -> Result<Option<EmailTonePresetRow>, sqlx::Error> {
    sqlx::query_as::<_, EmailTonePresetRow>(
        "SELECT * FROM email_tone_presets WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(db)
    .await
}

pub async fn upsert_tone_preset<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    name: &str,
    description: Option<&str>,
    system_instruction: &str,
    is_builtin: bool,
    created_by: Option<Uuid>,
) -> Result<EmailTonePresetRow, sqlx::Error> {
    sqlx::query_as::<_, EmailTonePresetRow>(
        "INSERT INTO email_tone_presets (name, description, system_instruction, is_builtin, created_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (name) DO UPDATE SET
           description = EXCLUDED.description,
           system_instruction = EXCLUDED.system_instruction,
           updated_at = NOW()
         WHERE email_tone_presets.is_builtin = false
         RETURNING *",
    )
    .bind(name)
    .bind(description)
    .bind(system_instruction)
    .bind(is_builtin)
    .bind(created_by)
    .fetch_one(db)
    .await
}

pub async fn delete_tone_preset<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM email_tone_presets WHERE name = $1 AND is_builtin = false")
        .bind(name)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Scheduled emails
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn create_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Uuid,
    user_id: Option<Uuid>,
    provider: &str,
    from_address: &str,
    to_addresses: &serde_json::Value,
    cc_addresses: &serde_json::Value,
    bcc_addresses: &serde_json::Value,
    subject: &str,
    body_text: Option<&str>,
    body_html: Option<&str>,
    reply_to_msg_id: Option<&str>,
    tone: Option<&str>,
    scheduled_at: DateTime<Utc>,
    thread_id: Option<Uuid>,
    tool_run_id: Option<Uuid>,
) -> Result<EmailScheduledRow, sqlx::Error> {
    sqlx::query_as::<_, EmailScheduledRow>(
        "INSERT INTO email_scheduled
         (project_id, user_id, provider, from_address, to_addresses, cc_addresses, bcc_addresses,
          subject, body_text, body_html, reply_to_msg_id, tone, scheduled_at, thread_id, tool_run_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         RETURNING *",
    )
    .bind(project_id)
    .bind(user_id)
    .bind(provider)
    .bind(from_address)
    .bind(to_addresses)
    .bind(cc_addresses)
    .bind(bcc_addresses)
    .bind(subject)
    .bind(body_text)
    .bind(body_html)
    .bind(reply_to_msg_id)
    .bind(tone)
    .bind(scheduled_at)
    .bind(thread_id)
    .bind(tool_run_id)
    .fetch_one(db)
    .await
}

pub async fn list_due_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    limit: i64,
) -> Result<Vec<EmailScheduledRow>, sqlx::Error> {
    sqlx::query_as::<_, EmailScheduledRow>(
        "SELECT * FROM email_scheduled WHERE status = 'scheduled' AND scheduled_at <= NOW()
         ORDER BY scheduled_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn claim_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE email_scheduled SET status = 'sending', updated_at = NOW()
         WHERE id = $1 AND status = 'scheduled'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn complete_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    _provider_message_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    // provider_message_id is captured in email_log, not on this row
    sqlx::query(
        "UPDATE email_scheduled SET status = 'sent', sent_at = NOW(), updated_at = NOW(),
         error_message = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn fail_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
    error_msg: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE email_scheduled SET
           attempt_count = attempt_count + 1,
           error_message = $2,
           status = CASE WHEN attempt_count + 1 >= max_attempts THEN 'failed' ELSE 'scheduled' END,
           updated_at = NOW()
         WHERE id = $1",
    )
    .bind(id)
    .bind(error_msg)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn cancel_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE email_scheduled SET status = 'cancelled', updated_at = NOW()
         WHERE id = $1 AND status = 'scheduled'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_scheduled<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
    status: Option<&str>,
) -> Result<Vec<EmailScheduledRow>, sqlx::Error> {
    match (project_id, status) {
        (Some(pid), Some(st)) => {
            sqlx::query_as::<_, EmailScheduledRow>(
                "SELECT * FROM email_scheduled WHERE project_id = $1 AND status = $2 ORDER BY scheduled_at DESC",
            )
            .bind(pid)
            .bind(st)
            .fetch_all(db)
            .await
        }
        (Some(pid), None) => {
            sqlx::query_as::<_, EmailScheduledRow>(
                "SELECT * FROM email_scheduled WHERE project_id = $1 ORDER BY scheduled_at DESC",
            )
            .bind(pid)
            .fetch_all(db)
            .await
        }
        (None, Some(st)) => {
            sqlx::query_as::<_, EmailScheduledRow>(
                "SELECT * FROM email_scheduled WHERE status = $1 ORDER BY scheduled_at DESC",
            )
            .bind(st)
            .fetch_all(db)
            .await
        }
        (None, None) => {
            sqlx::query_as::<_, EmailScheduledRow>(
                "SELECT * FROM email_scheduled ORDER BY scheduled_at DESC",
            )
            .fetch_all(db)
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Email log
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn insert_email_log<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
    action: &str,
    provider: &str,
    from_address: Option<&str>,
    to_addresses: Option<&serde_json::Value>,
    subject: Option<&str>,
    tone: Option<&str>,
    success: bool,
    error_message: Option<&str>,
    provider_message_id: Option<&str>,
    scheduled_email_id: Option<Uuid>,
    tool_run_id: Option<Uuid>,
    thread_id: Option<Uuid>,
    metadata: Option<&serde_json::Value>,
) -> Result<Uuid, sqlx::Error> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO email_log
         (project_id, user_id, action, provider, from_address, to_addresses, subject, tone,
          success, error_message, provider_message_id, scheduled_email_id, tool_run_id, thread_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         RETURNING id",
    )
    .bind(project_id)
    .bind(user_id)
    .bind(action)
    .bind(provider)
    .bind(from_address)
    .bind(to_addresses)
    .bind(subject)
    .bind(tone)
    .bind(success)
    .bind(error_message)
    .bind(provider_message_id)
    .bind(scheduled_email_id)
    .bind(tool_run_id)
    .bind(thread_id)
    .bind(metadata)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

pub async fn list_email_log<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<EmailLogRow>, sqlx::Error> {
    match (project_id, user_id) {
        (Some(pid), Some(uid)) => {
            sqlx::query_as::<_, EmailLogRow>(
                "SELECT * FROM email_log WHERE project_id = $1 AND user_id = $2
                 ORDER BY created_at DESC LIMIT $3",
            )
            .bind(pid)
            .bind(uid)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (Some(pid), None) => {
            sqlx::query_as::<_, EmailLogRow>(
                "SELECT * FROM email_log WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(pid)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, Some(uid)) => {
            sqlx::query_as::<_, EmailLogRow>(
                "SELECT * FROM email_log WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2",
            )
            .bind(uid)
            .bind(limit)
            .fetch_all(db)
            .await
        }
        (None, None) => {
            sqlx::query_as::<_, EmailLogRow>(
                "SELECT * FROM email_log ORDER BY created_at DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(db)
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

pub async fn upsert_credential<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
    provider: &str,
    email_address: &str,
    credentials_json: &serde_json::Value,
    is_default: bool,
) -> Result<EmailCredentialRow, sqlx::Error> {
    sqlx::query_as::<_, EmailCredentialRow>(
        "INSERT INTO email_credentials (user_id, provider, email_address, credentials_json, is_default)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (user_id, provider, email_address) DO UPDATE SET
           credentials_json = EXCLUDED.credentials_json,
           is_default = EXCLUDED.is_default,
           updated_at = NOW()
         RETURNING *",
    )
    .bind(user_id)
    .bind(provider)
    .bind(email_address)
    .bind(credentials_json)
    .bind(is_default)
    .fetch_one(db)
    .await
}

pub async fn list_credentials<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
) -> Result<Vec<EmailCredentialRow>, sqlx::Error> {
    sqlx::query_as::<_, EmailCredentialRow>(
        "SELECT * FROM email_credentials WHERE user_id = $1 ORDER BY is_default DESC, created_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn list_all_credentials<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
) -> Result<Vec<EmailCredentialRow>, sqlx::Error> {
    sqlx::query_as::<_, EmailCredentialRow>(
        "SELECT * FROM email_credentials ORDER BY created_at",
    )
    .fetch_all(db)
    .await
}

pub async fn get_default_credential<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
    provider: Option<&str>,
) -> Result<Option<EmailCredentialRow>, sqlx::Error> {
    if let Some(prov) = provider {
        sqlx::query_as::<_, EmailCredentialRow>(
            "SELECT * FROM email_credentials WHERE user_id = $1 AND provider = $2
             ORDER BY is_default DESC, created_at LIMIT 1",
        )
        .bind(user_id)
        .bind(prov)
        .fetch_optional(db)
        .await
    } else {
        sqlx::query_as::<_, EmailCredentialRow>(
            "SELECT * FROM email_credentials WHERE user_id = $1
             ORDER BY is_default DESC, created_at LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(db)
        .await
    }
}

pub async fn delete_credential<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM email_credentials WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
