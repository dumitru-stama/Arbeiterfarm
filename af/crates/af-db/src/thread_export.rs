use crate::message_evidence::MessageEvidenceRow;
use crate::messages::MessageRow;
use crate::threads::ThreadRow;
use sqlx::PgConnection;
use uuid::Uuid;

pub enum ExportFormat {
    Markdown,
    Json,
}

/// Run thread export within a provided connection (can be a scoped transaction).
pub async fn run_thread_export(
    conn: &mut PgConnection,
    thread_id: Uuid,
    format: ExportFormat,
) -> anyhow::Result<String> {
    let thread = crate::threads::get_thread(&mut *conn, thread_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("thread not found: {thread_id}"))?;

    let messages = crate::messages::get_thread_messages(&mut *conn, thread_id).await?;
    let evidence = crate::message_evidence::get_for_thread(&mut *conn, thread_id).await?;

    match format {
        ExportFormat::Markdown => Ok(render_markdown(&thread, &messages, &evidence)),
        ExportFormat::Json => render_json(&thread, &messages, &evidence),
    }
}

fn render_markdown(
    thread: &ThreadRow,
    messages: &[MessageRow],
    evidence: &[MessageEvidenceRow],
) -> String {
    let mut out = String::new();

    // Title
    out.push_str(&format!(
        "# {}\n\n",
        thread.title.as_deref().unwrap_or("Analysis Report")
    ));
    out.push_str(&format!("**Thread**: `{}`\n", thread.id));
    out.push_str(&format!("**Agent**: {}\n", thread.agent_name));
    if let Some(parent_id) = thread.parent_thread_id {
        out.push_str(&format!("**Parent Thread**: `{}`\n", parent_id));
    }
    out.push_str(&format!(
        "**Created**: {}\n\n",
        thread.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str("---\n\n");

    // Messages
    for msg in messages {
        match msg.role.as_str() {
            "user" => {
                out.push_str(&format!(
                    "## User\n\n{}\n\n",
                    msg.content.as_deref().unwrap_or("")
                ));
            }
            "assistant" => {
                let heading = if let Some(ref agent) = msg.agent_name {
                    format!("## Assistant [{agent}]")
                } else {
                    "## Assistant".to_string()
                };
                out.push_str(&format!(
                    "{heading}\n\n{}\n\n",
                    msg.content.as_deref().unwrap_or("")
                ));
            }
            "tool" => {
                let content = msg.content.as_deref().unwrap_or("{}");
                let formatted = if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
                    serde_json::to_string_pretty(&v).unwrap_or_else(|_| content.to_string())
                } else {
                    content.to_string()
                };
                out.push_str(&format!("### Tool Result\n\n```json\n{}\n```\n\n", formatted));
            }
            _ => {}
        }
    }

    // Evidence summary
    if !evidence.is_empty() {
        out.push_str("## Evidence References\n\n");
        for ev in evidence {
            out.push_str(&format!("- `{}:{}`\n", ev.ref_type, ev.ref_id));
        }
        out.push('\n');
    }

    out
}

fn render_json(
    thread: &ThreadRow,
    messages: &[MessageRow],
    evidence: &[MessageEvidenceRow],
) -> anyhow::Result<String> {
    let json_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id.to_string(),
                "role": m.role,
                "content": m.content,
                "agent_name": m.agent_name,
                "created_at": m.created_at.to_rfc3339(),
            })
        })
        .collect();

    let json_evidence: Vec<serde_json::Value> = evidence
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id.to_string(),
                "message_id": e.message_id.to_string(),
                "ref_type": e.ref_type,
                "ref_id": e.ref_id.to_string(),
            })
        })
        .collect();

    let output = serde_json::json!({
        "thread": {
            "id": thread.id.to_string(),
            "project_id": thread.project_id.to_string(),
            "agent_name": thread.agent_name,
            "title": thread.title,
            "parent_thread_id": thread.parent_thread_id.map(|id| id.to_string()),
            "created_at": thread.created_at.to_rfc3339(),
        },
        "messages": json_messages,
        "evidence": json_evidence,
    });

    Ok(serde_json::to_string_pretty(&output)?)
}
