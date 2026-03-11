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
│  arbeiterfarm (compiled RE plugin) or af-app (TOML only)    │
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

Produces three binaries: `af` (main CLI + API server), `af-builtin-executor`, `af-executor`.

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
af tool list
```

For persistent configuration, edit `Makefile.local` (auto-generated on first `make` run, gitignored). It holds API keys, model names, and tool paths so you don't need to export env vars every session. See [Development](#development) below.

### Use

```bash
# Create a project and upload a sample
af project create malware-sample-42
af artifact add suspicious.exe --project <project-id>

# Interactive analysis
af chat --project <project-id>

# Run a multi-agent workflow
af chat --workflow full-analysis --project <project-id>

# Autonomous thinking thread
af think --project <project-id> --goal "Determine if this is APT29-related"

# Start the web UI
af serve --bind 127.0.0.1:8080
# Open http://localhost:8080
```

### Web UI

Create an admin user to log in:

```bash
af user create --name admin --roles admin
af user api-key create --user <user-id> --name "browser"
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
