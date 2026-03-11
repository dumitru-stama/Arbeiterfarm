use crate::app::{AuditAction, AuditCommand};
use crate::backend::Backend;

pub async fn handle(backend: &dyn Backend, cmd: AuditCommand) -> anyhow::Result<()> {
    match cmd.action {
        AuditAction::List { limit, event_type } => {
            let rows =
                backend.list_audit(limit, event_type.as_deref()).await?;
            if rows.is_empty() {
                println!("No audit log entries.");
            } else {
                for row in rows {
                    let actor = row.actor_subject.as_deref().unwrap_or("-");
                    let detail_summary = row
                        .detail
                        .as_ref()
                        .and_then(|d| serde_json::to_string(d).ok())
                        .unwrap_or_default();
                    let detail_truncated = if detail_summary.len() > 80 {
                        let end = detail_summary.floor_char_boundary(80);
                        format!("{}...", &detail_summary[..end])
                    } else {
                        detail_summary
                    };
                    println!(
                        "{}  {:20}  {:20}  {}",
                        row.created_at.format("%Y-%m-%d %H:%M:%S"),
                        row.event_type,
                        actor,
                        detail_truncated,
                    );
                }
            }
        }
    }
    Ok(())
}
