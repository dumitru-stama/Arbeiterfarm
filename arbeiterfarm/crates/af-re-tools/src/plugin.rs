use async_trait::async_trait;
use af_plugin_api::{
    AgentConfig, ArtifactRef, EvidenceResolverRegistry, Migration, NoopPluginDb, Plugin, PluginDb,
    PostToolHook, ToolConfigHook, ToolExecutorRegistry, ToolRendererRegistry, ToolSpecRegistry,
    WorkflowDef, WorkflowStepDef,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The reverse-engineering plugin. Fully self-contained: owns all RE tools (rizin, Ghidra,
/// IOC, artifact, family) and VT tools. A distribution binary just creates this with config
/// and calls `run_plugins()`.
pub struct RePlugin {
    pub config: RePluginConfig,
}

pub struct RePluginConfig {
    // Rizin
    pub rizin_path: Option<PathBuf>,
    pub executor_path: Option<PathBuf>,
    pub allow_unsandboxed: bool,
    // Ghidra
    pub ghidra_home: Option<PathBuf>,
    pub ghidra_cache: Option<PathBuf>,
    pub ghidra_scripts: Option<PathBuf>,
    pub ghidra_java_home: Option<PathBuf>,
    // VT
    pub vt_socket: Option<PathBuf>,
    pub vt_api_key: Option<String>,
    pub vt_rate_limit_rpm: u32,
    pub vt_cache_ttl_secs: u64,
    pub vt_max_tracked_users: usize,
    // Sandbox
    pub sandbox_socket: Option<PathBuf>,
    // YARA
    pub yara_path: Option<PathBuf>,
    pub yara_rules_dir: Option<PathBuf>,
}

impl Default for RePluginConfig {
    fn default() -> Self {
        Self {
            rizin_path: None,
            executor_path: None,
            allow_unsandboxed: false,
            ghidra_home: None,
            ghidra_cache: None,
            ghidra_scripts: None,
            ghidra_java_home: None,
            vt_socket: None,
            vt_api_key: None,
            vt_rate_limit_rpm: 4,
            vt_cache_ttl_secs: 86400,
            vt_max_tracked_users: 10_000,
            sandbox_socket: None,
            yara_path: None,
            yara_rules_dir: None,
        }
    }
}

impl RePluginConfig {
    /// Build config from environment variables. `exe_dir` is used for executor/scripts discovery.
    pub fn from_env(exe_dir: &std::path::Path) -> Self {
        let allow_unsandboxed = std::env::var("AF_ALLOW_UNSANDBOXED").is_ok();
        let executor_path = find_executor("af-executor", exe_dir);

        if executor_path.is_some() {
            eprintln!("[af] RE executor found, tools will run in bwrap sandbox");
        } else if allow_unsandboxed {
            eprintln!("[af] WARNING: af-executor not found; running InProcess (AF_ALLOW_UNSANDBOXED set)");
        } else {
            eprintln!(
                "[af] RE tools DISABLED: af-executor not found and AF_ALLOW_UNSANDBOXED not set. \
                 Install af-executor or set AF_ALLOW_UNSANDBOXED=1."
            );
        }

        let ghidra = resolve_ghidra_paths(exe_dir);

        let vt_api_key = std::env::var("AF_VT_API_KEY").ok();
        let vt_socket = vt_api_key.as_ref().map(|_| {
            PathBuf::from(
                std::env::var("AF_VT_SOCKET")
                    .unwrap_or_else(|_| "/run/af/vt_gateway.sock".to_string()),
            )
        });

        Self {
            rizin_path: Some(PathBuf::from(
                std::env::var("AF_RIZIN_PATH").unwrap_or_else(|_| "/usr/bin/rizin".to_string()),
            )),
            executor_path,
            allow_unsandboxed,
            ghidra_home: ghidra.as_ref().map(|g| g.0.clone()),
            ghidra_cache: ghidra.as_ref().map(|g| g.1.clone()),
            ghidra_scripts: ghidra.as_ref().map(|g| g.2.clone()),
            ghidra_java_home: ghidra.as_ref().and_then(|g| resolve_ghidra_java_home(&g.0)),
            vt_socket,
            vt_api_key,
            vt_rate_limit_rpm: std::env::var("AF_VT_RATE_LIMIT")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(4),
            vt_cache_ttl_secs: std::env::var("AF_VT_CACHE_TTL")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(86400),
            vt_max_tracked_users: std::env::var("AF_VT_MAX_TRACKED_USERS")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(10_000),
            sandbox_socket: std::env::var("AF_SANDBOX_SOCKET").ok().map(PathBuf::from),
            yara_path: resolve_yara_path(),
            yara_rules_dir: resolve_yara_rules_dir(),
        }
    }
}

impl RePlugin {
    pub fn new(config: RePluginConfig) -> Self {
        Self { config }
    }

    /// Create from environment variables. Convenience for `RePlugin::new(RePluginConfig::from_env(exe_dir))`.
    pub fn from_env(exe_dir: &std::path::Path) -> Self {
        Self::new(RePluginConfig::from_env(exe_dir))
    }

    pub fn can_sandbox(&self) -> bool {
        self.config.executor_path.is_some() || self.config.allow_unsandboxed
    }

    fn has_rizin(&self) -> bool {
        self.config
            .rizin_path
            .as_ref()
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    fn has_ghidra(&self) -> bool {
        self.config.ghidra_home.as_ref().map_or(false, |home| {
            home.join("support").join("analyzeHeadless").exists()
        })
    }

    fn has_yara(&self) -> bool {
        self.config
            .yara_path
            .as_ref()
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    fn has_vt(&self) -> bool {
        self.config.vt_socket.is_some() && self.config.vt_api_key.is_some()
    }

    /// Start the sandbox gateway daemon. Call this after `run_plugins()`.
    /// Returns a join handle for the gateway task, or None if sandbox is not configured.
    pub async fn start_sandbox_gateway(&self) -> Option<tokio::task::JoinHandle<()>> {
        let socket_path = self.config.sandbox_socket.as_ref()?;
        let handle = af_re_sandbox::start_sandbox_gateway(socket_path).await?;
        Some(handle)
    }

    /// Start the VT gateway daemon. Call this after `run_plugins()`.
    /// Returns a join handle for the gateway task, or None if VT is not configured.
    pub async fn start_vt_gateway(
        &self,
        pool: Option<&sqlx::PgPool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let vt_socket = self.config.vt_socket.as_ref()?;
        let vt_api_key = self.config.vt_api_key.as_ref()?;

        if !self.can_sandbox() {
            return None;
        }

        let db: Arc<dyn PluginDb> = match pool {
            Some(p) => Arc::new(af_db::ScopedPluginDb::new(p.clone(), "re")),
            None => Arc::new(NoopPluginDb),
        };

        let gateway = af_re_vt::VtGateway::new(
            vt_socket.clone(),
            vt_api_key.clone(),
            db,
            self.config.vt_rate_limit_rpm,
            std::time::Duration::from_secs(self.config.vt_cache_ttl_secs),
        )
        .with_max_tracked_users(self.config.vt_max_tracked_users);
        let handle = gateway.start().await;

        eprintln!("[af] VT gateway started at {}", vt_socket.display());
        Some(handle)
    }
}

impl Plugin for RePlugin {
    fn name(&self) -> &str {
        "re"
    }

    fn schema(&self) -> &str {
        "re"
    }

    fn migrations(&self) -> Vec<Migration> {
        let mut migs = crate::migrations();
        if self.has_vt() {
            migs.extend(af_re_vt::migrations());
        }
        migs
    }

    fn declare(&self, specs: &mut ToolSpecRegistry) {
        // Rizin tools — only if rizin is available AND sandbox is available
        if self.has_rizin() && self.can_sandbox() {
            crate::declare(specs);
            eprintln!(
                "[af] RE tools registered (rizin: {})",
                self.config.rizin_path.as_ref().unwrap().display()
            );
        } else if self.has_rizin() {
            eprintln!(
                "[af] RE tools DISABLED: no sandbox available. \
                 Install af-executor or set AF_ALLOW_UNSANDBOXED=1"
            );
        } else {
            eprintln!(
                "[af] WARNING: rizin not found at {}, RE tools disabled",
                self.config
                    .rizin_path
                    .as_deref()
                    .unwrap_or_else(|| std::path::Path::new("(none)"))
                    .display()
            );
        }

        // IOC, artifact, family, dedup tools — always (DB-only, no rizin/ghidra dependency)
        crate::declare_ioc(specs);
        crate::declare_artifact(specs);
        crate::declare_family(specs);
        crate::declare_dedup(specs);
        eprintln!("[af] IOC, artifact, family, and dedup tools registered");

        // Ghidra tools
        if self.has_ghidra() && self.can_sandbox() {
            let ghidra_home = self.config.ghidra_home.as_ref().unwrap();
            let cache = self
                .config
                .ghidra_cache
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("/tmp/af/ghidra_cache"));
            let scripts = self
                .config
                .ghidra_scripts
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("ghidra-scripts"));
            crate::declare_ghidra(specs, ghidra_home, cache, scripts, self.config.ghidra_java_home.as_deref());
            eprintln!(
                "[af] Ghidra tools registered (home: {}, cache: {}, scripts: {}, jdk: {})",
                ghidra_home.display(),
                cache.display(),
                scripts.display(),
                self.config.ghidra_java_home.as_deref().map_or("(system)".into(), |p| p.display().to_string()),
            );
        } else if self.has_ghidra() {
            eprintln!(
                "[af] Ghidra tools DISABLED: no sandbox available. \
                 Install af-executor or set AF_ALLOW_UNSANDBOXED=1"
            );
        } else if self.config.ghidra_home.is_none() {
            eprintln!("[af] Ghidra tools disabled (set AF_GHIDRA_HOME to enable)");
        } else {
            eprintln!("[af] WARNING: analyzeHeadless not found, ghidra.* tools disabled");
        }

        // VT tools
        if let Some(ref vt_socket) = self.config.vt_socket {
            if self.can_sandbox() {
                af_re_vt::declare(specs, vt_socket);
                eprintln!("[af] VT tools registered");
            } else {
                eprintln!(
                    "[af] VT tools DISABLED: no sandbox available. \
                     Install af-executor or set AF_ALLOW_UNSANDBOXED=1"
                );
            }
        } else {
            eprintln!("[af] VT tools disabled (set AF_VT_API_KEY to enable)");
        }

        // Sandbox tools
        if let Some(ref sandbox_socket) = self.config.sandbox_socket {
            af_re_sandbox::declare(specs, sandbox_socket);
            eprintln!("[af] Sandbox tools registered (socket: {})", sandbox_socket.display());
        } else {
            eprintln!("[af] Sandbox tools disabled (set AF_SANDBOX_SOCKET to enable)");
        }

        // YARA tools
        if self.has_yara() && self.can_sandbox() {
            crate::declare_yara(specs, self.config.yara_rules_dir.as_deref());
            eprintln!(
                "[af] YARA tools registered (yara: {}, rules: {})",
                self.config.yara_path.as_ref().unwrap().display(),
                self.config.yara_rules_dir.as_deref().map_or("(none)".into(), |p| p.display().to_string()),
            );
        } else if self.has_yara() {
            eprintln!(
                "[af] YARA tools DISABLED: no sandbox available. \
                 Install af-executor or set AF_ALLOW_UNSANDBOXED=1"
            );
        } else {
            eprintln!(
                "[af] YARA tools disabled (yara binary not found; set AF_YARA_PATH to enable)"
            );
        }

        // Transform tools — always registered (pure Rust, no external deps)
        if self.can_sandbox() {
            crate::declare_transform(specs);
            eprintln!("[af] Transform tools registered");
        } else {
            eprintln!("[af] Transform tools DISABLED: no sandbox available");
        }

        // Document tools — always registered (pure Rust, no external deps)
        if self.can_sandbox() {
            crate::declare_doc(specs);
            eprintln!("[af] Document tools registered");
        } else {
            eprintln!("[af] Document tools DISABLED: no sandbox available");
        }
    }

    fn wire(
        &self,
        executors: &mut ToolExecutorRegistry,
        evidence: &mut EvidenceResolverRegistry,
        _renderers: &mut ToolRendererRegistry,
        plugin_db: Arc<dyn PluginDb>,
    ) {
        // Rizin tools
        if self.has_rizin() && self.can_sandbox() {
            let rizin_path = self.config.rizin_path.as_ref().unwrap();
            crate::wire(
                executors,
                evidence,
                Arc::clone(&plugin_db),
                rizin_path,
                self.config.executor_path.as_deref(),
            );
        }

        // IOC, artifact, family, dedup tools — always wired
        crate::wire_ioc(executors, Arc::clone(&plugin_db));
        crate::wire_artifact(executors, Arc::clone(&plugin_db));
        crate::wire_family(executors, Arc::clone(&plugin_db));
        crate::wire_dedup(executors, Arc::clone(&plugin_db));

        // Ghidra tools
        if self.has_ghidra() && self.can_sandbox() {
            let ghidra_home = self.config.ghidra_home.as_ref().unwrap();
            let cache = self
                .config
                .ghidra_cache
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("/tmp/af/ghidra_cache"));
            let scripts = self
                .config
                .ghidra_scripts
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("ghidra-scripts"));
            crate::wire_ghidra(
                executors,
                Arc::clone(&plugin_db),
                ghidra_home,
                cache,
                scripts,
                self.config.executor_path.as_deref(),
            );
        }

        // VT tools
        if let Some(ref vt_socket) = self.config.vt_socket {
            if self.can_sandbox() {
                af_re_vt::wire(executors, vt_socket, self.config.executor_path.as_deref());
            }
        }

        // Sandbox tools (Trusted — in-process, talk to gateway via UDS)
        if let Some(ref sandbox_socket) = self.config.sandbox_socket {
            af_re_sandbox::wire(executors, sandbox_socket);
        }

        // YARA tools
        if self.has_yara() && self.can_sandbox() {
            let yara_path = self.config.yara_path.as_ref().unwrap();
            crate::wire_yara(
                executors,
                yara_path,
                self.config.yara_rules_dir.as_deref(),
                self.config.executor_path.as_deref(),
                Arc::clone(&plugin_db),
            );
        }

        // Transform tools
        if self.can_sandbox() {
            crate::wire_transform(executors, self.config.executor_path.as_deref());
        }

        // Document tools
        if self.can_sandbox() {
            crate::wire_doc(executors, self.config.executor_path.as_deref());
        }
    }

    fn agent_configs(&self) -> Vec<AgentConfig> {
        crate::agents::agent_configs()
    }

    fn workflows(&self) -> Vec<WorkflowDef> {
        vec![WorkflowDef {
            name: "full-analysis".to_string(),
            description: Some(
                "Standard RE pipeline: surface + intel parallel, then decompiler, then reporter"
                    .to_string(),
            ),
            steps: vec![
                WorkflowStepDef {
                    agent: "surface".into(),
                    group: 1,
                    prompt: "Perform quick surface triage.".into(),
                    can_repivot: true,
                    timeout_secs: None,
                },
                WorkflowStepDef {
                    agent: "intel".into(),
                    group: 1,
                    prompt: "Look up threat intelligence.".into(),
                    can_repivot: true,
                    timeout_secs: None,
                },
                WorkflowStepDef {
                    agent: "decompiler".into(),
                    group: 2,
                    prompt: "Analyze key functions found in surface triage.".into(),
                    can_repivot: true,
                    timeout_secs: None,
                },
                WorkflowStepDef {
                    agent: "reporter".into(),
                    group: 3,
                    prompt: "Write report synthesizing all findings.".into(),
                    can_repivot: true,
                    timeout_secs: None,
                },
            ],
        }]
    }

    fn post_tool_hooks(&self, plugin_db: Arc<dyn PluginDb>) -> Vec<Arc<dyn PostToolHook>> {
        vec![
            Arc::new(crate::ioc_hook::IocPostToolHook {
                plugin_db: Arc::clone(&plugin_db),
            }),
            Arc::new(crate::yara_hook::YaraPostToolHook { plugin_db }),
        ]
    }

    fn tool_config_hooks(&self, plugin_db: Arc<dyn PluginDb>) -> Vec<Arc<dyn ToolConfigHook>> {
        if self.has_ghidra() {
            vec![Arc::new(GhidraToolConfigHook { plugin_db })]
        } else {
            vec![]
        }
    }

    fn metadata(&self) -> serde_json::Value {
        if let Some(ref cache) = self.config.ghidra_cache {
            serde_json::json!({"ghidra_cache_dir": cache.to_string_lossy()})
        } else {
            serde_json::json!({})
        }
    }
}

