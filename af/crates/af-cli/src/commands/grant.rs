use crate::app::GrantCommand;
use crate::backend::Backend;

pub async fn handle(backend: &dyn Backend, cmd: GrantCommand) -> anyhow::Result<()> {
    use crate::app::GrantAction;

    match cmd.action {
        GrantAction::Tool { user_id, pattern } => {
            let uid: uuid::Uuid = user_id
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid user UUID: {user_id}"))?;
            let grant = backend.add_user_grant(uid, &pattern).await?;
            println!("Granted '{}' to user {} (id: {})", grant.tool_pattern, &user_id[..8], grant.id);
        }

        GrantAction::Revoke { user_id, pattern } => {
            let uid: uuid::Uuid = user_id
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid user UUID: {user_id}"))?;
            let deleted = backend.remove_user_grant(uid, &pattern).await?;
            if deleted {
                println!("Revoked '{}' from user {}", pattern, &user_id[..8]);
            } else {
                println!("No grant '{}' found for user {}", pattern, &user_id[..8]);
            }
        }

        GrantAction::List { user_id } => {
            let uid: uuid::Uuid = user_id
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid user UUID: {user_id}"))?;
            let grants = backend.list_user_grants(uid).await?;
            if grants.is_empty() {
                println!("No tool grants for user {}.", &user_id[..8]);
            } else {
                for g in &grants {
                    println!("{} | {} | granted {}", &g.id.to_string()[..8], g.tool_pattern, g.created_at.format("%Y-%m-%d"));
                }
            }
        }

        GrantAction::Restricted => {
            let restricted = backend.list_restricted_tools().await?;
            if restricted.is_empty() {
                println!("No tools are currently restricted.");
            } else {
                for r in &restricted {
                    println!(
                        "{} | {}",
                        r.tool_pattern,
                        r.description.as_deref().unwrap_or("no description")
                    );
                }
            }
        }

        GrantAction::Restrict {
            pattern,
            description,
        } => {
            let desc = description.as_deref().unwrap_or("Restricted by admin");
            let row = backend.add_restricted_tool(&pattern, desc).await?;
            println!("Restricted tool pattern: {}", row.tool_pattern);
        }

        GrantAction::Unrestrict { pattern } => {
            let deleted = backend.remove_restricted_tool(&pattern).await?;
            if deleted {
                println!("Unrestricted: {pattern}");
            } else {
                println!("Pattern '{pattern}' was not restricted");
            }
        }
    }

    Ok(())
}
