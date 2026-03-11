use crate::CliConfig;
use uuid::Uuid;

pub async fn handle(config: &CliConfig, action: crate::app::NotifyAction) -> anyhow::Result<()> {
    match action {
        crate::app::NotifyAction::Channel(cmd) => handle_channel(config, cmd.action).await,
        crate::app::NotifyAction::Queue(cmd) => handle_queue(config, cmd.action).await,
    }
}

async fn handle_channel(
    config: &CliConfig,
    action: crate::app::NotifyChannelAction,
) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;

    match action {
        crate::app::NotifyChannelAction::Add {
            project,
            name,
            channel_type,
            config: config_str,
        } => {
            let project_id = Uuid::parse_str(&project)
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            // Validate channel type
            if !["webhook", "email", "matrix", "webdav"].contains(&channel_type.as_str()) {
                anyhow::bail!(
                    "invalid channel type '{}' (must be webhook, email, matrix, or webdav)",
                    channel_type
                );
            }

            let config_json: serde_json::Value = serde_json::from_str(&config_str)
                .map_err(|e| anyhow::anyhow!("invalid config JSON: {e}"))?;

            // Validate config by type
            validate_channel_config(&channel_type, &config_json)?;

            let row = af_db::notifications::create_channel(
                &pool,
                project_id,
                &name,
                &channel_type,
                &config_json,
            )
            .await?;

            println!("Created channel: {} ({}) id={}", row.name, row.channel_type, row.id);
        }

        crate::app::NotifyChannelAction::List { project } => {
            let project_id = Uuid::parse_str(&project)
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rows = af_db::notifications::list_channels(&pool, project_id).await?;

            if rows.is_empty() {
                println!("No notification channels found.");
                return Ok(());
            }

            println!(
                "{:<36}  {:<20}  {:<10}  {:<8}",
                "ID", "NAME", "TYPE", "ENABLED"
            );
            for r in &rows {
                println!(
                    "{:<36}  {:<20}  {:<10}  {:<8}",
                    r.id,
                    r.name,
                    r.channel_type,
                    if r.enabled { "yes" } else { "no" },
                );
            }
        }

        crate::app::NotifyChannelAction::Remove { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::notifications::delete_channel(&pool, uuid).await? {
                println!("Deleted channel {id}");
            } else {
                println!("Channel {id} not found");
            }
        }

        crate::app::NotifyChannelAction::Test { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;

            let channel = af_db::notifications::get_channel(&pool, uuid)
                .await?
                .ok_or_else(|| anyhow::anyhow!("channel {id} not found"))?;

            let row = af_db::notifications::enqueue(
                &pool,
                channel.project_id,
                channel.id,
                "Test notification from Arbeiterfarm",
                "This is a test notification to verify channel configuration.",
                None,
                None,
            )
            .await?;

            println!(
                "Test notification queued: {} (run 'af tick' to deliver)",
                row.id
            );
        }
    }

    Ok(())
}

async fn handle_queue(
    config: &CliConfig,
    action: crate::app::NotifyQueueAction,
) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;

    match action {
        crate::app::NotifyQueueAction::List { project, status } => {
            let project_id = project
                .map(|s| Uuid::parse_str(&s))
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rows = af_db::notifications::list_queue(
                &pool,
                project_id,
                status.as_deref(),
                100,
            )
            .await?;

            if rows.is_empty() {
                println!("No notification queue items found.");
                return Ok(());
            }

            println!(
                "{:<36}  {:<12}  {:<36}  {:<30}  {}",
                "ID", "STATUS", "CHANNEL_ID", "SUBJECT", "ERROR"
            );
            for r in &rows {
                let subject_display = if r.subject.chars().count() > 30 {
                    let s: String = r.subject.chars().take(27).collect();
                    format!("{s}...")
                } else {
                    r.subject.clone()
                };
                println!(
                    "{:<36}  {:<12}  {:<36}  {:<30}  {}",
                    r.id,
                    r.status,
                    r.channel_id,
                    subject_display,
                    r.error_message.as_deref().unwrap_or(""),
                );
            }
        }

        crate::app::NotifyQueueAction::Cancel { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::notifications::cancel(&pool, uuid).await? {
                println!("Cancelled notification {id}");
            } else {
                println!("Notification {id} not found or not in pending status");
            }
        }

        crate::app::NotifyQueueAction::Retry { id } => {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| anyhow::anyhow!("invalid UUID: {id}"))?;
            if af_db::notifications::retry(&pool, uuid).await? {
                println!("Reset notification {id} to pending");
            } else {
                println!("Notification {id} not found or not in failed status");
            }
        }
    }

    Ok(())
}

fn validate_channel_config(
    channel_type: &str,
    config: &serde_json::Value,
) -> anyhow::Result<()> {
    match channel_type {
        "webhook" => {
            let url = config["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("webhook config requires 'url' field"))?;
            if !url.starts_with("https://") {
                anyhow::bail!("webhook URL must use https://");
            }
        }
        "email" => {
            let to = config["to"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("email config requires 'to' array"))?;
            if to.is_empty() {
                anyhow::bail!("email config 'to' must not be empty");
            }
            if config["credential_id"].as_str().is_none() {
                anyhow::bail!("email config requires 'credential_id'");
            }
        }
        "matrix" => {
            if config["homeserver"].as_str().is_none() {
                anyhow::bail!("matrix config requires 'homeserver'");
            }
            if config["room_id"].as_str().is_none() {
                anyhow::bail!("matrix config requires 'room_id'");
            }
            if config["access_token"].as_str().is_none() {
                anyhow::bail!("matrix config requires 'access_token'");
            }
        }
        "webdav" => {
            let url = config["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("webdav config requires 'url' field"))?;
            if !url.starts_with("https://") {
                anyhow::bail!("webdav URL must use https://");
            }
        }
        _ => anyhow::bail!("unknown channel type: {channel_type}"),
    }
    Ok(())
}
