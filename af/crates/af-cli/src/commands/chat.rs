use crate::app::ChatCommand;
use crate::CliConfig;
use af_agents::{AgentRuntime, OrchestratorRuntime};
use af_core::{AgentEvent, OrchestratorEvent};
use af_jobs::invoker::JobQueueInvoker;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

const HISTORY_FILE: &str = ".af_history";

pub async fn handle(config: &CliConfig, cmd: ChatCommand) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;
    let project_id: Uuid = cmd.project.parse()?;

    af_db::projects::get_project(&pool, project_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;

    let agent_config = if let Some(ref path) = cmd.agent_file {
        crate::commands::agent_file::load_agent_from_file(path)?
    } else {
        let agent_name = cmd.agent.as_deref().unwrap_or("default");
        af_agents::resolve_agent_config(&pool, agent_name, &config.agent_configs)
            .await
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", agent_name))?
    };

    let thread_id = if let Some(tid) = cmd.conversation {
        tid.parse()?
    } else {
        let thread = af_db::threads::create_thread(&pool, project_id, &agent_config.name, None).await?;
        println!("Created conversation: {}", thread.id);
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

    // Workflow mode: run the workflow once and return
    if let Some(ref workflow_name) = cmd.workflow {
        return run_workflow_mode(
            &pool,
            thread_id,
            workflow_name,
            config,
            router,
            invoker,
        )
        .await;
    }

    let mut runtime = AgentRuntime::new(
        pool.clone(),
        router,
        config.specs.clone(),
        invoker,
    );
    runtime.set_evidence_resolvers(config.evidence_resolvers.clone());
    if let Some(ref hook) = config.post_tool_hook {
        runtime.set_post_tool_hook(hook.clone());
    }
    runtime.set_compaction_threshold(config.compaction.threshold);
    if let Some(ref route_str) = config.compaction.summarization_route {
        if let Some(ref r) = config.router {
            let route = af_core::LlmRoute::from_str(route_str);
            if let Ok(backend) = r.resolve(&route) {
                runtime.set_summarization_backend(backend);
            }
        }
    }
    let runtime = Arc::new(runtime);

    println!("Chat started (agent: {}, project: {})", agent_config.name, project_id);
    println!("Type /help for commands, /quit to exit.\n");

    // Set up rustyline
    let history_path = dirs_history_path();
    let mut rl = DefaultEditor::new()?;
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    loop {
        match rl.readline("> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(input);

                // Handle slash commands
                if input.starts_with('/') {
                    if handle_slash_command(input, &config, &agent_config, thread_id, &pool).await {
                        break;
                    }
                    continue;
                }

                // Legacy quit
                if input == "quit" || input == "exit" {
                    println!("Goodbye.");
                    break;
                }

                // Streaming chat
                let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
                let rt = runtime.clone();
                let ac = agent_config.clone();
                let input_owned = input.to_string();

                let handle = tokio::spawn(async move {
                    if let Err(e) = rt
                        .send_message_streaming(thread_id, &ac, &input_owned, tx)
                        .await
                    {
                        eprintln!("Error: {e}");
                    }
                });

                // Render events as they arrive
                let mut got_done = false;
                while let Some(event) = rx.recv().await {
                    match &event {
                        AgentEvent::Done { .. } => {
                            got_done = true;
                        }
                        _ => {}
                    }
                    render_event(&event);
                }

                let _ = handle.await;

                if !got_done {
                    // Ensure newline after streaming
                    println!();
                }
                println!();
            }
            Err(ReadlineError::Interrupted) => {
                println!("Ctrl-C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye.");
                break;
            }
            Err(e) => {
                eprintln!("Readline error: {e}");
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }

    Ok(())
}

async fn handle_slash_command(
    input: &str,
    config: &CliConfig,
    agent_config: &af_core::AgentConfig,
    thread_id: Uuid,
    pool: &sqlx::PgPool,
) -> bool {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];

    match cmd {
        "/quit" | "/exit" => {
            println!("Goodbye.");
            return true;
        }
        "/tools" => {
            let specs = crate::commands::tool::list_allowed_tools(config, &agent_config.allowed_tools);
            if specs.is_empty() {
                println!("No tools available.");
            } else {
                for (name, desc) in specs {
                    println!("  {name}  — {desc}");
                }
            }
        }
        "/conversation" => {
            println!("Conversation: {thread_id}");
        }
        "/history" => {
            let n = parts
                .get(1)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(10);
            match af_db::messages::get_thread_messages(pool, thread_id).await {
                Ok(msgs) => {
                    let start = if msgs.len() > n { msgs.len() - n } else { 0 };
                    for msg in &msgs[start..] {
                        let content = msg.content.as_deref().unwrap_or("(no content)");
                        let truncated = if content.len() > 120 {
                            let end = content.floor_char_boundary(120);
                            format!("{}...", &content[..end])
                        } else {
                            content.to_string()
                        };
                        println!(
                            "  [{}] {} | {}",
                            msg.created_at.format("%H:%M:%S"),
                            msg.role,
                            truncated
                        );
                    }
                }
                Err(e) => eprintln!("Error loading history: {e}"),
            }
        }
        "/help" => {
            println!("Slash commands:");
            println!("  /tools        — list available tools for this agent");
            println!("  /conversation — show current conversation ID");
            println!("  /history [n]  — show last N messages (default 10)");
            println!("  /quit         — exit chat");
            println!("  /help         — show this help");
        }
        _ => {
            println!("Unknown command: {cmd}. Type /help for available commands.");
        }
    }
    false
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
            println!("\n[Calling {tool_name}...]");
            if let Ok(pretty) = serde_json::to_string_pretty(tool_input) {
                println!("  Input: {pretty}");
            }
        }
        AgentEvent::ToolCallResult {
            tool_name,
            success,
            summary,
        } => {
            let status = if *success { "OK" } else { "FAILED" };
            println!("[{tool_name}: {status}] {summary}");
        }
        AgentEvent::Evidence { ref_type, ref_id } => {
            println!("  [evidence:{ref_type}:{ref_id}]");
        }
        AgentEvent::Done { content, .. } => {
            // In streaming mode, content was already printed token-by-token.
            // Only print if we didn't stream any tokens.
            if !content.is_empty() {
                // The tokens may have been partially printed via Token events.
                // Print a newline to finish the streamed output.
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
                "  [usage] {route}: {} in + {} out{cached_str}{pct}",
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
            eprintln!("Agent error: {msg}");
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

async fn run_workflow_mode(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    workflow_name: &str,
    config: &CliConfig,
    router: Arc<af_llm::LlmRouter>,
    invoker: Arc<dyn af_core::ToolInvoker>,
) -> anyhow::Result<()> {
    let workflow = af_db::workflows::get(pool, workflow_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workflow '{}' not found", workflow_name))?;

    let steps: Vec<af_db::workflows::WorkflowStep> =
        serde_json::from_value(workflow.steps.clone())?;

    println!("Running workflow '{}' on conversation {}", workflow_name, thread_id);
    println!(
        "Description: {}",
        workflow.description.as_deref().unwrap_or("(none)")
    );
    println!();

    // Read initial message from stdin
    println!("Enter your message (press Enter to send):");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("empty message");
    }

    let mut orchestrator = OrchestratorRuntime::new(
        pool.clone(),
        router,
        config.specs.clone(),
        invoker,
    );
    orchestrator.set_evidence_resolvers(config.evidence_resolvers.clone());
    if let Some(ref hook) = config.post_tool_hook {
        orchestrator.set_post_tool_hook(hook.clone());
    }

    let (tx, mut rx) = mpsc::channel::<OrchestratorEvent>(512);

    let workflow_name_owned = workflow_name.to_string();
    let input_owned = input.to_string();
    let agent_configs = config.agent_configs.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) = orchestrator
            .execute_workflow(
                thread_id,
                &workflow_name_owned,
                &steps,
                &input_owned,
                &agent_configs,
                tx,
            )
            .await
        {
            eprintln!("Workflow error: {e}");
        }
    });

    while let Some(event) = rx.recv().await {
        render_orchestrator_event(&event);
    }

    let _ = handle.await;
    println!("\nWorkflow complete.");
    Ok(())
}

