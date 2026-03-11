use crate::app::{ToolAction, ToolCommand};
use crate::CliConfig;
use af_core::ToolRequest;
use af_jobs::invoker::JobQueueInvoker;
use uuid::Uuid;

/// List tools matching the allowed patterns — used by /tools slash command.
pub fn list_allowed_tools(config: &CliConfig, allowed: &[String]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut names = config.specs.list();
    names.sort();
    for name in names {
        if is_allowed(name, allowed) {
            if let Some(spec) = config.specs.get_latest(name) {
                result.push((spec.name.clone(), spec.description.clone()));
            }
        }
    }
    result
}

fn is_allowed(tool_name: &str, allowed: &[String]) -> bool {
    for pattern in allowed {
        if pattern == tool_name || pattern == "*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix(".*") {
            if tool_name.starts_with(prefix) && tool_name[prefix.len()..].starts_with('.') {
                return true;
            }
        }
    }
    false
}

pub async fn handle(config: &CliConfig, cmd: ToolCommand) -> anyhow::Result<()> {
    match cmd.action {
        ToolAction::List => {
            let mut names = config.specs.list();
            names.sort();
            if names.is_empty() {
                println!("No tools registered.");
            } else {
                for name in names {
                    if let Some(spec) = config.specs.get_latest(name) {
                        let tag = config
                            .source_map
                            .tools
                            .get(&spec.name)
                            .map(|s| format!(" [{}]", s))
                            .unwrap_or_default();
                        println!(
                            "{}  v{}{}  {}",
                            spec.name, spec.version, tag, spec.description
                        );
                    }
                }
            }
        }
        ToolAction::Enable { name } => {
            if config.specs.get_latest(&name).is_none() {
                eprintln!("Warning: tool '{name}' is not registered (enabling anyway)");
            }
            let pool = crate::get_pool_from(&config.pool).await?;
            af_db::tool_config::set_enabled(&pool, &name, true).await?;
            println!("Tool '{name}' enabled.");
        }
        ToolAction::Disable { name } => {
            if config.specs.get_latest(&name).is_none() {
                eprintln!("Warning: tool '{name}' is not registered (disabling anyway)");
            }
            let pool = crate::get_pool_from(&config.pool).await?;
            af_db::tool_config::set_enabled(&pool, &name, false).await?;
            println!("Tool '{name}' disabled.");
        }
        ToolAction::Reload => {
            println!(
                "Local TOML tools are loaded at startup. \
                 Restart the process to reload."
            );
        }
        ToolAction::Run {
            name,
            project,
            input,
        } => {
            let pool = crate::get_pool_from(&config.pool).await?;
            let project_id: Uuid = project.parse()?;
            let input_json: serde_json::Value = serde_json::from_str(&input)?;

            let invoker = JobQueueInvoker::new(
                pool.clone(),
                config.core_config.clone(),
                config.specs.clone(),
                config.executors.clone(),
            );

            let request = ToolRequest {
                tool_name: name.clone(),
                input_json,
                project_id,
                thread_id: None,
                parent_message_id: None,
                actor_user_id: None,
            };

            println!("Running {name}...");

            match af_core::ToolInvoker::invoke(&invoker, request).await {
                Ok(result) => {
                    println!("Status: completed");
                    let rendered = config.renderers.get(&name).render(&result.output_json);
                    println!("Output:\n{rendered}");
                    if !result.produced_artifacts.is_empty() {
                        println!("Produced artifacts:");
                        for aid in &result.produced_artifacts {
                            println!("  {aid}");
                        }
                    }
                }
                Err(err) => {
                    println!("Status: failed");
                    println!("Error: {err}");
                }
            }
        }
    }
    Ok(())
}
