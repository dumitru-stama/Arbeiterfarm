use crate::CliConfig;
use uuid::Uuid;

pub async fn handle(config: &CliConfig, action: crate::app::EmbedQueueAction) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;

    match action {
        crate::app::EmbedQueueAction::List { project, status } => {
            let project_id = project
                .map(|s| Uuid::parse_str(&s))
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rows = af_db::embed_queue::list_queue(
                &pool,
                project_id,
                status.as_deref(),
                100,
            )
            .await?;

            if rows.is_empty() {
                println!("No embed queue items found.");
                return Ok(());
            }

            println!(
                "{:<36}  {:<12}  {:>6}  {:>8}  {:<12}  {:<20}  {}",
                "ID", "STATUS", "CHUNKS", "EMBEDDED", "TOOL", "CREATED", "ERROR"
            );
            for r in &rows {
                println!(
                    "{:<36}  {:<12}  {:>6}  {:>8}  {:<12}  {:<20}  {}",
                    r.id,
                    r.status,
                    r.chunk_count.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
                    r.chunks_embedded,
                    r.tool_name,
                    r.created_at.format("%Y-%m-%d %H:%M:%S"),
                    r.error_message.as_deref().unwrap_or(""),
                );
            }
        }

        crate::app::EmbedQueueAction::Cancel { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::embed_queue::cancel(&pool, uuid).await? {
                println!("Cancelled embed queue item {id}");
            } else {
                println!("Item {id} not found or not in pending status");
            }
        }

        crate::app::EmbedQueueAction::Retry { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::embed_queue::retry(&pool, uuid).await? {
                println!("Reset embed queue item {id} to pending");
            } else {
                println!("Item {id} not found or not in failed status");
            }
        }
    }

    Ok(())
}
