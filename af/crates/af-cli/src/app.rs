use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "af", about = "Arbeiterfarm multi-agent AI workstation",
    after_help = "Use --help for environment variable reference.",
    after_long_help = "\
ENVIRONMENT VARIABLES:

  Database & Server:
    AF_DATABASE_URL          Postgres connection [postgres://af:af@localhost/af]
    AF_DB_POOL_SIZE          Connection pool size [10] (use >=20 for thinking threads)
    AF_BIND_ADDR             Server bind address [127.0.0.1:8080]
    AF_CORS_ORIGIN           CORS origin (e.g. \"*\")
    AF_TLS_CERT / AF_TLS_KEY TLS certificate and key (PEM)
    AF_API_RATE_LIMIT        API rate limit, requests/min/key [60]
    AF_UPLOAD_MAX_BYTES      Max upload size [104857600] (100 MB)
    AF_MAX_STREAM_DURATION_SECS  Global agent/stream timeout [1800] (30 min)
    AF_MAX_CONCURRENT_STREAMS    Max concurrent HTTP streams [5]

  Storage:
    AF_STORAGE_ROOT          Blob storage root [/tmp/af/storage]
    AF_SCRATCH_ROOT          Scratch directories [/tmp/af/scratch]
    AF_CONFIG_PATH           Config file path [~/.af/config.toml]

  LLM Backends (at least one required for chat/serve):
    AF_LOCAL_ENDPOINT        Local LLM server (Ollama/vLLM/llama.cpp)
    AF_LOCAL_MODEL           Local model name [gpt-oss]
    AF_LOCAL_API_KEY         Local server API key (if needed)
    AF_LOCAL_MODELS          Extra local models (comma-separated)
    AF_OPENAI_API_KEY        OpenAI API key
    AF_OPENAI_ENDPOINT       Custom OpenAI-compatible endpoint
    AF_OPENAI_MODEL          OpenAI model [gpt-4o]
    AF_OPENAI_MODELS         Extra OpenAI models (comma-separated)
    AF_ANTHROPIC_API_KEY     Anthropic API key
    AF_ANTHROPIC_MODEL       Anthropic model [claude-sonnet-4-20250514]
    AF_ANTHROPIC_MODELS      Extra Anthropic models (comma-separated)
    AF_VERTEX_ENDPOINT       Vertex AI endpoint URL
    AF_VERTEX_ACCESS_TOKEN   OAuth2 token for Vertex AI
    AF_DEFAULT_ROUTE         Default LLM backend for auto routing

  Embeddings:
    AF_EMBEDDING_ENDPOINT    Embedding server [falls back to AF_LOCAL_ENDPOINT]
    AF_EMBEDDING_MODEL       Embedding model [snowflake-arctic-embed2]
    AF_EMBEDDING_DIMENSIONS  Vector dimensions [768 or 1024]

  Context Compaction:
    AF_USE_CWC               Use CWC context compiler [1]. Set 0 for legacy compaction

  Tool Paths:
    AF_GHIDRA_HOME           Ghidra installation directory
    AF_GHIDRA_CACHE          Ghidra project cache [/tmp/af/ghidra_cache]
    AF_RIZIN_PATH            rizin binary [/usr/bin/rizin]
    AF_YARA_PATH             yara binary (auto-discovered)
    AF_YARA_RULES_DIR        YARA rules directory [~/.af/yara/]
    AF_EXECUTOR_PATH         Path to executor binary (auto-discovered)
    AF_EXECUTOR_SHA256       Expected SHA-256 hash of executor binary
    AF_ALLOW_UNSANDBOXED     Skip bwrap sandbox (dev only, unsafe)

  VirusTotal:
    AF_VT_API_KEY            VirusTotal API key
    AF_VT_SOCKET             VT gateway socket [/run/af/vt_gateway.sock]
    AF_VT_RATE_LIMIT         Requests per minute [4]
    AF_VT_CACHE_TTL          Cache TTL in seconds [86400] (24h)

  Dynamic Analysis (Frida + QEMU/KVM):
    AF_SANDBOX_SOCKET        UDS path for sandbox gateway
    AF_SANDBOX_QMP           QMP Unix socket for QEMU VM
    AF_SANDBOX_AGENT         Guest agent address [192.168.122.10:9111]
    AF_SANDBOX_SNAPSHOT      VM snapshot name [clean]

  Web Gateway:
    AF_WEB_GATEWAY_SOCKET    UDS path (enables web.fetch/web.search)

  Email:
    AF_EMAIL_RATE_LIMIT      Global sends per minute [10]
    AF_EMAIL_PER_USER_RPM    Per-user sends per minute [5]
    AF_EMAIL_MAX_RECIPIENTS  Max recipients per email [50]
    AF_EMAIL_MAX_BODY_BYTES  Max email body size [1048576] (1 MB)

  TOML Extensions:
    AF_TOOLS_DIR             TOML tool definitions [~/.af/tools/]
    AF_AGENTS_DIR            TOML agent definitions [~/.af/agents/]
    AF_WORKFLOWS_DIR         TOML workflow definitions [~/.af/workflows/]
    AF_MODELS_DIR            TOML model cards [~/.af/models/]
    AF_PLUGINS_DIR           TOML plugins [~/.af/plugins/]

  Remote CLI:
    AF_REMOTE_URL            Remote server URL
    AF_API_KEY               API key for remote access
")]
pub struct Cli {
    /// Load only specific TOML plugins by name (can be repeated; omit to load all)
    #[arg(long = "plugin", global = true)]
    pub plugins: Vec<String>,

    /// Remote API server URL (e.g. https://af.example.com)
    #[arg(long, global = true, env = "AF_REMOTE_URL")]
    pub remote: Option<String>,

    /// API key for remote access
    #[arg(long, global = true, env = "AF_API_KEY")]
    pub api_key: Option<String>,

    /// Allow plaintext HTTP for --remote (insecure: sends API key unencrypted)
    #[arg(long, global = true)]
    pub allow_insecure: bool,

    /// Use OAIE sandbox instead of bubblewrap for tool isolation
    #[arg(long, global = true)]
    pub oaie: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage projects
    Project(ProjectCommand),
    /// Manage samples (artifacts)
    Artifact(ArtifactCommand),
    /// Manage and run tools
    Tool(ToolCommand),
    /// Manage conversations (internally: threads)
    Conversation(ThreadCommand),
    /// Interactive chat with an agent
    Chat(ChatCommand),
    /// View audit log
    Audit(AuditCommand),
    /// Manage users and API keys
    User(UserCommand),
    /// Start the HTTP API server
    Serve(ServeCommand),
    /// Manage agents
    Agent(AgentCommand),
    /// Start a worker daemon
    Worker(WorkerCommand),
    /// Manage workflows
    Workflow(WorkflowCommand),
    /// Manage project hooks (event-driven automation)
    Hook(HookCommand),
    /// Start autonomous analysis (thinking thread)
    Think(ThinkCommand),
    /// Manage web fetch URL rules and country blocks
    WebRule(WebRuleCommand),
    /// Manage user tool grants (restricted tools)
    Grant(GrantCommand),
    /// Manage email recipient rules (allowlist/blocklist)
    EmailRule(EmailRuleCommand),
    /// Manage email credentials, tone presets, and scheduled emails
    Email(EmailCommand),
    /// Manage YARA rules and scan results
    Yara(YaraCommand),
    /// Manage the background embedding queue
    EmbedQueue(EmbedQueueCommand),
    /// Manage the URL ingestion queue (bulk URL import for RAG)
    UrlIngest(UrlIngestCommand),
    /// Manage notification channels and queue
    Notify(NotifyCommand),
    /// Manage Ghidra function renames (cross-project rename suggestions)
    GhidraRenames(GhidraRenamesCommand),
    /// Fire all due tick hooks once and exit (designed for cron)
    Tick,
}

#[derive(Parser)]
pub struct ServeCommand {
    /// Bind address (host:port)
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub bind: String,
    /// Path to TLS certificate file (PEM)
    #[arg(long)]
    pub tls_cert: Option<String>,
    /// Path to TLS private key file (PEM)
    #[arg(long)]
    pub tls_key: Option<String>,
    /// Allow serving HTTP (no TLS) on non-localhost addresses
    #[arg(long)]
    pub allow_insecure: bool,
}

#[derive(Parser)]
pub struct ProjectCommand {
    #[command(subcommand)]
    pub action: ProjectAction,
}

#[derive(Subcommand)]
pub enum ProjectAction {
    /// Create a new project
    Create {
        /// Project name
        name: String,
        /// Mark project as NDA (no cross-project data sharing)
        #[arg(long)]
        nda: bool,
    },
    /// List all projects
    List,
    /// List project members
    Members {
        /// Project ID
        #[arg(long)]
        project: String,
    },
    /// Add a member to a project (owner/manager only)
    AddMember {
        /// Project ID
        #[arg(long)]
        project: String,
        /// User ID or @all for public access
        #[arg(long)]
        user: String,
        /// Role: manager, collaborator, or viewer
        #[arg(long, default_value = "viewer")]
        role: String,
    },
    /// Remove a member from a project (owner/manager only)
    RemoveMember {
        /// Project ID
        #[arg(long)]
        project: String,
        /// User ID or @all
        #[arg(long)]
        user: String,
    },
    /// Show or update project settings
    Settings {
        /// Project ID
        project: String,
        /// Set a setting (key=value, e.g. exclude_from_search=true)
        #[arg(long)]
        set: Option<String>,
    },
    /// Delete a project and all its data
    Delete {
        /// Project ID
        project: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Set or clear the NDA flag on a project
    Nda {
        /// Project ID
        project: String,
        /// Set NDA on
        #[arg(long, group = "nda_toggle")]
        on: bool,
        /// Set NDA off
        #[arg(long, group = "nda_toggle")]
        off: bool,
    },
}

#[derive(Parser)]
pub struct ArtifactCommand {
    #[command(subcommand)]
    pub action: ArtifactAction,
}

#[derive(Subcommand)]
pub enum ArtifactAction {
    /// Add a sample file to the project
    Add {
        /// Path to the sample file
        file: String,
        /// Project ID
        #[arg(long)]
        project: String,
    },
    /// List samples in a project
    List {
        /// Project ID
        #[arg(long)]
        project: String,
    },
    /// Show sample details
    Info {
        /// Artifact ID
        id: String,
    },
    /// Delete a specific artifact
    Delete {
        /// Artifact ID
        id: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Delete all generated artifacts in a project (keeps uploaded samples)
    CleanGenerated {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Set sample description
    Describe {
        /// Artifact ID
        id: String,
        /// Description text
        description: String,
    },
}

#[derive(Parser)]
pub struct ToolCommand {
    #[command(subcommand)]
    pub action: ToolAction,
}

#[derive(Subcommand)]
pub enum ToolAction {
    /// List registered tools
    List,
    /// Run a tool
    Run {
        /// Tool name (e.g. "echo.tool")
        name: String,
        /// Project ID
        #[arg(long)]
        project: String,
        /// Input JSON
        #[arg(long)]
        input: String,
    },
    /// Enable a tool
    Enable {
        /// Tool name
        name: String,
    },
    /// Disable a tool
    Disable {
        /// Tool name
        name: String,
    },
    /// Reload local TOML tools (advisory — restart process to take effect)
    Reload,
}

#[derive(Parser)]
pub struct ThreadCommand {
    #[command(subcommand)]
    pub action: ThreadAction,
}

#[derive(Subcommand)]
pub enum ThreadAction {
    /// List conversations in a project (internally: threads)
    List {
        /// Project ID
        #[arg(long)]
        project: String,
    },
    /// Show messages in a conversation
    Show {
        /// Conversation ID (thread UUID)
        id: String,
    },
    /// Delete a conversation and all its messages
    Delete {
        /// Conversation ID (thread UUID)
        id: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Export a conversation as Markdown or JSON
    Export {
        /// Conversation ID (thread UUID)
        id: String,
        /// Export format
        #[arg(long, default_value = "markdown")]
        format: String,
    },
    /// Queue a user message without triggering the LLM
    QueueMessage {
        /// Conversation ID (thread UUID)
        id: String,
        /// Message content
        content: String,
    },
}

#[derive(Parser)]
pub struct ChatCommand {
    /// Agent name (from DB or builtin)
    #[arg(long, conflicts_with = "agent_file")]
    pub agent: Option<String>,

    /// Path to a local agent TOML file
    #[arg(long, conflicts_with = "agent")]
    pub agent_file: Option<String>,

    /// Project ID
    #[arg(long)]
    pub project: String,

    /// Resume an existing conversation (thread UUID)
    #[arg(long)]
    pub conversation: Option<String>,

    /// Run a workflow instead of a single agent
    #[arg(long)]
    pub workflow: Option<String>,
}

#[derive(Parser)]
pub struct ThinkCommand {
    /// Project ID
    #[arg(long)]
    pub project: String,

    /// Analysis goal
    #[arg(long)]
    pub goal: String,

    /// Thinker agent name (default: "thinker")
    #[arg(long, conflicts_with = "agent_file")]
    pub agent: Option<String>,

    /// Path to a local agent TOML file
    #[arg(long, conflicts_with = "agent")]
    pub agent_file: Option<String>,

    /// Resume an existing thinking conversation (thread UUID)
    #[arg(long)]
    pub conversation: Option<String>,
}

#[derive(Parser)]
pub struct AuditCommand {
    #[command(subcommand)]
    pub action: AuditAction,
}

#[derive(Subcommand)]
pub enum AuditAction {
    /// List recent audit log entries
    List {
        /// Maximum number of entries
        #[arg(long, default_value = "50")]
        limit: i64,
        /// Filter by event type
        #[arg(long, name = "type")]
        event_type: Option<String>,
    },
}

#[derive(Parser)]
pub struct UserCommand {
    #[command(subcommand)]
    pub action: UserAction,
}

#[derive(Subcommand)]
pub enum UserAction {
    /// Create a new user
    Create {
        /// Unique subject identifier (e.g. "alice" or "oidc:alice@example.com")
        #[arg(long)]
        name: String,
        /// Display name
        #[arg(long)]
        display: Option<String>,
        /// Email address
        #[arg(long)]
        email: Option<String>,
        /// Comma-separated roles (e.g. "operator,admin")
        #[arg(long, default_value = "operator")]
        roles: String,
    },
    /// List all users
    List,
    /// Manage API keys
    ApiKey(ApiKeyCommand),
    /// Manage allowed LLM routes for a user (admin)
    Routes(UserRoutesCommand),
}

#[derive(Parser)]
pub struct UserRoutesCommand {
    /// User ID (UUID)
    pub user_id: String,
    /// Add a route (e.g. "openai:gpt-4o-mini" or "openai:*")
    #[arg(long)]
    pub add: Option<String>,
    /// Remove a specific route
    #[arg(long)]
    pub remove: Option<String>,
    /// Remove all routes (return to unrestricted)
    #[arg(long)]
    pub clear: bool,
}

#[derive(Parser)]
pub struct ApiKeyCommand {
    #[command(subcommand)]
    pub action: ApiKeyAction,
}

#[derive(Subcommand)]
pub enum ApiKeyAction {
    /// Create a new API key for a user
    Create {
        /// User ID (UUID)
        #[arg(long)]
        user: String,
        /// Key name / description
        #[arg(long)]
        name: String,
    },
    /// List API keys for a user
    List {
        /// User ID (UUID)
        #[arg(long)]
        user: String,
    },
    /// Revoke (delete) an API key
    Revoke {
        /// API key ID (UUID)
        id: String,
    },
}

// --- Agents ---

#[derive(Parser)]
pub struct AgentCommand {
    #[command(subcommand)]
    pub action: AgentAction,
}

#[derive(Subcommand)]
pub enum AgentAction {
    /// List all agents
    List,
    /// Show agent details
    Show {
        /// Agent name
        name: String,
    },
    /// Create a new agent
    Create {
        /// Agent name
        #[arg(long)]
        name: String,
        /// System prompt
        #[arg(long)]
        prompt: String,
        /// Comma-separated tool patterns (e.g. "file.*,rizin.*")
        #[arg(long)]
        tools: String,
        /// LLM route (auto, local, backend:<name>)
        #[arg(long, default_value = "auto")]
        route: String,
        /// Timeout in seconds for this agent (optional)
        #[arg(long)]
        timeout: Option<u32>,
    },
    /// Delete an agent (cannot delete builtins)
    Delete {
        /// Agent name
        name: String,
    },
    /// Promote a local TOML agent file to the database
    Promote {
        /// Path to the agent TOML file
        file: String,
        /// Overwrite if agent already exists
        #[arg(long)]
        force: bool,
    },
}

// --- Worker ---

#[derive(Parser)]
pub struct WorkerCommand {
    #[command(subcommand)]
    pub action: WorkerAction,
}

#[derive(Subcommand)]
pub enum WorkerAction {
    /// Start the worker daemon
    Start {
        /// Number of concurrent workers
        #[arg(long, default_value = "4")]
        concurrency: u32,
        /// Poll interval in milliseconds when idle
        #[arg(long, default_value = "500")]
        poll_ms: u64,
    },
}

// --- Workflows ---

#[derive(Parser)]
pub struct WorkflowCommand {
    #[command(subcommand)]
    pub action: WorkflowAction,
}

#[derive(Subcommand)]
pub enum WorkflowAction {
    /// List all workflows
    List,
    /// Show workflow details
    Show {
        /// Workflow name
        name: String,
    },
    /// Validate a workflow TOML file without registering
    Validate {
        /// Path to workflow TOML file
        file: String,
    },
}

// --- Hooks ---

#[derive(Parser)]
pub struct HookCommand {
    #[command(subcommand)]
    pub action: HookAction,
}

#[derive(Subcommand)]
pub enum HookAction {
    /// List hooks in a project
    List {
        /// Project ID
        #[arg(long)]
        project: String,
    },
    /// Create a new hook
    Create {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Hook name (unique per project)
        #[arg(long)]
        name: String,
        /// Event type: artifact_uploaded or tick
        #[arg(long)]
        event: String,
        /// Workflow to run (mutually exclusive with --agent)
        #[arg(long, conflicts_with = "agent")]
        workflow: Option<String>,
        /// Agent to run (mutually exclusive with --workflow)
        #[arg(long, conflicts_with = "workflow")]
        agent: Option<String>,
        /// Prompt template (supports {{variable}} placeholders)
        #[arg(long)]
        prompt: String,
        /// LLM route override
        #[arg(long)]
        route: Option<String>,
        /// Tick interval in minutes (required for tick hooks)
        #[arg(long)]
        interval: Option<i32>,
    },
    /// Show hook details
    Show {
        /// Hook ID (UUID)
        id: String,
    },
    /// Enable a hook
    Enable {
        /// Hook ID (UUID)
        id: String,
    },
    /// Disable a hook
    Disable {
        /// Hook ID (UUID)
        id: String,
    },
    /// Delete a hook
    Delete {
        /// Hook ID (UUID)
        id: String,
    },
}

// --- Web Rules ---

#[derive(Parser)]
pub struct WebRuleCommand {
    #[command(subcommand)]
    pub action: WebRuleAction,
}

#[derive(Subcommand)]
pub enum WebRuleAction {
    /// Add a URL rule
    Add {
        /// Block matching URLs
        #[arg(long, conflicts_with = "allow")]
        block: bool,
        /// Allow matching URLs
        #[arg(long, conflicts_with = "block")]
        allow: bool,
        /// Match exact domain
        #[arg(long, group = "pattern_group")]
        domain: Option<String>,
        /// Match domain suffix (e.g. .ru)
        #[arg(long, group = "pattern_group")]
        domain_suffix: Option<String>,
        /// Match URL prefix
        #[arg(long, group = "pattern_group")]
        url_prefix: Option<String>,
        /// Match URL by regex
        #[arg(long, group = "pattern_group")]
        url_regex: Option<String>,
        /// Match resolved IP by CIDR (e.g. 5.0.0.0/8)
        #[arg(long, group = "pattern_group")]
        ip_cidr: Option<String>,
        /// Human-readable description
        #[arg(long)]
        description: Option<String>,
        /// Scope to a specific project (omit for global)
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove a URL rule by ID
    Remove {
        /// Rule UUID
        id: String,
    },
    /// List URL rules
    List {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
    },
    /// Block a country by ISO 3166-1 alpha-2 code
    BlockCountry {
        /// Country code (e.g. RU, CN, KP)
        code: String,
        /// Country name
        #[arg(long)]
        name: Option<String>,
    },
    /// Unblock a country
    UnblockCountry {
        /// Country code
        code: String,
    },
    /// List blocked countries
    ListCountries,
}

// --- Grants ---

#[derive(Parser)]
pub struct GrantCommand {
    #[command(subcommand)]
    pub action: GrantAction,
}

#[derive(Subcommand)]
pub enum GrantAction {
    /// Grant a user access to a restricted tool pattern
    Tool {
        /// User UUID
        user_id: String,
        /// Tool pattern (e.g. web.*, web.fetch)
        pattern: String,
    },
    /// Revoke a user's tool grant
    Revoke {
        /// User UUID
        user_id: String,
        /// Tool pattern to revoke
        pattern: String,
    },
    /// List a user's tool grants
    List {
        /// User UUID
        user_id: String,
    },
    /// List all restricted tool patterns
    Restricted,
    /// Mark a tool pattern as restricted (requires admin grants to use)
    Restrict {
        /// Tool pattern (e.g. web.*)
        pattern: String,
        /// Description
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a tool restriction
    Unrestrict {
        /// Tool pattern to unrestrict
        pattern: String,
    },
}

// --- Email Rules ---

#[derive(Parser)]
pub struct EmailRuleCommand {
    #[command(subcommand)]
    pub action: EmailRuleAction,
}

#[derive(Subcommand)]
pub enum EmailRuleAction {
    /// Add an email recipient rule
    Add {
        /// Block matching recipients
        #[arg(long, conflicts_with = "allow")]
        block: bool,
        /// Allow matching recipients
        #[arg(long, conflicts_with = "block")]
        allow: bool,
        /// Match exact email address
        #[arg(long, group = "pattern_group")]
        email: Option<String>,
        /// Match domain (e.g. example.com)
        #[arg(long, group = "pattern_group")]
        domain: Option<String>,
        /// Match domain suffix (e.g. .gov)
        #[arg(long, group = "pattern_group")]
        domain_suffix: Option<String>,
        /// Human-readable description
        #[arg(long)]
        description: Option<String>,
        /// Scope to a specific project (omit for global)
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove a recipient rule by ID
    Remove {
        /// Rule UUID
        id: String,
    },
    /// List recipient rules
    List {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
    },
}

// --- Email Management ---

#[derive(Parser)]
pub struct EmailCommand {
    #[command(subcommand)]
    pub action: EmailAction,
}

#[derive(Subcommand)]
pub enum EmailAction {
    /// Configure email credentials for a user
    Setup {
        /// Provider: gmail or protonmail
        provider: String,
        /// User UUID
        #[arg(long)]
        user: String,
        /// Email address
        #[arg(long)]
        address: String,
        /// Credentials JSON string or @filepath
        #[arg(long)]
        credentials: String,
        /// Set as default account
        #[arg(long)]
        default: bool,
    },
    /// List email accounts for a user
    Accounts {
        /// User UUID
        #[arg(long)]
        user: String,
    },
    /// Remove an email credential
    RemoveAccount {
        /// Credential UUID
        id: String,
    },
    /// Manage tone presets
    Tones(EmailTonesCommand),
    /// List scheduled emails
    Scheduled {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
        /// Filter by status (scheduled, sending, sent, failed, cancelled)
        #[arg(long)]
        status: Option<String>,
    },
    /// Cancel a scheduled email
    Cancel {
        /// Scheduled email UUID
        id: String,
    },
}

#[derive(Parser)]
pub struct EmailTonesCommand {
    #[command(subcommand)]
    pub action: EmailTonesAction,
}

#[derive(Subcommand)]
pub enum EmailTonesAction {
    /// List all tone presets
    List,
    /// Add or update a custom tone preset
    Add {
        /// Preset name
        name: String,
        /// Description
        #[arg(long)]
        description: String,
        /// System instruction text
        #[arg(long)]
        instruction: String,
    },
    /// Remove a custom tone preset (builtins cannot be removed)
    Remove {
        /// Preset name
        name: String,
    },
}

// --- YARA ---

#[derive(Parser)]
pub struct YaraCommand {
    #[command(subcommand)]
    pub action: YaraAction,
}

#[derive(Subcommand)]
pub enum YaraAction {
    /// List YARA rules
    List {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
        /// Filter by name, description, or tag (substring match)
        #[arg(long)]
        filter: Option<String>,
    },
    /// Show full YARA rule source
    Show {
        /// Rule UUID
        id: String,
    },
    /// Remove a YARA rule
    Remove {
        /// Rule UUID
        id: String,
    },
    /// List YARA scan results
    ScanResults {
        /// Filter by artifact UUID
        #[arg(long)]
        artifact: Option<String>,
        /// Filter by rule name
        #[arg(long)]
        rule: Option<String>,
    },
}

// --- Embed Queue ---

#[derive(Parser)]
pub struct EmbedQueueCommand {
    #[command(subcommand)]
    pub action: EmbedQueueAction,
}

#[derive(Subcommand)]
pub enum EmbedQueueAction {
    /// List embed queue items
    List {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
        /// Filter by status (pending, processing, completed, failed, cancelled)
        #[arg(long)]
        status: Option<String>,
    },
    /// Cancel a pending embed queue item
    Cancel {
        /// Queue item UUID
        id: String,
    },
    /// Retry a failed embed queue item
    Retry {
        /// Queue item UUID
        id: String,
    },
}

// --- URL Ingest ---

#[derive(Parser)]
pub struct UrlIngestCommand {
    #[command(subcommand)]
    pub action: UrlIngestAction,
}

#[derive(Subcommand)]
pub enum UrlIngestAction {
    /// Submit URLs for ingestion into the knowledge base
    Submit {
        /// Project ID
        project: String,
        /// URLs to ingest (one or more)
        urls: Vec<String>,
    },
    /// List URL ingest queue items
    List {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
        /// Filter by status (pending, processing, completed, failed, cancelled)
        #[arg(long)]
        status: Option<String>,
    },
    /// Cancel a pending URL ingest item
    Cancel {
        /// Queue item UUID
        id: String,
    },
    /// Retry a failed URL ingest item
    Retry {
        /// Queue item UUID
        id: String,
    },
}

// --- Notifications ---

#[derive(Parser)]
pub struct NotifyCommand {
    #[command(subcommand)]
    pub action: NotifyAction,
}

#[derive(Subcommand)]
pub enum NotifyAction {
    /// Manage notification channels
    Channel(NotifyChannelCommand),
    /// Manage notification queue
    Queue(NotifyQueueCommand),
}

#[derive(Parser)]
pub struct NotifyChannelCommand {
    #[command(subcommand)]
    pub action: NotifyChannelAction,
}

#[derive(Subcommand)]
pub enum NotifyChannelAction {
    /// Add a notification channel to a project
    Add {
        /// Project ID
        project: String,
        /// Channel name (unique per project)
        name: String,
        /// Channel type (webhook, email, matrix, webdav)
        channel_type: String,
        /// Channel configuration as JSON
        #[arg(long)]
        config: String,
    },
    /// List notification channels for a project
    List {
        /// Project ID
        project: String,
    },
    /// Remove a notification channel
    Remove {
        /// Channel UUID
        id: String,
    },
    /// Send a test notification to a channel
    Test {
        /// Channel UUID
        id: String,
    },
}

#[derive(Parser)]
pub struct NotifyQueueCommand {
    #[command(subcommand)]
    pub action: NotifyQueueAction,
}

#[derive(Subcommand)]
pub enum NotifyQueueAction {
    /// List notification queue items
    List {
        /// Filter by project ID
        #[arg(long)]
        project: Option<String>,
        /// Filter by status (pending, processing, completed, failed, cancelled)
        #[arg(long)]
        status: Option<String>,
    },
    /// Cancel a pending notification
    Cancel {
        /// Queue item UUID
        id: String,
    },
    /// Retry a failed notification
    Retry {
        /// Queue item UUID
        id: String,
    },
}

// --- Ghidra Renames ---

#[derive(Parser)]
pub struct GhidraRenamesCommand {
    #[command(subcommand)]
    pub action: GhidraRenamesAction,
}

#[derive(Subcommand)]
pub enum GhidraRenamesAction {
    /// List renames for a binary in a project
    List {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Binary SHA256
        #[arg(long)]
        sha256: String,
    },
    /// Show cross-project rename suggestions
    Suggest {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Binary SHA256
        #[arg(long)]
        sha256: String,
    },
    /// Import renames from another project
    Import {
        /// Target project ID (import into)
        #[arg(long)]
        project: String,
        /// Binary SHA256
        #[arg(long)]
        sha256: String,
        /// Source project ID (import from)
        #[arg(long)]
        from_project: String,
    },
}

pub fn parse() -> Cli {
    Cli::parse()
}
