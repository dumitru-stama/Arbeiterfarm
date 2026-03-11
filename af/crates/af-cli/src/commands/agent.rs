use crate::app::AgentCommand;
use crate::backend::Backend;

pub async fn handle(backend: &dyn Backend, cmd: AgentCommand) -> anyhow::Result<()> {
    use crate::app::AgentAction;

    match cmd.action {
        AgentAction::List => {
            let rows = backend.list_agents().await?;
            if rows.is_empty() {
                println!("No agents found.");
            } else {
                println!("{:<20} {:<8} {:<12} {:<10} Tools", "NAME", "BUILTIN", "ROUTE", "TIMEOUT");
                println!("{}", "-".repeat(80));
                for row in rows {
                    let tools_str = row
                        .allowed_tools
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let timeout_str = row
                        .timeout_secs
                        .map(|s| format!("{}s", s))
                        .unwrap_or_else(|| "-".into());
                    println!(
                        "{:<20} {:<8} {:<12} {:<10} {}",
                        row.name,
                        if row.is_builtin { "yes" } else { "no" },
                        row.default_route,
                        timeout_str,
                        tools_str
                    );
                }
            }
        }
        AgentAction::Show { name } => {
            let row = backend.get_agent(&name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", name))?;
            println!("Name:          {}", row.name);
            println!("Builtin:       {}", row.is_builtin);
            println!("Route:         {}", row.default_route);
            println!(
                "Timeout:       {}",
                row.timeout_secs
                    .map(|s| format!("{}s", s))
                    .unwrap_or_else(|| "(none)".into())
            );
            println!("Created:       {}", row.created_at);
            println!("Updated:       {}", row.updated_at);
            let tools_str = row
                .allowed_tools
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            println!("Allowed tools: {}", tools_str);
            println!("\nSystem prompt:\n{}", row.system_prompt);
        }
        AgentAction::Create {
            name,
            prompt,
            tools,
            route,
            timeout,
        } => {
            let tools_vec: Vec<String> = tools.split(',').map(|s| s.trim().to_string()).collect();
            let tools_json = serde_json::Value::Array(
                tools_vec
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            );
            let timeout_i32 = timeout.map(|s| s as i32);
            let row = backend.upsert_agent(
                &name,
                &prompt,
                &tools_json,
                &route,
                &serde_json::json!({}),
                false,
                Some("user"),
                timeout_i32,
            )
            .await?;
            println!("Created agent: {}", row.name);
        }
        AgentAction::Delete { name } => {
            let deleted = backend.delete_agent(&name).await?;
            if deleted {
                println!("Deleted agent: {name}");
            } else {
                if let Ok(Some(row)) = backend.get_agent(&name).await {
                    if row.is_builtin {
                        anyhow::bail!("cannot delete builtin agent '{name}'");
                    }
                }
                anyhow::bail!("agent '{name}' not found");
            }
        }
        AgentAction::Promote { file, force } => {
            let agent_config = crate::commands::agent_file::load_agent_from_file(&file)?;

            if !force {
                if let Some(existing) = backend.get_agent(&agent_config.name).await? {
                    anyhow::bail!(
                        "agent '{}' already exists (created {}). Use --force to overwrite.",
                        existing.name,
                        existing.created_at.format("%Y-%m-%d %H:%M")
                    );
                }
            }

            let tools_json = agent_config.allowed_tools_json();
            let route_str = agent_config.default_route.to_db_string();
            let row = backend.upsert_agent(
                &agent_config.name,
                &agent_config.system_prompt,
                &tools_json,
                &route_str,
                &agent_config.metadata,
                false,
                Some("user"),
                agent_config.timeout_secs.map(|s| s as i32),
            )
            .await?;
            println!("Promoted agent '{}' to database.", row.name);
        }
    }
    Ok(())
}
