use crate::app::{ApiKeyAction, UserAction, UserCommand};
use crate::backend::Backend;
use uuid::Uuid;

pub async fn handle(backend: &dyn Backend, cmd: UserCommand) -> anyhow::Result<()> {
    match cmd.action {
        UserAction::Create { name, display, email, roles } => {
            let role_list: Vec<String> = roles.split(',').map(|s| s.trim().to_string()).collect();
            let user = backend.create_user(
                &name,
                display.as_deref(),
                email.as_deref(),
                &role_list,
            )
            .await?;
            println!("Created user:");
            println!("  id:      {}", user.id);
            println!("  subject: {}", user.subject);
            if let Some(d) = &user.display_name {
                println!("  display: {d}");
            }
            if let Some(e) = &user.email {
                println!("  email:   {e}");
            }
            println!("  roles:   {:?}", user.roles);
        }
        UserAction::List => {
            let users = backend.list_users().await?;
            if users.is_empty() {
                println!("No users found.");
                return Ok(());
            }
            for u in &users {
                println!(
                    "{} | {} | roles={:?} | enabled={} | {}",
                    u.id,
                    u.subject,
                    u.roles,
                    u.enabled,
                    u.created_at.format("%Y-%m-%d %H:%M"),
                );
            }
        }
        UserAction::ApiKey(ak_cmd) => match ak_cmd.action {
            ApiKeyAction::Create { user, name } => {
                let user_id: Uuid = user.parse().map_err(|_| anyhow::anyhow!("invalid user UUID: {user}"))?;

                let (raw_key, row) = backend.create_api_key(user_id, &name).await?;

                println!("API key created:");
                println!("  id:     {}", row.id);
                println!("  prefix: {}", row.key_prefix);
                println!("  name:   {}", row.name);
                println!();
                println!("  Raw key (will NOT be shown again):");
                println!("  {raw_key}");
            }
            ApiKeyAction::List { user } => {
                let user_id: Uuid = user.parse().map_err(|_| anyhow::anyhow!("invalid user UUID: {user}"))?;
                let keys = backend.list_api_keys(user_id).await?;
                if keys.is_empty() {
                    println!("No API keys found for user {user_id}.");
                    return Ok(());
                }
                for k in &keys {
                    println!(
                        "{} | {}... | {} | last_used={} | {}",
                        k.id,
                        k.key_prefix,
                        k.name,
                        k.last_used_at
                            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "never".to_string()),
                        k.created_at.format("%Y-%m-%d %H:%M"),
                    );
                }
            }
            ApiKeyAction::Revoke { id } => {
                let key_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid key UUID: {id}"))?;
                let deleted = backend.revoke_api_key(key_id).await?;
                if deleted {
                    println!("API key {key_id} revoked.");
                } else {
                    println!("API key {key_id} not found.");
                }
            }
        },
        UserAction::Routes(routes_cmd) => {
            let user_id: Uuid = routes_cmd.user_id.parse()
                .map_err(|_| anyhow::anyhow!("invalid user UUID: {}", routes_cmd.user_id))?;

            if let Some(route) = routes_cmd.add {
                backend.add_user_route(user_id, &route).await?;
                println!("Added route '{}' for user {user_id}", route);
            } else if let Some(route) = routes_cmd.remove {
                let deleted = backend.remove_user_route(user_id, &route).await?;
                if deleted {
                    println!("Removed route '{}' for user {user_id}", route);
                } else {
                    println!("Route '{}' not found for user {user_id}", route);
                }
            } else if routes_cmd.clear {
                let count = backend.clear_user_routes(user_id).await?;
                println!("Cleared {count} route(s) for user {user_id} (now unrestricted)");
            }

            // Always show current state
            let routes = backend.list_user_routes(user_id).await?;
            if routes.is_empty() {
                println!("User {user_id}: unrestricted (all models allowed)");
            } else {
                println!("User {user_id}: allowed routes:");
                for r in &routes {
                    println!("  - {r}");
                }
            }
        }
    }
    Ok(())
}
