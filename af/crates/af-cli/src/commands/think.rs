use crate::app::ThinkCommand;
use crate::CliConfig;
use af_agents::AgentRuntime;
use af_core::AgentEvent;
use af_jobs::invoker::JobQueueInvoker;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

pub async fn handle(config: &CliConfig, cmd: ThinkCommand) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;
    let project_id: Uuid = cmd.project.parse()?;

    af_db::projects::get_project(&pool, project_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;

    let agent_config = if let Some(ref path) = cmd.agent_file {
        crate::commands::agent_file::load_agent_from_file(path)?
    } else {
        let agent_name = cmd.agent.as_deref().unwrap_or("thinker");
        af_agents::resolve_agent_config(&pool, agent_name, &config.agent_configs)
            .await
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", agent_name))?
    };

    let thread_id = if let Some(tid) = cmd.conversation {
        tid.parse()?
    } else {
        let thread = af_db::threads::create_thread_typed(
            &pool,
            project_id,
            &agent_config.name,
            Some(&format!("thinking: {}", truncate_goal(&cmd.goal, 60))),
            "thinking",
        )
        .await?;
        println!("Created thinking thread: {}", thread.id);
        thread.id
    };

    let router = config
        .router
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no LLM backends configured"))?
        .clone();

    let invoker: Arc<dyn af_core::ToolInvoker> = Arc::new(JobQueueInvoker::new(
        pool.clone(),
        config.core_config.clone(),
        config.specs.clone(),
        config.executors.clone(),
    ));

    let mut runtime = AgentRuntime::new(pool.clone(), router.clone(), config.specs.clone(), invoker);
    runtime.set_evidence_resolvers(config.evidence_resolvers.clone());
    if let Some(ref hook) = config.post_tool_hook {
        runtime.set_post_tool_hook(hook.clone());
    }
    runtime.set_compaction_threshold(config.compaction.threshold);
    if let Some(ref route_str) = config.compaction.summarization_route {
        let route = af_core::LlmRoute::from_str(route_str);
        if let Ok(backend) = router.resolve(&route) {
            runtime.set_summarization_backend(backend);
        }
    }
    let runtime = Arc::new(runtime);

    println!(
        "Thinking started (agent: {}, project: {})",
        agent_config.name, project_id
    );
    println!("Goal: {}\n", cmd.goal);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let rt = runtime.clone();
    let ac = agent_config.clone();
    let goal = cmd.goal.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) = rt
            .send_message_streaming(thread_id, &ac, &goal, tx)
            .await
        {
            eprintln!("Error: {e}");
        }
    });

    let mut got_done = false;
    while let Some(event) = rx.recv().await {
        if matches!(&event, AgentEvent::Done { .. }) {
            got_done = true;
        }
        render_event(&event);
    }

    let _ = handle.await;

    if !got_done {
        println!();
    }
    println!("\nThinking complete. Thread: {thread_id}");

    // Show child threads if any were created
    let children = af_db::threads::list_child_threads(&pool, thread_id).await?;
    if !children.is_empty() {
        println!("\nChild threads:");
        for child in &children {
            let title = child.title.as_deref().unwrap_or("(untitled)");
            println!("  {} [{}] {}", child.id, child.agent_name, title);
        }
    }

    Ok(())
}

fn render_event(event: &AgentEvent) {
    match event {
        AgentEvent::Token(t) => {
            print!("{t}");
            let _ = io::stdout().flush();
        }
        AgentEvent::Reasoning(t) => {
            print!("\x1b[90m{t}\x1b[0m");
            let _ = io::stdout().flush();
        }
        AgentEvent::ToolCallStart {
            tool_name,
            tool_input,
        } => {
            println!("\n\x1b[36m[{tool_name}]\x1b[0m");
            if let Ok(pretty) = serde_json::to_string_pretty(tool_input) {
                // Show first 300 chars of input
                let display = if pretty.len() > 300 {
                    format!("{}...", &pretty[..300])
                } else {
                    pretty
                };
                println!("  \x1b[90m{display}\x1b[0m");
            }
        }
        AgentEvent::ToolCallResult {
            tool_name,
            success,
            summary,
        } => {
            let status = if *success {
                "\x1b[32mOK\x1b[0m"
            } else {
                "\x1b[31mFAILED\x1b[0m"
            };
            // Truncate summary for display
            let display = if summary.len() > 500 {
                format!("{}...", &summary[..500])
            } else {
                summary.clone()
            };
            println!("[{tool_name}: {status}] {display}");
        }
        AgentEvent::Evidence { ref_type, ref_id } => {
            println!("  \x1b[90m[evidence:{ref_type}:{ref_id}]\x1b[0m");
        }
        AgentEvent::Done { content, .. } => {
            if !content.is_empty() {
                println!();
            }
        }
        AgentEvent::Usage {
            prompt_tokens,
            completion_tokens,
            cached_read_tokens,
            context_window,
            route,
            ..
        } => {
            let cached_str = if *cached_read_tokens > 0 {
                format!(" ({cached_read_tokens} cached)")
            } else {
                String::new()
            };
            let pct = if *context_window > 0 {
                format!(" | {:.1}% of {} ctx", *prompt_tokens as f64 / *context_window as f64 * 100.0, format_tokens(*context_window))
            } else {
                String::new()
            };
            println!(
                "  \x1b[90m[usage] {route}: {} in + {} out{cached_str}{pct}\x1b[0m",
                format_tokens(*prompt_tokens),
                format_tokens(*completion_tokens),
            );
        }
        AgentEvent::ContextCompacted {
            estimated_tokens,
            messages_compacted,
            context_window,
        } => {
            println!(
                "  \x1b[2m[compacted] {} tokens \u{2192} summarized {} messages (context: {})\x1b[0m",
                format_tokens(*estimated_tokens),
                messages_compacted,
                format_tokens(*context_window),
            );
        }
        AgentEvent::Error(msg) => {
            eprintln!("\x1b[31mAgent error: {msg}\x1b[0m");
        }
    }
}

fn format_tokens(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

fn truncate_goal(goal: &str, max: usize) -> String {
    if goal.len() <= max {
        goal.to_string()
    } else {
        let end = goal.floor_char_boundary(max);
        format!("{}...", &goal[..end])
    }
}
