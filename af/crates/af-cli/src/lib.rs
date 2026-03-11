pub mod app;
pub mod backend;
pub mod bootstrap;
pub mod commands;
pub mod config;
pub mod local_agents;
pub mod local_models;
pub mod local_tools;
pub mod local_workflows;
pub mod plugin_runner;
pub mod render;
pub mod toml_plugin;

use af_api::SourceMap;
use af_core::{AgentConfig, CoreConfig, EvidenceResolverRegistry, PostToolHook, ToolExecutorRegistry, ToolRendererRegistry, ToolSpecRegistry};
use af_llm::LlmRouter;
use std::sync::Arc;

/// Configuration passed by the distribution binary.
pub struct CliConfig {
    pub specs: Arc<ToolSpecRegistry>,
    pub executors: Arc<ToolExecutorRegistry>,
    pub renderers: ToolRendererRegistry,
    pub evidence_resolvers: Arc<EvidenceResolverRegistry>,
    pub post_tool_hook: Option<Arc<dyn PostToolHook>>,
    pub tool_config_hooks: Vec<Arc<dyn af_core::ToolConfigHook>>,
    pub core_config: CoreConfig,
    pub agent_configs: Vec<AgentConfig>,
    pub router: Option<Arc<LlmRouter>>,
    /// Optional pre-initialized DB pool (avoids creating a second pool).
    pub pool: Option<sqlx::PgPool>,
    /// Path to Ghidra analysis cache (for project downloads via API).
    pub ghidra_cache_dir: Option<std::path::PathBuf>,
    /// Tracks the origin plugin/source for every tool, agent, and workflow.
    pub source_map: SourceMap,
    /// Compaction configuration (threshold and summarization route).
    pub compaction: config::CompactionConfig,
}

/// Connect to DB — reuses the pre-initialized pool from CliConfig if available,
/// otherwise creates a new one.
pub(crate) async fn get_pool_from(pool: &Option<sqlx::PgPool>) -> anyhow::Result<sqlx::PgPool> {
    if let Some(p) = pool {
        return Ok(p.clone());
    }
    let database_url = std::env::var("AF_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://af:af@localhost/af".to_string());
    let pool = af_db::init_db(&database_url).await?;
    Ok(pool)
}

/// Parse the CLI arguments. Call this before `bootstrap()` to read `--plugin` flags.
pub fn parse_cli() -> app::Cli {
    app::parse()
}

/// Main entry point with pre-parsed CLI. Preferred when the binary needs to
/// inspect CLI args before bootstrap (e.g. to read `--plugin` flags).
pub async fn run_with_cli(config: CliConfig, cli: app::Cli) -> anyhow::Result<()> {
    let is_remote = cli.remote.is_some();

    // Construct backend: remote if --remote given, else direct DB
    let backend: Box<dyn backend::Backend> = if let Some(ref url) = cli.remote {
        let api_key = cli.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--api-key or AF_API_KEY required when using --remote")
        })?;
        Box::new(backend::remote::RemoteApi::new(url, api_key, cli.allow_insecure)?)
    } else {
        let pool = get_pool_from(&config.pool).await?;
        Box::new(backend::direct::DirectDb::new(
            pool,
            config.core_config.clone(),
        ))
    };

    match cli.command {
        // Commands that use Backend trait:
        app::Commands::Project(cmd) => {
            commands::project::handle(&*backend, cmd).await?
        }
        app::Commands::Artifact(cmd) => {
            commands::artifact::handle(&config, &*backend, cmd).await?
        }
        app::Commands::Conversation(cmd) => {
            commands::thread::handle(&*backend, cmd).await?
        }
        app::Commands::Audit(cmd) => {
            commands::audit::handle(&*backend, cmd).await?
        }
        app::Commands::User(cmd) => {
            commands::user::handle(&*backend, cmd).await?
        }
        app::Commands::Agent(cmd) => {
            commands::agent::handle(&*backend, cmd).await?
        }
        app::Commands::Workflow(cmd) => {
            commands::workflow::handle(&config, &*backend, cmd).await?
        }
        app::Commands::Hook(cmd) => {
            commands::hook::handle(&*backend, cmd).await?
        }
        app::Commands::WebRule(cmd) => {
            commands::web_rule::handle(&*backend, cmd).await?
        }
        app::Commands::Grant(cmd) => {
            commands::grant::handle(&*backend, cmd).await?
        }
        app::Commands::EmailRule(cmd) => {
            commands::email_rule::handle(cmd.action, &*backend).await?
        }
        app::Commands::Email(cmd) => {
            commands::email::handle(cmd.action, &*backend).await?
        }
        app::Commands::Yara(cmd) => {
            commands::yara::handle(&*backend, cmd).await?
        }

        // Local-only commands — reject --remote:
        app::Commands::Tool(cmd) => {
            reject_remote(is_remote)?;
            commands::tool::handle(&config, cmd).await?
        }
        app::Commands::Chat(cmd) => {
            reject_remote(is_remote)?;
            commands::chat::handle(&config, cmd).await?
        }
        app::Commands::Serve(cmd) => {
            reject_remote(is_remote)?;
            commands::serve::handle(&config, cmd).await?
        }
        app::Commands::Worker(cmd) => {
            reject_remote(is_remote)?;
            commands::worker::handle(&config, cmd).await?
        }
        app::Commands::Think(cmd) => {
            reject_remote(is_remote)?;
            commands::think::handle(&config, cmd).await?
        }
        app::Commands::EmbedQueue(cmd) => {
            reject_remote(is_remote)?;
            commands::embed_queue::handle(&config, cmd.action).await?
        }
        app::Commands::UrlIngest(cmd) => {
            reject_remote(is_remote)?;
            commands::url_ingest::handle(&config, cmd.action).await?
        }
        app::Commands::Notify(cmd) => {
            reject_remote(is_remote)?;
            commands::notify::handle(&config, cmd.action).await?
        }
        app::Commands::GhidraRenames(cmd) => {
            reject_remote(is_remote)?;
            commands::ghidra_renames::handle(&config, cmd.action).await?
        }
        app::Commands::Tick => {
            reject_remote(is_remote)?;
            commands::tick::handle(&config).await?
        }
    }

    Ok(())
}

fn reject_remote(is_remote: bool) -> anyhow::Result<()> {
    if is_remote {
        anyhow::bail!("this command requires local access and cannot be used with --remote");
    }
    Ok(())
}

/// Convenience entry point: parses CLI internally. Use `run_with_cli` when you
/// need to inspect CLI args (e.g. `--plugin`) before calling `bootstrap()`.
pub async fn run(config: CliConfig) -> anyhow::Result<()> {
    run_with_cli(config, parse_cli()).await
}
