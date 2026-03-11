use crate::CliConfig;
use uuid::Uuid;

pub async fn handle(config: &CliConfig, action: crate::app::UrlIngestAction) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;

    match action {
        crate::app::UrlIngestAction::Submit { project, urls } => {
            let project_id = Uuid::parse_str(&project)
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            if urls.is_empty() {
                anyhow::bail!("no URLs provided");
            }
            if urls.len() > 50 {
                anyhow::bail!("max 50 URLs per request");
            }

            // Validate URLs
            for url in &urls {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    anyhow::bail!("invalid URL (must start with http:// or https://): {url}");
                }
                if url.len() > 2048 {
                    let preview: String = url.chars().take(80).collect();
                    anyhow::bail!("URL too long (max 2048 chars): {preview}");
                }
            }

            let rows = af_db::url_ingest::enqueue_urls(&pool, project_id, &urls, None).await?;

            if rows.is_empty() {
                println!("No new URLs enqueued (all duplicates of active entries).");
            } else {
                println!("Enqueued {} URL(s):", rows.len());
                for r in &rows {
                    println!("  {} {}", r.id, r.url);
                }
            }
        }

        crate::app::UrlIngestAction::List { project, status } => {
            let project_id = project
                .map(|s| Uuid::parse_str(&s))
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rows = af_db::url_ingest::list_queue(
                &pool,
                project_id,
                status.as_deref(),
                100,
            )
            .await?;

            if rows.is_empty() {
                println!("No URL ingest items found.");
                return Ok(());
            }

            println!(
                "{:<36}  {:<12}  {:>6}  {:<40}  {:<20}  {}",
                "ID", "STATUS", "CHUNKS", "URL", "TITLE", "ERROR"
            );
            for r in &rows {
                let url_display = if r.url.chars().count() > 40 {
                    let s: String = r.url.chars().take(37).collect();
                    format!("{s}...")
                } else {
                    r.url.clone()
                };
                let title_raw = r.title.as_deref().unwrap_or("-");
                let title_display = if title_raw.chars().count() > 20 {
                    let s: String = title_raw.chars().take(17).collect();
                    format!("{s}...")
                } else {
                    title_raw.to_string()
                };
                println!(
                    "{:<36}  {:<12}  {:>6}  {:<40}  {:<20}  {}",
                    r.id,
                    r.status,
                    r.chunk_count.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
                    url_display,
                    title_display,
                    r.error_message.as_deref().unwrap_or(""),
                );
            }
        }

        crate::app::UrlIngestAction::Cancel { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::url_ingest::cancel(&pool, uuid).await? {
                println!("Cancelled URL ingest item {id}");
            } else {
                println!("Item {id} not found or not in pending status");
            }
        }

        crate::app::UrlIngestAction::Retry { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::url_ingest::retry(&pool, uuid).await? {
                println!("Reset URL ingest item {id} to pending");
            } else {
                println!("Item {id} not found or not in failed status");
            }
        }
    }

    Ok(())
}
