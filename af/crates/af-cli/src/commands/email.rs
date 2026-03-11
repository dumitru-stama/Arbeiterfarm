use crate::app::EmailAction;
use crate::backend::Backend;
use uuid::Uuid;

pub async fn handle(action: EmailAction, backend: &dyn Backend) -> anyhow::Result<()> {
    match action {
        EmailAction::Setup {
            provider,
            user,
            address,
            credentials,
            default,
        } => {
            let user_id: Uuid = user.parse().map_err(|_| anyhow::anyhow!("invalid user UUID"))?;

            // Parse credentials: JSON string or @file path
            let creds_str = if credentials.starts_with('@') {
                let path = &credentials[1..];
                std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("failed to read credentials file '{path}': {e}"))?
            } else {
                credentials
            };

            let creds_json: serde_json::Value = serde_json::from_str(&creds_str)
                .map_err(|e| anyhow::anyhow!("invalid credentials JSON: {e}"))?;

            let row = backend
                .upsert_email_credential(user_id, &provider, &address, &creds_json, default)
                .await?;

            println!("Email credential configured:");
            println!("  id:       {}", row.id);
            println!("  provider: {}", row.provider);
            println!("  address:  {}", row.email_address);
            println!("  default:  {}", row.is_default);
        }

        EmailAction::Accounts { user } => {
            let user_id: Uuid = user.parse().map_err(|_| anyhow::anyhow!("invalid user UUID"))?;
            let creds = backend.list_email_credentials(user_id).await?;
            if creds.is_empty() {
                println!("No email accounts configured for user {user}.");
                return Ok(());
            }

            println!("{:<38} {:<12} {:<30} {}", "ID", "PROVIDER", "ADDRESS", "DEFAULT");
            for c in &creds {
                println!(
                    "{:<38} {:<12} {:<30} {}",
                    c.id, c.provider, c.email_address,
                    if c.is_default { "yes" } else { "no" }
                );
            }
        }

        EmailAction::RemoveAccount { id } => {
            let cred_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid UUID"))?;
            let removed = backend.delete_email_credential(cred_id).await?;
            if removed {
                println!("Removed email credential {id}");
            } else {
                println!("Credential {id} not found");
            }
        }

        EmailAction::Tones(tones_cmd) => match tones_cmd.action {
            crate::app::EmailTonesAction::List => {
                let presets = backend.list_email_tone_presets().await?;
                if presets.is_empty() {
                    println!("No tone presets configured.");
                    return Ok(());
                }

                println!("{:<20} {:<8} {}", "NAME", "BUILTIN", "DESCRIPTION");
                for p in &presets {
                    println!(
                        "{:<20} {:<8} {}",
                        p.name,
                        if p.is_builtin { "yes" } else { "no" },
                        p.description.as_deref().unwrap_or("")
                    );
                }
            }
            crate::app::EmailTonesAction::Add {
                name,
                description,
                instruction,
            } => {
                let preset = backend
                    .upsert_email_tone_preset(&name, Some(&description), &instruction)
                    .await?;
                println!("Tone preset '{}' created/updated.", preset.name);
            }
            crate::app::EmailTonesAction::Remove { name } => {
                let removed = backend.delete_email_tone_preset(&name).await?;
                if removed {
                    println!("Removed tone preset '{name}'");
                } else {
                    println!("Preset '{name}' not found or is a builtin preset (cannot delete builtins)");
                }
            }
        },

        EmailAction::Scheduled { project, status } => {
            let project_id = project
                .as_deref()
                .map(|p| p.parse::<Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let emails = backend.list_scheduled_emails(project_id, status.as_deref()).await?;
            if emails.is_empty() {
                println!("No scheduled emails.");
                return Ok(());
            }

            println!("{:<38} {:<10} {:<24} {:<30} {}", "ID", "STATUS", "SCHEDULED_AT", "TO", "SUBJECT");
            for e in &emails {
                let to_str = e
                    .to_addresses
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!(
                    "{:<38} {:<10} {:<24} {:<30} {}",
                    e.id,
                    e.status,
                    e.scheduled_at.format("%Y-%m-%d %H:%M:%S UTC"),
                    to_str,
                    e.subject
                );
            }
        }

        EmailAction::Cancel { id } => {
            let email_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid UUID"))?;
            let cancelled = backend.cancel_scheduled_email(email_id).await?;
            if cancelled {
                println!("Cancelled scheduled email {id}");
            } else {
                println!("Email {id} not found or not in 'scheduled' state");
            }
        }
    }

    Ok(())
}
