use crate::app::{ThreadAction, ThreadCommand};
use crate::backend::Backend;
use uuid::Uuid;

pub async fn handle(backend: &dyn Backend, cmd: ThreadCommand) -> anyhow::Result<()> {
    match cmd.action {
        ThreadAction::List { project } => {
            let project_id: Uuid = project.parse()?;
            let threads = backend.list_threads(project_id).await?;
            if threads.is_empty() {
                println!("No conversations found.");
            } else {
                for t in threads {
                    let title = t.title.as_deref().unwrap_or("(untitled)");
                    let type_badge = match t.thread_type.as_str() {
                        "thinking" => " [thinking]",
                        "workflow" => " [workflow]",
                        _ => "",
                    };
                    println!(
                        "{}  {}{}  {}  {}",
                        t.id,
                        t.agent_name,
                        type_badge,
                        title,
                        t.created_at.format("%Y-%m-%d %H:%M")
                    );
                }
            }
        }
        ThreadAction::Show { id } => {
            let thread_id: Uuid = id.parse()?;
            let messages = backend.get_thread_messages(thread_id).await?;
            if messages.is_empty() {
                println!("No messages in conversation.");
            } else {
                for msg in messages {
                    let content = msg.content.as_deref().unwrap_or("(no content)");
                    let truncated = if content.len() > 200 {
                        let end = content.floor_char_boundary(200);
                        format!("{}...", &content[..end])
                    } else {
                        content.to_string()
                    };
                    println!(
                        "[{}] {} | {}",
                        msg.created_at.format("%H:%M:%S"),
                        msg.role,
                        truncated
                    );
                }
            }
        }
        ThreadAction::Delete { id, yes } => {
            let thread_id: Uuid = id.parse()?;
            if !yes {
                eprint!("Delete conversation {thread_id} and all its messages? [y/N] ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            let deleted = backend.delete_thread(thread_id).await?;
            if deleted {
                println!("Conversation {thread_id} deleted.");
            } else {
                println!("Conversation {thread_id} not found.");
            }
        }
        ThreadAction::Export { id, format } => {
            let thread_id: Uuid = id.parse()?;
            let output = backend.export_thread(thread_id, &format).await?;
            println!("{output}");
        }
        ThreadAction::QueueMessage { id, content } => {
            let thread_id: Uuid = id.parse()?;
            let msg = backend.queue_message(thread_id, &content).await?;
            println!(
                "Queued message {} (seq {}) at {}",
                msg.id,
                msg.seq,
                msg.created_at.format("%H:%M:%S")
            );
        }
    }
    Ok(())
}