fn render_orchestrator_event(event: &OrchestratorEvent) {
    match event {
        OrchestratorEvent::AgentEvent { agent_name, event } => {
            match event {
                AgentEvent::Token(t) => {
                    print!("{t}");
                    let _ = io::stdout().flush();
                }
                AgentEvent::Reasoning(t) => {
                    print!("\x1b[90m{t}\x1b[0m");
                    let _ = io::stdout().flush();
                }
                AgentEvent::ToolCallStart { tool_name, .. } => {
                    println!("\n[{agent_name}] Calling {tool_name}...");
                }
                AgentEvent::ToolCallResult {
                    tool_name,
                    success,
                    summary,
                } => {
                    let status = if *success { "OK" } else { "FAILED" };
                    println!("[{agent_name}] [{tool_name}: {status}] {summary}");
                }
                AgentEvent::Evidence { ref_type, ref_id } => {
                    println!("[{agent_name}] evidence:{ref_type}:{ref_id}");
                }
                AgentEvent::Done { .. } => {
                    println!();
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
                        "  [{agent_name}] [usage] {route}: {} in + {} out{cached_str}{pct}",
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
                        "  [{agent_name}] \x1b[2m[compacted] {} tokens \u{2192} summarized {} messages (context: {})\x1b[0m",
                        format_tokens(*estimated_tokens),
                        messages_compacted,
                        format_tokens(*context_window),
                    );
                }
                AgentEvent::Error(msg) => {
                    eprintln!("[{agent_name}] Error: {msg}");
                }
            }
        }
        OrchestratorEvent::GroupComplete { group, agents } => {
            println!("\n--- Group {} complete ({}) ---\n", group, agents.join(", "));
        }
        OrchestratorEvent::WorkflowComplete { workflow_name } => {
            println!("--- Workflow '{}' complete ---", workflow_name);
        }
        OrchestratorEvent::SignalApplied {
            kind,
            target_agent,
            reason,
            source_agent,
        } => {
            println!("  [signal] {source_agent} -> {kind}:{target_agent} ({reason})");
        }
        OrchestratorEvent::RepivotApplied {
            original_artifact_id,
            new_artifact_id,
            new_filename,
            requeued_agents,
        } => {
            println!(
                "\n  [REPIVOT] artifact:{original_artifact_id} -> artifact:{new_artifact_id} ({new_filename})"
            );
            if !requeued_agents.is_empty() {
                println!("    Re-queuing: {}", requeued_agents.join(", "));
            }
        }
        OrchestratorEvent::FanOutStarted {
            parent_artifact_id,
            child_count,
            child_thread_ids,
        } => {
            println!(
                "\n  [FAN-OUT] artifact:{parent_artifact_id} -> {child_count} child conversations"
            );
            for tid in child_thread_ids {
                println!("    conversation:{tid}");
            }
        }
        OrchestratorEvent::FanOutComplete {
            parent_thread_id,
            child_thread_ids: _,
            completed,
            failed,
        } => {
            let total = completed + failed;
            println!(
                "  [FAN-OUT COMPLETE] parent:{parent_thread_id} — {completed} succeeded, {failed} failed ({total} total)"
            );
        }
        OrchestratorEvent::Error(msg) => {
            eprintln!("Orchestrator error: {msg}");
        }
    }
}

fn dirs_history_path() -> Option<String> {
    if let Ok(home) = std::env::var("HOME") {
        let dir = format!("{home}/.af");
        let _ = std::fs::create_dir_all(&dir);
        Some(format!("{dir}/{HISTORY_FILE}"))
    } else {
        None
    }
}