/// Pre-execution hook that enriches tool_config for Ghidra tools:
/// - Injects `nda` flag from project settings
/// - Injects `ghidra_renames` map for decompile overlay
struct GhidraToolConfigHook {
    plugin_db: Arc<dyn PluginDb>,
}

#[async_trait]
impl ToolConfigHook for GhidraToolConfigHook {
    async fn enrich(
        &self,
        tool_name: &str,
        project_id: uuid::Uuid,
        artifacts: &[ArtifactRef],
        tool_config: &mut serde_json::Value,
    ) {
        if !tool_name.starts_with("ghidra.") {
            return;
        }

        let tc = match tool_config.as_object_mut() {
            Some(obj) => obj,
            None => return,
        };

        // Load NDA flag from project
        let is_nda = match self.plugin_db.query_json(
            "SELECT nda FROM projects WHERE id = $1::uuid",
            vec![serde_json::json!(project_id.to_string())],
            None,
        ).await {
            Ok(rows) => rows.first()
                .and_then(|r| r.get("nda"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true), // default to isolated if unknown
            Err(_) => true,
        };
        tc.insert("nda".into(), serde_json::json!(is_nda));

        // Load renames for decompile overlay (only ghidra.decompile uses them)
        if tool_name == "ghidra.decompile" {
            if let Some(art) = artifacts.first() {
                match crate::ghidra_renames_db::get_renames(
                    &self.plugin_db,
                    project_id,
                    &art.sha256,
                ).await {
                    Ok(renames) if !renames.is_empty() => {
                        tc.insert("ghidra_renames".into(), serde_json::json!(renames));
                    }
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Env helpers (used by from_env)
// ---------------------------------------------------------------------------

/// Resolve the Java home that Ghidra will use inside the bwrap sandbox.
///
/// Checks (in order):
/// 1. `JAVA_HOME_OVERRIDE` in `<ghidra_home>/support/launch.properties`
/// 2. `java_home.save` in `~/.ghidra/.ghidra_<version>/`
/// 3. `JAVA_HOME` env var
///
/// Returns `Some(path)` if a valid JDK directory is found.
fn resolve_ghidra_java_home(ghidra_home: &Path) -> Option<PathBuf> {
    // 1. Check launch.properties for JAVA_HOME_OVERRIDE
    let launch_props = ghidra_home.join("support").join("launch.properties");
    if let Ok(contents) = std::fs::read_to_string(&launch_props) {
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("JAVA_HOME_OVERRIDE=") {
                let val = trimmed.strip_prefix("JAVA_HOME_OVERRIDE=").unwrap().trim();
                if !val.is_empty() {
                    let p = PathBuf::from(val);
                    if p.join("bin").join("java").exists() {
                        eprintln!("[af] Ghidra JDK from launch.properties: {}", p.display());
                        return Some(p);
                    }
                }
            }
        }
    }

    // 2. Check java_home.save files under ~/.ghidra
    if let Ok(home) = std::env::var("HOME") {
        let ghidra_user = PathBuf::from(&home).join(".ghidra");
        if let Ok(entries) = std::fs::read_dir(&ghidra_user) {
            for entry in entries.flatten() {
                let save = entry.path().join("java_home.save");
                if save.exists() {
                    if let Ok(contents) = std::fs::read_to_string(&save) {
                        let val = contents.trim();
                        if !val.is_empty() {
                            let p = PathBuf::from(val);
                            if p.join("bin").join("java").exists() {
                                eprintln!("[af] Ghidra JDK from java_home.save: {}", p.display());
                                return Some(p);
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Check JAVA_HOME env
    if let Ok(val) = std::env::var("JAVA_HOME") {
        let p = PathBuf::from(&val);
        if p.join("bin").join("java").exists() {
            eprintln!("[af] Ghidra JDK from JAVA_HOME: {}", p.display());
            return Some(p);
        }
    }

    None
}

/// Returns (home, cache, scripts) if Ghidra is configured and analyzeHeadless exists.
fn resolve_ghidra_paths(exe_dir: &std::path::Path) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let home = PathBuf::from(std::env::var("AF_GHIDRA_HOME").ok()?);
    if !home.join("support").join("analyzeHeadless").exists() {
        return None;
    }
    let cache = std::env::var("AF_GHIDRA_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/af/ghidra_cache"));
    std::fs::create_dir_all(&cache).ok();

    let scripts = [
        exe_dir.join("ghidra-scripts"),
        PathBuf::from("arbeiterfarm/ghidra-scripts"),
        PathBuf::from("ghidra-scripts"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .unwrap_or_else(|| PathBuf::from("ghidra-scripts"));

    Some((home, cache, scripts))
}

/// Resolve the YARA binary path: AF_YARA_PATH env → which → default.
fn resolve_yara_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("AF_YARA_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
        eprintln!("[af] WARNING: AF_YARA_PATH={path} does not exist");
    }
    // Try PATH lookup
    if let Ok(p) = which::which("yara") {
        return Some(p);
    }
    None
}

/// Resolve the YARA rules directory: AF_YARA_RULES_DIR env → ~/.af/yara/.
fn resolve_yara_rules_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AF_YARA_RULES_DIR") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            return Some(p);
        }
        eprintln!("[af] WARNING: AF_YARA_RULES_DIR={dir} is not a directory");
    }
    // Default: ~/.af/yara/
    if let Ok(home) = std::env::var("HOME") {
        let default_dir = PathBuf::from(home).join(".af").join("yara");
        if default_dir.is_dir() {
            return Some(default_dir);
        }
    }
    None
}

/// Find an executor binary: AF_EXECUTOR_PATH env → sibling of exe → PATH lookup.
fn find_executor(name: &str, exe_dir: &std::path::Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("AF_EXECUTOR_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
        eprintln!("[af] WARNING: AF_EXECUTOR_PATH={path} does not exist, trying fallbacks");
    }
    let sibling = exe_dir.join(name);
    if sibling.exists() {
        return Some(sibling);
    }
    which::which(name).ok()
}
