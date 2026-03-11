use crate::app::{HookAction, HookCommand};
use crate::backend::Backend;
use uuid::Uuid;

pub async fn handle(backend: &dyn Backend, cmd: HookCommand) -> anyhow::Result<()> {
    match cmd.action {
        HookAction::List { project } => {
            let project_id: Uuid = project.parse().map_err(|_| anyhow::anyhow!("invalid project UUID"))?;
            let hooks = backend.list_hooks(project_id).await?;
            if hooks.is_empty() {
                println!("No hooks for project {project_id}");
                return Ok(());
            }
            println!(
                "{:<36}  {:<20}  {:<20}  {:<8}  {:<12}",
                "ID", "NAME", "EVENT", "ENABLED", "TARGET"
            );
            println!("{}", "-".repeat(100));
            for h in hooks {
                let target = h
                    .workflow_name
                    .as_deref()
                    .map(|w| format!("wf:{w}"))
                    .or_else(|| h.agent_name.as_deref().map(|a| format!("agent:{a}")))
                    .unwrap_or_default();
                println!(
                    "{:<36}  {:<20}  {:<20}  {:<8}  {:<12}",
                    h.id, h.name, h.event_type, h.enabled, target,
                );
            }
        }
        HookAction::Create {
            project,
            name,
            event,
            workflow,
            agent,
            prompt,
            route,
            interval,
        } => {
            let project_id: Uuid = project.parse().map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            if workflow.is_none() && agent.is_none() {
                anyhow::bail!("either --workflow or --agent is required");
            }

            if event == "tick" && interval.is_none() {
                anyhow::bail!("--interval is required for tick hooks");
            }

            let hook = backend.create_hook(
                project_id,
                &name,
                &event,
                workflow.as_deref(),
                agent.as_deref(),
                &prompt,
                route.as_deref(),
                interval,
            )
            .await?;

            println!("Created hook: {} ({})", hook.id, hook.name);
        }
        HookAction::Show { id } => {
            let hook_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid hook UUID"))?;
            let hook = backend.get_hook(hook_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("hook {hook_id} not found"))?;

            println!("ID:            {}", hook.id);
            println!("Project:       {}", hook.project_id);
            println!("Name:          {}", hook.name);
            println!("Enabled:       {}", hook.enabled);
            println!("Event:         {}", hook.event_type);
            if let Some(ref wf) = hook.workflow_name {
                println!("Workflow:      {wf}");
            }
            if let Some(ref ag) = hook.agent_name {
                println!("Agent:         {ag}");
            }
            println!("Prompt:        {}", hook.prompt_template);
            if let Some(ref r) = hook.route_override {
                println!("Route:         {r}");
            }
            if let Some(mins) = hook.tick_interval_minutes {
                println!("Interval:      {mins} min");
            }
            if let Some(ref t) = hook.last_tick_at {
                println!("Last tick:     {t}");
            }
            println!("Tick gen:      {}", hook.tick_generation);
            println!("Created:       {}", hook.created_at);
            println!("Updated:       {}", hook.updated_at);
        }
        HookAction::Enable { id } => {
            let hook_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid hook UUID"))?;
            backend.update_hook(hook_id, Some(true), None, None, None)
                .await?
                .ok_or_else(|| anyhow::anyhow!("hook {hook_id} not found"))?;
            println!("Enabled hook {hook_id}");
        }
        HookAction::Disable { id } => {
            let hook_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid hook UUID"))?;
            backend.update_hook(hook_id, Some(false), None, None, None)
                .await?
                .ok_or_else(|| anyhow::anyhow!("hook {hook_id} not found"))?;
            println!("Disabled hook {hook_id}");
        }
        HookAction::Delete { id } => {
            let hook_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid hook UUID"))?;
            let deleted = backend.delete_hook(hook_id).await?;
            if deleted {
                println!("Deleted hook {hook_id}");
            } else {
                anyhow::bail!("hook {hook_id} not found");
            }
        }
    }
    Ok(())
}
