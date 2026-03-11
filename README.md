# Arbeiterfarm

**Multi-agent AI workstation framework in Rust.**

Arbeiterfarm is an SDK for building AI-powered technical workstations where specialized agents collaborate on analysis tasks using real tools — not just chat. Agents run Ghidra, query VirusTotal, trace malware in sandboxed VMs, write YARA rules, and produce verifiable results traceable back to raw tool output.

The first distribution built on this SDK is **Reverse-Arbeiterfarm**: an AI-powered reverse engineering workstation for malware analysis.

## Key Features

- **Multi-agent orchestration** — single agent, workflow pipelines (parallel + sequential groups), and autonomous thinking threads where a supervisor agent decides the analysis strategy at runtime
- **30+ tools** — file analysis, Ghidra decompilation, Rizin disassembly, VirusTotal intelligence, YARA rules, dynamic analysis (Frida + QEMU/KVM), web fetch/search, email, notifications, data transforms (decode/unpack/jq/csv/regex/convert)
- **Any LLM backend** — local (Ollama, vLLM, llama.cpp), OpenAI, Anthropic, Vertex AI. Multi-model routing with per-user access control
- **Sandbox isolation** — all tool execution in bubblewrap (bwrap) namespaces with `--tmpfs / --unshare-all --cap-drop ALL`. Optional [OAIE](https://github.com/dumitru-stama/Oaie) sandbox backend with seccomp BPF + Landlock LSM
- **Multi-tenant** — project-based isolation with RBAC (owner/manager/collaborator/viewer), Postgres RLS, per-user quotas, NDA-flagged projects with audit trails
- **Extensible via plugins** — compiled Rust plugins or TOML-defined tools/agents/workflows loaded from `~/.af/`
- **Local LLM reliability** — dual tool-calling modes (text-based for local models, native API for cloud), sliding window compaction, thread memory, goal anchoring — enables 20B parameter models to run multi-step tool chains
- **Web UI** — vanilla JS SPA (no build step), 4 themes, SSE streaming, project/artifact/conversation management

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Distribution Binary                       │
│  af-re (compiled RE plugin) or af (TOML only)               │
├──────────┬──────────┬───────────┬──────────┬────────────────┤
│  af-api  │af-agents │  af-jobs  │  af-llm  │   af-auth      │
│  HTTP    │  agent   │ job queue │   LLM    │  API key auth  │
│  routes  │  runtime │  worker   │  router  │  RBAC          │
│  SSE     │  orchest.│  OOP exec │ redaction│  project access│
├──────────┴──────────┼───────────┴──────────┴────────────────┤
│  af-web-gateway     │             af-core                   │
│  web.fetch/search   │  Types, traits, registries            │
│  SSRF protection    │  (ToolSpec, ToolExecutor, Plugin)     │
├─────────────────────┼───────────────────────────────────────┤
│  af-db              │             af-storage                │
│  PostgreSQL         │  Content-addressed blobs (SHA256)     │
│  38 migrations      │  Scratch directories                  │
└─────────────────────┴───────────────────────────────────────┘
```

**18 crates** in a single Cargo workspace. Domain-agnostic SDK (`af/crates/af-*`) + domain-specific RE plugin (`arbeiterfarm/crates/af-re-*`).

## Quick Start

### Prerequisites

| Dependency | Required | Purpose |
|---|---|---|
| Rust (edition 2021) | Yes | Build toolchain |
| PostgreSQL 13+ | Yes | Storage (threads, artifacts, agents, jobs) |
| bubblewrap (`bwrap`) | Yes | Sandbox for tool execution |
| rizin | No | Binary analysis |
| Ghidra 11+ | No | Decompilation |

### Build

```bash
cargo build --release
```

Produces four binaries: `af` (generic CLI + API server), `af-re` (RE distribution with compiled plugins), `af-builtin-executor`, `af-re-executor`.

### Setup

```bash
# Create database
make setup-db

# Configure an LLM backend (pick one)
export AF_ANTHROPIC_API_KEY="sk-ant-..."
# or
export AF_LOCAL_ENDPOINT="http://localhost:11434"
export AF_LOCAL_MODEL="llama3"
# or
export AF_OPENAI_API_KEY="sk-..."

# Verify
af-re tool list
```

For persistent configuration, edit `Makefile.local` (auto-generated on first `make` run, gitignored). It holds API keys, model names, and tool paths so you don't need to export env vars every session. See [Development](#development) below.

### Use

```bash
# Create a project and upload a sample
af-re project create malware-sample-42
af-re artifact add suspicious.exe --project <project-id>

# Interactive analysis
af-re chat --project <project-id>

# Run a multi-agent workflow
af-re chat --workflow full-analysis --project <project-id>

# Autonomous thinking thread
af-re think --project <project-id> --goal "Determine if this is APT29-related"

# Start the web UI
af-re serve --bind 127.0.0.1:8080
# Open http://localhost:8080
```

### Web UI

Create an admin user to log in:

```bash
af-re user create --name admin --roles admin
af-re user api-key create --user <user-id> --name "browser"
# Paste the key into the login screen
```

## Agent Orchestration

### Single Agent

One agent with its own system prompt, tool allowlist, and LLM route. Runs an independent tool-call loop (up to 20 iterations).

### Workflows

Sequential groups of agents. Groups execute one after another; agents within a group run in parallel. All agents share the same conversation — later agents see earlier output.

```
Group 1: surface + intel    (parallel — quick triage + threat intel)
Group 2: decompiler         (deep analysis with Ghidra)
Group 3: reporter           (synthesize findings)
```

### Thinking Threads

A supervisor agent autonomously invokes specialists via `meta.*` tools, reads their results, iterates, and synthesizes. Unlike workflows (predefined pipeline), the LLM decides the analysis strategy at runtime.

## Extending

### TOML Tools (`~/.af/tools/`)

```toml
[tool]
name = "custom.hash"
binary = "/usr/local/bin/hash-tool"
protocol = "oop"
description = "Compute hashes"

[tool.input_schema]
type = "object"
required = ["artifact_id"]
[tool.input_schema.properties.artifact_id]
"$ref" = "#/$defs/ArtifactId"

[tool.policy]
sandbox = "NoNetReadOnly"
timeout_ms = 30000
```

### TOML Agents (`~/.af/agents/`)

```toml
name = "my-agent"
route = "auto"
tools = ["file.*", "custom.*"]
timeout_secs = 120

[prompt]
text = "You are a specialized analysis agent."
```

### TOML Workflows (`~/.af/workflows/`)

```toml
[workflow]
name = "my-pipeline"
description = "Custom analysis pipeline"

[[workflow.steps]]
agent = "surface"
group = 1
parallel = true

[[workflow.steps]]
agent = "decompiler"
group = 2
```

See the [examples/](examples/) directory for complete reference with all options.

## Security

- **Sandbox isolation**: bubblewrap namespaces (PID/IPC/UTS/net/cgroup), tmpfs root, capability drop. Optional OAIE backend with seccomp BPF + Landlock LSM
- **Multi-tenant**: Postgres RLS on all tenant-scoped tables, scoped transactions with `SET LOCAL ROLE`, application-level access checks
- **Auth**: SHA-256 hashed API keys, Bearer authentication, RBAC (admin/owner/manager/collaborator/viewer)
- **NDA enforcement**: project-level flag with immutable audit trail, cross-project data leakage prevention
- **Web gateway**: SSRF protection with DNS pinning/rebinding prevention, IPv6 embedding checks, URL allowlist/blocklist, GeoIP blocking
- **Tool restrictions**: admin-controlled per-tool access grants, `web.*` and `notify.*` restricted by default
- **Input validation**: parameterized SQL (sqlx), regex size limits, decompression bomb protection, archive path traversal rejection, bounded reads
- **PII redaction**: conversation text redacted before sending to non-local summarization backends

See [docs/THREAT_MODEL_V4.md](docs/THREAT_MODEL_V4.md) for the full STRIDE analysis.

## Environment Variables

Run `af-re --help` for the full reference. Summary:

| Category | Variable | Description | Default |
|---|---|---|---|
| **Database & Server** | `AF_DATABASE_URL` | Postgres connection | `postgres://af:af@localhost/af` |
| | `AF_DB_POOL_SIZE` | Connection pool size (use >=20 for thinking threads) | `10` |
| | `AF_BIND_ADDR` | Server bind address | `127.0.0.1:8080` |
| | `AF_CORS_ORIGIN` | CORS origin (e.g. `*`) | — |
| | `AF_TLS_CERT` / `AF_TLS_KEY` | TLS certificate and key (PEM) | — |
| | `AF_API_RATE_LIMIT` | Requests/min/key | `60` |
| | `AF_UPLOAD_MAX_BYTES` | Max upload size | `104857600` (100 MB) |
| | `AF_MAX_STREAM_DURATION_SECS` | Global agent/stream timeout | `1800` (30 min) |
| | `AF_MAX_CONCURRENT_STREAMS` | Max concurrent HTTP streams | `5` |
| **Storage** | `AF_STORAGE_ROOT` | Blob storage root | `/tmp/af/storage` |
| | `AF_SCRATCH_ROOT` | Scratch directories | `/tmp/af/scratch` |
| | `AF_CONFIG_PATH` | Config file path | `~/.af/config.toml` |
| **LLM Backends** | `AF_LOCAL_ENDPOINT` | Local LLM server (Ollama/vLLM/llama.cpp) | — |
| | `AF_LOCAL_MODEL` | Local model name | `gpt-oss` |
| | `AF_LOCAL_API_KEY` | Local server API key | — |
| | `AF_LOCAL_MODELS` | Extra local models (comma-separated) | — |
| | `AF_OPENAI_API_KEY` | OpenAI API key | — |
| | `AF_OPENAI_ENDPOINT` | Custom OpenAI-compatible endpoint | — |
| | `AF_OPENAI_MODEL` | OpenAI model | `gpt-4o` |
| | `AF_OPENAI_MODELS` | Extra OpenAI models (comma-separated) | — |
| | `AF_ANTHROPIC_API_KEY` | Anthropic API key | — |
| | `AF_ANTHROPIC_MODEL` | Anthropic model | `claude-sonnet-4-20250514` |
| | `AF_ANTHROPIC_MODELS` | Extra Anthropic models (comma-separated) | — |
| | `AF_VERTEX_ENDPOINT` | Vertex AI endpoint URL | — |
| | `AF_VERTEX_ACCESS_TOKEN` | OAuth2 token for Vertex AI | — |
| | `AF_DEFAULT_ROUTE` | Default LLM backend for auto routing | — |
| **Embeddings** | `AF_EMBEDDING_ENDPOINT` | Embedding server | falls back to `AF_LOCAL_ENDPOINT` |
| | `AF_EMBEDDING_MODEL` | Embedding model | `snowflake-arctic-embed2` |
| | `AF_EMBEDDING_DIMENSIONS` | Vector dimensions | `768` or `1024` |
| **Context** | `AF_USE_CWC` | Use CWC context compiler (set `0` for legacy) | `1` |
| **Tool Paths** | `AF_GHIDRA_HOME` | Ghidra installation directory | — |
| | `AF_GHIDRA_CACHE` | Ghidra project cache | `/tmp/af/ghidra_cache` |
| | `AF_RIZIN_PATH` | rizin binary | `/usr/bin/rizin` |
| | `AF_YARA_PATH` | yara binary | auto-discovered |
| | `AF_YARA_RULES_DIR` | YARA rules directory | `~/.af/yara/` |
| | `AF_EXECUTOR_PATH` | Path to executor binary | auto-discovered |
| | `AF_EXECUTOR_SHA256` | Expected SHA-256 hash of executor | — |
| | `AF_ALLOW_UNSANDBOXED` | Skip bwrap sandbox (dev only, unsafe) | — |
| **VirusTotal** | `AF_VT_API_KEY` | VirusTotal API key | — |
| | `AF_VT_SOCKET` | VT gateway socket | `/run/af/vt_gateway.sock` |
| | `AF_VT_RATE_LIMIT` | Requests per minute | `4` |
| | `AF_VT_CACHE_TTL` | Cache TTL in seconds | `86400` (24h) |
| **Dynamic Analysis** | `AF_SANDBOX_SOCKET` | UDS path for sandbox gateway | — |
| | `AF_SANDBOX_QMP` | QMP Unix socket for QEMU VM | — |
| | `AF_SANDBOX_AGENT` | Guest agent address | `192.168.122.10:9111` |
| | `AF_SANDBOX_SNAPSHOT` | VM snapshot name | `clean` |
| **Web Gateway** | `AF_WEB_GATEWAY_SOCKET` | UDS path (enables web.fetch/web.search) | — |
| **Email** | `AF_EMAIL_RATE_LIMIT` | Global sends per minute | `10` |
| | `AF_EMAIL_PER_USER_RPM` | Per-user sends per minute | `5` |
| | `AF_EMAIL_MAX_RECIPIENTS` | Max recipients per email | `50` |
| | `AF_EMAIL_MAX_BODY_BYTES` | Max email body size | `1048576` (1 MB) |
| **TOML Extensions** | `AF_TOOLS_DIR` | TOML tool definitions | `~/.af/tools/` |
| | `AF_AGENTS_DIR` | TOML agent definitions | `~/.af/agents/` |
| | `AF_WORKFLOWS_DIR` | TOML workflow definitions | `~/.af/workflows/` |
| | `AF_MODELS_DIR` | TOML model cards | `~/.af/models/` |
| | `AF_PLUGINS_DIR` | TOML plugins | `~/.af/plugins/` |
| **Remote CLI** | `AF_REMOTE_URL` | Remote server URL | — |
| | `AF_API_KEY` | API key for remote access | — |

At least one LLM backend (`AF_LOCAL_ENDPOINT`, `AF_OPENAI_API_KEY`, or `AF_ANTHROPIC_API_KEY`) is required for chat/serve.

## Tests

```bash
cargo test --workspace    # 314 unit + integration tests
make test-e2e             # end-to-end (needs running DB)
```

## Documentation

### Guides

| Document | Description |
|---|---|
| [INSTALL.md](docs/INSTALL.md) | Step-by-step installation, prerequisites, environment variables |
| [USAGE.md](docs/USAGE.md) | Complete CLI and API usage reference |
| [OVERVIEW.md](docs/OVERVIEW.md) | Architecture, plugin system, orchestration, multi-tenancy |
| [PROJECT_OVERVIEW.md](docs/PROJECT_OVERVIEW.md) | Business case, conceptual model, evolution |
| [FEATURES.md](docs/FEATURES.md) | Complete feature inventory across all subsystems |

### Architecture & Security

| Document | Description |
|---|---|
| [ARCHITECTURE.txt](docs/ARCHITECTURE.txt) | Execution flow diagrams, budgets/limits, timeout hierarchy |
| [THREAT_MODEL_V4.md](docs/THREAT_MODEL_V4.md) | Full STRIDE threat analysis with data flow diagrams |
| [oop-sandbox-manual.md](docs/oop-sandbox-manual.md) | OOP executor protocol, bwrap sandbox, artifact pipeline |
| [sandbox-dynamic-analysis.md](docs/sandbox-dynamic-analysis.md) | VM-based dynamic analysis setup (Frida + QEMU/KVM) |
| [postgresql-permissions.md](docs/postgresql-permissions.md) | Postgres RLS policies and multi-tenant isolation |

### LLM Reliability

| Document | Description |
|---|---|
| [reliable-local-llm-tool-calling.md](docs/reliable-local-llm-tool-calling.md) | 14 techniques for making local LLMs reliably call tools |
| [local-llm-context-window-tricks.md](docs/local-llm-context-window-tricks.md) | Context compaction, sliding window, thread memory |

### Operations & Roadmap

| Document | Description |
|---|---|
| [tick_hooks_scheduling.md](docs/tick_hooks_scheduling.md) | Background job scheduling and event-driven hooks |
| [AGENT_ROADMAP.md](docs/AGENT_ROADMAP.md) | Agent framework evolution and future plans |
| [oaie-integration-plan.md](docs/oaie-integration-plan.md) | OAIE sandbox backend integration plan |
| [cwc_integration_plan.md](docs/cwc_integration_plan.md) | Context Window Compiler integration plan |

### Examples

Complete TOML reference with annotated examples for extending Arbeiterfarm:

| Directory | Description |
|---|---|
| [examples/local-tools/](examples/local-tools/) | Writing custom tools (simple + OOP protocol, sandbox profiles) |
| [examples/local-agents/](examples/local-agents/) | Defining agents (prompts, tool patterns, routes) |
| [examples/local-workflows/](examples/local-workflows/) | Creating workflows (parallel groups, signals, fan-out) |
| [examples/local-models/](examples/local-models/) | Model cards for local LLMs (47 Ollama examples) |
| [examples/watchdog/](examples/watchdog/) | Event-driven automation with hooks |

## Built-in Agents

| Agent | Tools | Purpose |
|---|---|---|
| default | `file.*` | General file analysis |
| surface | `file.*`, `rizin.*` | Quick binary triage |
| decompiler | `file.*`, `ghidra.*` | Deep decompilation |
| asm | `file.*`, `rizin.*` | Assembly-level analysis |
| intel | `vt.*`, `re-ioc.*`, `family.*`, `dedup.*` | Threat intelligence |
| researcher | `web.fetch`, `web.search` | Web research |
| tracer | `sandbox.*` | Dynamic analysis (Frida + QEMU) |
| transformer | `transform.*` | Data decode/unpack/convert |
| yara-writer | `yara.*` | YARA rule generation |
| reporter | `file.*` | Report synthesis |
| thinker | `meta.*` | Autonomous orchestration |

## Development

### Local Configuration

Running any `make` target for the first time auto-generates `Makefile.local` — a gitignored file for personal settings:

```makefile
# Makefile.local — Personal configuration (NOT tracked by git)

AF_OPENAI_API_KEY    ?= sk-proj-YOUR-KEY
AF_OPENAI_MODEL      ?= gpt-4o
AF_ANTHROPIC_API_KEY ?= sk-ant-YOUR-KEY
AF_ANTHROPIC_MODEL   ?= claude-sonnet-4-20250514

AF_LOCAL_ENDPOINT    ?= http://localhost:11434
AF_LOCAL_MODEL       ?= gpt-oss

GHIDRA_HOME          ?= /opt/ghidra_11.0
AF_RIZIN_PATH        ?= /usr/bin/rizin
OLLAMA_MODEL         ?= gpt-oss
```

The generated template has all values commented out with placeholders. Uncomment and fill in what you use. The Makefile reads these for `make serve`, `make serve-local`, and `make worker`.

To regenerate the template, delete `Makefile.local` and run any make target.

### Make Targets

```
make help             Show all targets
make build            Build release binaries
make check            Fast type check (no codegen)
make test             Run all unit tests (314)
make test-e2e         End-to-end tests (needs DB)
make serve            Start API server + web UI
make serve-local      Start with Ollama backend
make worker           Start background job worker
make tick             Fire due tick hooks (cron-friendly)
make setup-db         Create af user and database
make setup-bwrap      Install bubblewrap sandbox
make db-status        Show tables and row counts
make clean-db         Drop and recreate database
```

## License

MIT License. See [LICENSE](LICENSE).
