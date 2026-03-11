use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: Uuid,
    #[serde(default)]
    pub thread_id: Uuid,
    pub role: String,
    pub content: Option<String>,
    #[serde(default)]
    pub content_json: Option<serde_json::Value>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub agent_name: Option<String>,
    #[serde(default)]
    pub seq: i64,
    pub created_at: DateTime<Utc>,
}

pub async fn insert_message(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    role: &str,
    content: Option<&str>,
    content_json: Option<&serde_json::Value>,
) -> Result<MessageRow, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (thread_id, role, content, content_json) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at",
    )
    .bind(thread_id)
    .bind(role)
    .bind(content)
    .bind(content_json)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Insert a message with agent_name attribution.
pub async fn insert_message_with_agent(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    role: &str,
    content: Option<&str>,
    content_json: Option<&serde_json::Value>,
    agent_name: Option<&str>,
) -> Result<MessageRow, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (thread_id, role, content, content_json, agent_name) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at",
    )
    .bind(thread_id)
    .bind(role)
    .bind(content)
    .bind(content_json)
    .bind(agent_name)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Insert a message with tool_call_id and tool_name (for tool result messages).
pub async fn insert_tool_message(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    role: &str,
    content: Option<&str>,
    content_json: Option<&serde_json::Value>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
) -> Result<MessageRow, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (thread_id, role, content, content_json, tool_call_id, tool_name) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at",
    )
    .bind(thread_id)
    .bind(role)
    .bind(content)
    .bind(content_json)
    .bind(tool_call_id)
    .bind(tool_name)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Insert a tool message with agent_name attribution.
pub async fn insert_tool_message_with_agent(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    role: &str,
    content: Option<&str>,
    content_json: Option<&serde_json::Value>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
    agent_name: Option<&str>,
) -> Result<MessageRow, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (thread_id, role, content, content_json, tool_call_id, tool_name, agent_name) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at",
    )
    .bind(thread_id)
    .bind(role)
    .bind(content)
    .bind(content_json)
    .bind(tool_call_id)
    .bind(tool_name)
    .bind(agent_name)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_thread_messages(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MessageRow>(
        "SELECT id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at \
         FROM messages WHERE thread_id = $1 ORDER BY seq ASC",
    )
    .bind(thread_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Get thread messages excluding compacted ones (for context building).
pub async fn get_thread_messages_compacted(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MessageRow>(
        "SELECT id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at \
         FROM messages WHERE thread_id = $1 AND NOT compacted ORDER BY seq ASC",
    )
    .bind(thread_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Fetch new user messages inserted after a given sequence number.
/// Used by the agent runtime to pick up messages queued via the /queue endpoint
/// while a tool-call loop is running.
pub async fn get_new_user_messages_since(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    since_seq: i64,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MessageRow>(
        "SELECT id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at \
         FROM messages WHERE thread_id = $1 AND seq > $2 AND role = 'user' AND NOT compacted ORDER BY seq ASC",
    )
    .bind(thread_id)
    .bind(since_seq)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Mark messages as compacted up to (and including) a given seq number.
/// System messages and already-compacted messages are excluded.
pub async fn mark_messages_compacted(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    up_to_seq: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE messages SET compacted = TRUE \
         WHERE thread_id = $1 AND seq <= $2 AND role != 'system' AND NOT compacted",
    )
    .bind(thread_id)
    .bind(up_to_seq)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Insert a compaction summary message.
/// Stored as a system message with metadata in content_json.
pub async fn insert_compaction_summary(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    thread_id: Uuid,
    summary: &str,
    compacted_count: usize,
    up_to_seq: i64,
    agent_name: Option<&str>,
) -> Result<MessageRow, sqlx::Error> {
    let content_json = serde_json::json!({
        "compaction_summary": true,
        "compacted_count": compacted_count,
        "compacted_up_to_seq": up_to_seq,
    });
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (thread_id, role, content, content_json, agent_name) \
         VALUES ($1, 'system', $2, $3, $4) \
         RETURNING id, thread_id, role, content, content_json, tool_call_id, tool_name, agent_name, seq, created_at",
    )
    .bind(thread_id)
    .bind(summary)
    .bind(&content_json)
    .bind(agent_name)
    .fetch_one(db)
    .await?;
    Ok(row)
}
