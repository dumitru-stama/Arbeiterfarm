use crate::CliConfig;
use af_api::SourceMap;
use af_core::{
    AgentConfig, CoreConfig, EvidenceResolverRegistry, LlmRoute, Plugin, ToolExecutor,
    ToolExecutorRegistry, ToolRendererRegistry, ToolSpec, ToolSpecRegistry, WorkflowDef,
};
use af_llm::{EmbeddingBackend, LlmRouter, OllamaEmbeddingBackend};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Extra tools to register alongside plugins (e.g. echo.tool for testing).
pub struct ExtraTool {
    pub spec: ToolSpec,
    pub executor: Box<dyn ToolExecutor>,
}

/// Bootstrap everything from plugins + environment. Returns a ready-to-run CliConfig.
///
/// Handles: tracing, registries, builtin file tools, DB pool, plugin lifecycle,
/// local TOML tools/agents/workflows, LLM router, default agent, DB seeding, core config.
///
/// `extra_tools` allows the binary to register non-plugin tools (e.g. echo.tool for testing).
///
/// `toml_plugin_filter` controls which TOML plugins from `~/.af/plugins/` are loaded:
/// - `None` → load all TOML plugins (default for distribution binaries)
/// - `Some(&[])` → skip all TOML plugins
/// - `Some(&["personal-assistant"])` → load only the named plugin(s)
pub async fn bootstrap(
    plugins: &[&dyn Plugin],
    extra_tools: Vec<ExtraTool>,
    toml_plugin_filter: Option<&[String]>,
) -> anyhow::Result<CliConfig> {
    // Load config from ~/.af/config.toml (or AF_CONFIG_PATH)
    let config = crate::config::load_or_create_default();

    // Tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Registries
    let mut specs = ToolSpecRegistry::new();
    let mut executors = ToolExecutorRegistry::new();
    let mut renderers = ToolRendererRegistry::new();
    crate::render::register_builtin(&mut renderers);
    let mut evidence_resolvers = EvidenceResolverRegistry::new();

    // Builtin file tools (domain-agnostic)
    af_builtin_tools::declare(&mut specs);
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    if let Some(ref path) = find_executor("af-builtin-executor", exe_dir) {
        verify_executor(path);
        af_builtin_tools::wire(&mut executors, path);
    } else {
        eprintln!(
            "Warning: af-builtin-executor not found; file tools will not work. \
             Set AF_EXECUTOR_PATH or install to PATH."
        );
    }

    // Tag builtin tools
    let mut source_map = SourceMap::new();
    for name in specs.list() {
        source_map.tools.insert(name.to_string(), "builtin".into());
    }

    // Extra tools from the binary (e.g. echo.tool)
    for extra in extra_tools {
        source_map.tools.insert(extra.spec.name.clone(), "builtin".into());
        specs.register(extra.spec)?;
        executors.register(extra.executor)?;
    }

    // tools.discover — spec only (intercepted in AgentRuntime, no executor needed)
    specs.register(tools_discover_spec())?;
    source_map.tools.insert("tools.discover".to_string(), "builtin".into());

    // Auto-derive DB URL from plugin filter names (single-plugin isolation)
    if std::env::var("AF_DATABASE_URL").is_err() {
        if let Some(names) = toml_plugin_filter {
            if names.len() == 1 {
                let db_name = format!("af_{}", names[0].replace('-', "_"));
                let derived_url = format!("postgres://af:af@localhost/{db_name}");
                eprintln!("[af] Auto-derived DB URL for plugin '{}': {derived_url}", names[0]);
                std::env::set_var("AF_DATABASE_URL", &derived_url);
            }
        }
    }

    // DB pool (lazy) — env var overrides config.toml
    let pool = {
        let url = std::env::var("AF_DATABASE_URL")
            .unwrap_or_else(|_| config.database.url.clone());
        match af_db::init_db(&url).await {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("[af] WARNING: DB unavailable ({e}), running without DB");
                None
            }
        }
    };

    // Load TOML plugins from ~/.af/plugins/ (or AF_PLUGINS_DIR)
    let mut toml_plugins =
        crate::toml_plugin::load_toml_plugins(&crate::toml_plugin::default_plugins_dir());

    // Filter TOML plugins by name if requested
    if let Some(names) = toml_plugin_filter {
        toml_plugins.retain(|p| names.iter().any(|n| n == p.plugin_name()));
        let loaded: Vec<&str> = toml_plugins.iter().map(|p| p.plugin_name()).collect();
        for name in names {
            if !loaded.iter().any(|n| *n == name.as_str()) {
                eprintln!("[af] WARNING: TOML plugin '{name}' not found in plugins dir");
            }
        }
    }

    // Combine compiled plugins + TOML plugins
    let mut all_plugins: Vec<&dyn Plugin> = plugins.to_vec();
    for tp in &toml_plugins {
        all_plugins.push(tp);
    }

    // Run all plugins through lifecycle
    let plugin_output = crate::plugin_runner::run_plugins(
        &all_plugins,
        pool.as_ref(),
        &mut specs,
        &mut executors,
        &mut evidence_resolvers,
        &mut renderers,
    )
    .await?;

    // Merge plugin source_map into ours
    source_map.tools.extend(plugin_output.source_map.tools);
    source_map.agents.extend(plugin_output.source_map.agents);
    source_map.workflows.extend(plugin_output.source_map.workflows);

    // Local TOML tools/agents/workflows
    let local_tool_names =
        crate::local_tools::register_local_tools(&mut specs, &mut executors, None);
    for name in &local_tool_names {
        source_map.tools.insert(name.clone(), "local".into());
    }

    // Local model cards (must be loaded before LLM router so capabilities() sees them)
    crate::local_models::register_local_models(None);

    // LLM router (build early so meta-tools can reference it)
    let router = build_llm_router();

    // Core config (build early so meta-tools can reference it) — env vars override config.toml
    let storage_root = std::env::var("AF_STORAGE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&config.storage.storage_root));
    let scratch_root = std::env::var("AF_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&config.storage.scratch_root));
    tokio::fs::create_dir_all(&storage_root).await?;
    tokio::fs::create_dir_all(&scratch_root).await?;
    let core_config = CoreConfig {
        storage_root,
        scratch_root,
        use_oaie: false, // overridden by --oaie CLI flag in main.rs
    };

    // Embedding backend (Ollama) — declare embed specs early so build_default_agent sees them.
    // Only declare when both backend and DB are available (validate_registries needs matching executors).
    let embedding_backend = build_embedding_backend();
    if embedding_backend.is_some() && pool.is_some() {
        af_builtin_tools::declare_embed(&mut specs);
        for name in ["embed.text", "embed.search", "embed.batch", "embed.list"] {
            source_map.tools.insert(name.to_string(), "builtin".into());
        }
    }

    // Web gateway tools (conditional on gateway socket config)
    let web_gateway_socket = std::env::var("AF_WEB_GATEWAY_SOCKET")
        .ok()
        .map(PathBuf::from);
    if let Some(ref socket) = web_gateway_socket {
        af_web_gateway::declare(&mut specs);
        af_web_gateway::wire(&mut executors, socket);
        for name in ["web.fetch", "web.search"] {
            source_map.tools.insert(name.to_string(), "builtin".into());
        }
        eprintln!("[af] Web gateway tools registered (socket: {})", socket.display());
    }

    // Email tools (need DB for credentials and scheduling)
    if let Some(ref p) = pool {
        af_email::declare(&mut specs);
        af_email::wire(&mut executors, p.clone());
        for name in ["email.send", "email.draft", "email.schedule",
                     "email.list_inbox", "email.read", "email.reply", "email.search"] {
            source_map.tools.insert(name.to_string(), "builtin".into());
        }
        eprintln!("[af] Email tools registered");
    }

    // Notification tools (need DB for queue and channels)
    if let Some(ref p) = pool {
        af_notify::declare(&mut specs);
        af_notify::wire(&mut executors, p.clone());
        for name in ["notify.send", "notify.upload", "notify.list", "notify.test"] {
            source_map.tools.insert(name.to_string(), "builtin".into());
        }
        eprintln!("[af] Notification tools registered");
    }

    // Dynamic default agent + thinker agent + plugin agents
    let mut agent_configs = vec![build_default_agent(&specs)];
    source_map.agents.insert("default".into(), "builtin".into());
    agent_configs.push(af_agents::meta_tools::build_thinker_agent());
    source_map.agents.insert("thinker".into(), "builtin".into());

    // Researcher agent (only when web gateway is available)
    if web_gateway_socket.is_some() {
        agent_configs.push(build_researcher_agent());
        source_map.agents.insert("researcher".into(), "builtin".into());
    }

    // Email composer agent (always available; tool restrictions enforce access)
    if pool.is_some() {
        agent_configs.push(af_email::email_composer_agent());
        source_map.agents.insert("email-composer".into(), "builtin".into());
    }

    // Notifier agent (always available; tool restrictions enforce access)
    if pool.is_some() {
        agent_configs.push(af_notify::notifier_agent());
        source_map.agents.insert("notifier".into(), "builtin".into());
    }

    agent_configs.extend(plugin_output.agent_configs);

    // Meta-tools: declare specs + wire executors (before validation and Arc wrapping)
    af_agents::meta_tools::declare_meta_tools(&mut specs);
    for name in ["internal.invoke_agent", "internal.list_agents", "internal.read_thread", "internal.list_artifacts", "internal.read_artifact"] {
        source_map.tools.insert(name.to_string(), "builtin".into());
    }

    // Arc-wrap evidence resolvers early so meta-tools can share them
    let evidence_resolvers = Arc::new(evidence_resolvers);

    let lazy_refs = Arc::new(af_agents::meta_tools::LazyMetaRefs::new());
    if let (Some(ref p), Some(ref r)) = (&pool, &router) {
        af_agents::meta_tools::wire_meta_tools(
            &mut executors,
            p.clone(),
            r.clone(),
            core_config.clone(),
            agent_configs.clone(),
            lazy_refs.clone(),
            Some(evidence_resolvers.clone()),
            plugin_output.post_tool_hook.clone(),
        );
        eprintln!("[af] Meta-tools wired (thinking threads enabled)");
    } else {
        eprintln!("[af] Meta-tools declared but not wired (needs DB + LLM router)");
    }

    // Wire embed tool executors (declaration happened earlier, before build_default_agent)
    if let (Some(ref eb), Some(ref p)) = (&embedding_backend, &pool) {
        af_builtin_tools::wire_embed(&mut executors, eb.clone(), p.clone());
        eprintln!("[af] Embedding tools wired (model: {})", eb.name());
    } else if embedding_backend.is_some() {
        eprintln!("[af] Embedding backend available but DB unavailable; embed tools not wired");
    }

    af_core::validate_registries(&specs, &executors)
        .map_err(|errs| anyhow::anyhow!("registry validation failed: {:?}", errs))?;

    // Seed to DB
    if let Some(ref p) = pool {
        seed_to_db(p, &agent_configs, &plugin_output.workflows, &source_map).await;
    }

    let local_agent_names = if let Some(ref p) = pool {
        crate::local_agents::register_local_agents(p, &mut agent_configs, None).await
    } else {
        eprintln!("[af] WARNING: DB unavailable, local agents not registered");
        vec![]
    };
    for name in &local_agent_names {
        source_map.agents.insert(name.clone(), "local".into());
    }
    let local_workflow_names = if let Some(ref p) = pool {
        crate::local_workflows::register_local_workflows(p, &mut agent_configs, None).await
    } else {
        eprintln!("[af] WARNING: DB unavailable, local workflows not registered");
        vec![]
    };
    for name in &local_workflow_names {
        source_map.workflows.insert(name.clone(), "local".into());
    }

    // Plugin metadata
    let ghidra_cache_dir = plugin_output
        .metadata
        .get("ghidra_cache_dir")
        .and_then(|v| v.as_str())
        .map(PathBuf::from);

    // Arc-wrap registries and finalize meta-tool lazy refs
    let specs = Arc::new(specs);
    let executors = Arc::new(executors);
    af_agents::meta_tools::finalize_meta_refs(&lazy_refs, specs.clone(), executors.clone());

    Ok(CliConfig {
        specs,
        executors,
        renderers,
        evidence_resolvers,
        post_tool_hook: plugin_output.post_tool_hook,
        tool_config_hooks: plugin_output.tool_config_hooks,
        core_config,
        agent_configs,
        router,
        pool,
        ghidra_cache_dir,
        source_map,
        compaction: config.compaction,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn build_default_agent(specs: &ToolSpecRegistry) -> AgentConfig {
    let mut tool_namespaces: BTreeSet<String> = specs
        .list()
        .iter()
        .filter_map(|name| name.split('.').next().map(|ns| format!("{ns}.*")))
        .collect();
    tool_namespaces.insert("echo.tool".to_string());

    AgentConfig {
        name: "default".to_string(),
        system_prompt: concat!(
            "You are a reverse engineering specialist. You can use tools to answer the user's questions. ",
            "Only call tools when the user's request requires it. Be concise and direct.",
        ).to_string(),
        allowed_tools: tool_namespaces.into_iter().collect(),
        default_route: LlmRoute::Auto,
        metadata: serde_json::Value::Object(Default::default()),
        tool_call_budget: None,
        timeout_secs: None,
    }
}

fn build_researcher_agent() -> AgentConfig {
    AgentConfig {
        name: "researcher".to_string(),
        system_prompt: concat!(
            "You are an internet researcher. You gather resources, documents, and data from the web ",
            "by request. You work methodically: search for relevant sources, fetch pages, extract ",
            "key information, and summarize findings.\n\n",
            "Workflow:\n",
            "1. Break down the research request into specific queries\n",
            "2. Use web.search to find relevant URLs\n",
            "3. Use web.fetch to retrieve content from promising URLs\n",
            "4. Extract and synthesize the relevant information\n",
            "5. Cite all sources with URLs\n\n",
            "Important:\n",
            "- Never attempt to bypass URL restrictions or access blocked resources\n",
            "- If a URL is blocked, report it and try alternative sources\n",
            "- Always verify information from multiple sources when possible\n",
            "- Summarize clearly with source citations\n",
        ).to_string(),
        allowed_tools: vec![
            "web.fetch".into(),
            "web.search".into(),
        ],
        default_route: LlmRoute::Auto,
        metadata: serde_json::Value::Null,
        tool_call_budget: Some(30),
        timeout_secs: Some(600),
    }
}

fn build_llm_router() -> Option<Arc<LlmRouter>> {
    let mut router = LlmRouter::new();

    // Local LLM backend: AF_LOCAL_ENDPOINT (Ollama, llama.cpp, vLLM, etc.)
    // Separate from cloud OpenAI — no name sanitization, no redaction, generous timeouts.
    if let Ok(endpoint) = std::env::var("AF_LOCAL_ENDPOINT") {
        let api_key = std::env::var("AF_LOCAL_API_KEY").ok();
        let model = std::env::var("AF_LOCAL_MODEL").unwrap_or_else(|_| "gpt-oss".to_string());
        let name = format!("local:{model}");
        router.register(Box::new(af_llm::LocalLlmBackend::new(
            name.clone(),
            endpoint.clone(),
            api_key.clone(),
            model.clone(),
        )));
        router.register_alias("local", &name);
        if let Ok(extra) = std::env::var("AF_LOCAL_MODELS") {
            for m in extra.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                router.register(Box::new(af_llm::LocalLlmBackend::new(
                    format!("local:{m}"),
                    endpoint.clone(),
                    api_key.clone(),
                    m.to_string(),
                )));
            }
        }
    }

    // OpenAI cloud backend: AF_OPENAI_ENDPOINT (custom) or AF_OPENAI_API_KEY (official API)
    let openai_endpoint = std::env::var("AF_OPENAI_ENDPOINT").ok().or_else(|| {
        std::env::var("AF_OPENAI_API_KEY").ok().map(|_| "https://api.openai.com".to_string())
    });
    if let Some(endpoint) = openai_endpoint {
        let api_key = std::env::var("AF_OPENAI_API_KEY").ok();
        let model = std::env::var("AF_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        let name = format!("openai:{model}");
        router.register(Box::new(af_llm::OpenAiBackend::new(
            name.clone(),
            endpoint.clone(),
            api_key.clone(),
            model.clone(),
        )));
        router.register_alias("openai", &name);
        if let Ok(extra) = std::env::var("AF_OPENAI_MODELS") {
            for m in extra.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                router.register(Box::new(af_llm::OpenAiBackend::new(
                    format!("openai:{m}"),
                    endpoint.clone(),
                    api_key.clone(),
                    m.to_string(),
                )));
            }
        }
    }

    if let Ok(api_key) = std::env::var("AF_ANTHROPIC_API_KEY") {
        let model = std::env::var("AF_ANTHROPIC_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
        let name = format!("anthropic:{model}");
        router.register(Box::new(af_llm::AnthropicBackend::new(
            name.clone(),
            api_key.clone(),
            model.clone(),
        )));
        router.register_alias("anthropic", &name);
        if let Ok(extra) = std::env::var("AF_ANTHROPIC_MODELS") {
            for m in extra.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                router.register(Box::new(af_llm::AnthropicBackend::new(
                    format!("anthropic:{m}"),
                    api_key.clone(),
                    m.to_string(),
                )));
            }
        }
    }

    if let Ok(endpoint) = std::env::var("AF_VERTEX_ENDPOINT") {
        let token = std::env::var("AF_VERTEX_ACCESS_TOKEN").unwrap_or_default();
        if !token.is_empty() {
            let model = endpoint.rsplit('/').next().unwrap_or("vertex").to_string();
            let name = format!("vertex:{model}");
            router.register(Box::new(af_llm::VertexAiBackend::new(
                name.clone(),
                endpoint,
                token,
            )));
            router.register_alias("vertex", &name);
        } else {
            eprintln!("Warning: AF_VERTEX_ENDPOINT set but AF_VERTEX_ACCESS_TOKEN missing");
        }
    }

    // Explicit default route override
    if let Ok(route) = std::env::var("AF_DEFAULT_ROUTE") {
        router.set_default(&route);
        eprintln!("[af] Default route: {route}");
    }

    if router.has_backends() {
        Some(Arc::new(router))
    } else {
        None
    }
}

async fn seed_to_db(
    pool: &af_db::PgPool,
    agents: &[AgentConfig],
    workflows: &[WorkflowDef],
    source_map: &SourceMap,
) {
    for c in agents {
        let meta = if c.metadata.is_null() {
            serde_json::json!({})
        } else {
            c.metadata.clone()
        };
        let src = source_map.agents.get(&c.name).map(|s| s.as_str());
        if let Err(e) = af_db::agents::upsert(
            pool,
            &c.name,
            &c.system_prompt,
            &c.allowed_tools_json(),
            &c.default_route.to_db_string(),
            &meta,
            true,
            src,
            c.timeout_secs.map(|s| s as i32),
        )
        .await
        {
            eprintln!("[af] WARNING: failed to seed agent '{}': {e}", c.name);
        }
    }
    for wf in workflows {
        let steps = serde_json::to_value(&wf.steps).unwrap_or_default();
        let src = source_map.workflows.get(&wf.name).map(|s| s.as_str());
        if let Err(e) =
            af_db::workflows::upsert(pool, &wf.name, wf.description.as_deref(), &steps, true, src)
                .await
        {
            eprintln!("[af] WARNING: failed to seed workflow '{}': {e}", wf.name);
        }
    }
    // Seed web.* as restricted tool (idempotent upsert)
    if let Err(e) = af_db::restricted_tools::add_restricted(
        pool,
        "web.*",
        "Web fetch and search tools — requires explicit user grant",
    )
    .await
    {
        eprintln!("[af] WARNING: failed to seed restricted tool 'web.*': {e}");
    }

    // Seed email.* as restricted tool (idempotent upsert)
    if let Err(e) = af_db::restricted_tools::add_restricted(
        pool,
        "email.*",
        "Email tools — requires admin grant and configured credentials",
    )
    .await
    {
        eprintln!("[af] WARNING: failed to seed restricted tool 'email.*': {e}");
    }

    // Seed notify.* as restricted tool (idempotent upsert)
    if let Err(e) = af_db::restricted_tools::add_restricted(
        pool,
        "notify.*",
        "Notification tools — requires admin grant and channel configuration",
    )
    .await
    {
        eprintln!("[af] WARNING: failed to seed restricted tool 'notify.*': {e}");
    }

    eprintln!(
        "[af] Seeded {} agents and {} workflows",
        agents.len(),
        workflows.len()
    );
}

/// Verify the executor binary's SHA-256 hash against AF_EXECUTOR_SHA256 if set.
/// Logs the computed hash for pinning if the env var is not set.
fn verify_executor(path: &std::path::Path) {
    use sha2::{Digest, Sha256};

    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[af] WARNING: cannot read executor for verification: {e}");
            return;
        }
    };
    let computed = hex::encode(Sha256::digest(&data));

    if let Ok(expected) = std::env::var("AF_EXECUTOR_SHA256") {
        if computed != expected {
            eprintln!(
                "[af] FATAL: executor SHA-256 mismatch!\n  expected: {expected}\n  computed: {computed}\n  path: {}",
                path.display()
            );
            std::process::exit(1);
        }
        eprintln!("[af] Executor verified: SHA-256={computed}");
    } else {
        eprintln!(
            "[af] Executor SHA-256={computed} (set AF_EXECUTOR_SHA256 to enforce verification)"
        );
    }
}

pub fn build_embedding_backend() -> Option<Arc<dyn EmbeddingBackend>> {
    // AF_EMBEDDING_ENDPOINT defaults to AF_LOCAL_ENDPOINT (same Ollama server)
    let endpoint = std::env::var("AF_EMBEDDING_ENDPOINT")
        .or_else(|_| std::env::var("AF_LOCAL_ENDPOINT"))
        .ok()?;

    let model = std::env::var("AF_EMBEDDING_MODEL")
        .unwrap_or_else(|_| "snowflake-arctic-embed2".to_string());

    let dimensions: u32 = std::env::var("AF_EMBEDDING_DIMENSIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            if model.contains("nomic") {
                768
            } else {
                1024
            }
        });

    Some(Arc::new(OllamaEmbeddingBackend::new(
        endpoint, model, dimensions,
    )))
}

fn tools_discover_spec() -> ToolSpec {
    ToolSpec {
        name: "tools.discover".into(),
        version: 1,
        deprecated: false,
        description: "Get the full input schema for a tool. Call this before using a tool for the first time.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "tool_name": {
                    "type": "string",
                    "description": "Name of the tool to discover (e.g. 'ghidra.decompile')"
                }
            },
            "required": ["tool_name"]
        }),
        policy: af_core::ToolPolicy {
            sandbox: af_core::SandboxProfile::Trusted,
            timeout_ms: 1_000,
            ..af_core::ToolPolicy::default()
        },
        output_redirect: Default::default(),
    }
}

fn find_executor(name: &str, exe_dir: &std::path::Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("AF_EXECUTOR_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
        eprintln!(
            "[af] WARNING: AF_EXECUTOR_PATH={path} does not exist, trying fallbacks"
        );
    }
    let sibling = exe_dir.join(name);
    if sibling.exists() {
        return Some(sibling);
    }
    which::which(name).ok()
}
