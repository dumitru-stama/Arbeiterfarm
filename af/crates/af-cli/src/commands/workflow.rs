use crate::app::WorkflowCommand;
use crate::backend::Backend;
use crate::CliConfig;

pub async fn handle(config: &CliConfig, backend: &dyn Backend, cmd: WorkflowCommand) -> anyhow::Result<()> {
    use crate::app::WorkflowAction;

    match cmd.action {
        WorkflowAction::List => {
            let rows = backend.list_workflows().await?;
            if rows.is_empty() {
                println!("No workflows found.");
            } else {
                println!("{:<25} {:<8} {}", "NAME", "SOURCE", "DESCRIPTION");
                println!("{}", "-".repeat(70));
                for row in rows {
                    let desc = row.description.as_deref().unwrap_or("");
                    let source = row
                        .source_plugin
                        .as_deref()
                        .unwrap_or(if row.is_builtin { "builtin" } else { "user" });
                    println!("{:<25} {:<8} {}", row.name, source, desc);
                }
            }
        }
        WorkflowAction::Show { name } => {
            let row = backend.get_workflow(&name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("workflow '{}' not found", name))?;

            let source = row
                .source_plugin
                .as_deref()
                .unwrap_or(if row.is_builtin { "builtin" } else { "user" });

            println!("Name:        {}", row.name);
            println!("Source:      {}", source);
            println!(
                "Description: {}",
                row.description.as_deref().unwrap_or("(none)")
            );
            println!("Created:     {}", row.created_at);
            println!("Updated:     {}", row.updated_at);

            let steps: Vec<af_db::workflows::WorkflowStep> =
                serde_json::from_value(row.steps.clone()).unwrap_or_default();
            println!("\nSteps:");
            for step in &steps {
                let flags = if step.parallel { " [parallel]" } else { "" };
                println!(
                    "  Group {}: agent={}{}, prompt=\"{}\"",
                    step.group, step.agent, flags, step.prompt
                );
            }
        }
        WorkflowAction::Validate { file } => {
            // Validate is purely local — no backend needed
            let _ = config; // silence unused warning
            let path = std::path::Path::new(&file);
            match crate::local_workflows::load_local_workflows(path.parent().unwrap_or(path)) {
                ref results if results.is_empty() => {
                    // Try loading as a single file directly
                    validate_single_file(path)?;
                }
                _ => {
                    validate_single_file(path)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_single_file(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("file not found: {}", path.display());
    }

    let contents = std::fs::read_to_string(path)?;

    // Try parsing as workflow TOML
    #[derive(serde::Deserialize)]
    struct LocalWorkflowToml {
        workflow: WorkflowSection,
    }
    #[derive(serde::Deserialize)]
    struct WorkflowSection {
        name: String,
        description: Option<String>,
        steps: Vec<StepCheck>,
        agents: Option<Vec<toml::Value>>,
    }
    #[derive(serde::Deserialize)]
    struct StepCheck {
        agent: String,
        group: u32,
        prompt: String,
        #[serde(default)]
        can_repivot: Option<bool>,
        #[serde(default)]
        parallel: bool,
    }

    let doc: LocalWorkflowToml = toml::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let wf = doc.workflow;

    // Validate name
    if wf.name.is_empty() {
        anyhow::bail!("workflow name cannot be empty");
    }

    if wf.steps.is_empty() {
        anyhow::bail!("workflow must have at least one step");
    }

    for (i, step) in wf.steps.iter().enumerate() {
        if step.agent.trim().is_empty() {
            anyhow::bail!("step {}: agent cannot be empty", i + 1);
        }
        if step.prompt.trim().is_empty() {
            anyhow::bail!("step {}: prompt cannot be empty", i + 1);
        }
    }

    println!("Workflow '{}' is valid.", wf.name);
    if let Some(desc) = &wf.description {
        println!("  Description: {desc}");
    }
    println!("  Steps: {}", wf.steps.len());
    for step in &wf.steps {
        let repivot = step.can_repivot.unwrap_or(true);
        let parallel_flag = if step.parallel { ", parallel=true" } else { "" };
        println!(
            "    Group {}: agent={}, can_repivot={}{}, prompt=\"{}\"",
            step.group, step.agent, repivot, parallel_flag, step.prompt
        );
    }
    if let Some(agents) = &wf.agents {
        println!("  Inline agents: {}", agents.len());
    }

    Ok(())
}
