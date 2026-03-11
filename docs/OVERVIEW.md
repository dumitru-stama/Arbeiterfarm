# Arbeiterfarm — Multi-Agent AI Workstation

## What is Arbeiterfarm?

Arbeiterfarm is a generic multi-agent AI workstation framework written in Rust. It provides the infrastructure for running specialized AI agents that collaborate on analysis tasks using domain-specific tools. Agents are orchestrated in configurable workflows where groups of agents execute in parallel or sequentially, sharing results through a common conversation thread.

The framework is domain-agnostic — the core handles projects, artifacts, conversations, LLM routing, tool execution, sandboxing, authentication, and multi-tenancy. Domain-specific functionality is provided by **plugins** that register their own tools, agents, workflows, and database schemas.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Distribution Binary                       │
│  af (compiled RE plugin)  or  af (TOML plugins only)    │
├─────────────────────────────────────────────────────────────────┤
│                         af-cli                                 │
│  CLI parsing, bootstrap, plugin runner, backend trait, TOML     │
├──────────┬──────────┬───────────┬──────────┬────────────────────┤
│ af-api │af-agents│ af-jobs │ af-llm │    af-auth      │
│ HTTP API │ agent     │ job queue │ LLM      │    API key auth   │
│ routes   │ runtime   │ worker    │ router   │    RBAC           │
│ SSE      │ orchestr. │ OOP exec  │ redaction│    project access │
├──────────┴──────────┼───────────┴──────────┴────────────────────┤
│   af-web-gateway  │              af-core                     │
│   web.fetch/search  │  Types, traits, registries                 │
│   SSRF, URL rules   │  (ToolSpec, ToolExecutor, Plugin, etc)    │
├──────────┬──────────┴───────────────────────────────────────────┤
│ af-db  │                    af-storage                      │
│ Postgres │              Content-addressed blobs                  │
│ migrations│             Scratch directories                      │
└──────────┴──────────────────────────────────────────────────────┘
```

### Crate Breakdown

| Crate | Purpose |
|---|---|
| **af-core** | Domain types, traits, registries. `ToolSpec`, `ToolExecutor`, `Plugin`, `AgentConfig`, `Identity`. No I/O. |
| **af-plugin-api** | Thin re-export crate — the only dependency plugins need. |
| **af-db** | Postgres migrations + runtime queries (sqlx runtime, NOT query! macros). Schema, RLS policies. |
| **af-storage** | Content-addressed blob storage, scratch directories, output store. |
| **af-jobs** | Job queue (Postgres-backed), worker daemon, OOP executor with bwrap sandbox. |
| **af-llm** | LLM abstraction: OpenAI-compatible, Anthropic, Vertex AI. Router with aliases. PII redaction. |
| **af-agents** | Agent runtime (tool-call loop), prompt builder, evidence parser, orchestrator. Meta-tools for thinking threads. Automatic context compaction. |
| **af-builtin-tools** | Domain-agnostic file analysis tools: `file.info`, `file.read_range`, `file.strings`, `file.hexdump`, `file.grep`. |
| **af-auth** | API key authentication (SHA-256), RBAC authorizer, project access checks. |
| **af-web-gateway** | Web gateway daemon: UDS-based web.fetch/web.search tools, SSRF protection, DNS pinning, URL rules, GeoIP blocking, rate limiting, response caching. |
| **af-api** | Axum HTTP API server. REST routes, SSE streaming, rate limiting, DTOs, CORS, TLS. |
| **af-cli** | CLI library: argument parsing, bootstrap, plugin runner, backend trait (DirectDb + RemoteApi), local TOML loading, command dispatch. |

## Plugin System

Arbeiterfarm supports two types of plugins:

### Compiled Rust Plugins

Implement the `Plugin` trait from `af-plugin-api`. These are compiled into the distribution binary and have full access to all framework capabilities:

- **Custom tool executors** (in-process or OOP)
- **Custom database schemas** (via `ScopedPluginDb` with per-schema RLS)
- **Post-tool hooks** (e.g., IOC extraction after every tool run)
- **Evidence resolvers** (custom `evidence:plugin:id` reference types)
- **VT gateway and other background services**
- **Agent presets and workflow definitions**

Example: **Arbeiterfarm** (`arbeiterfarm/`) — registers Rizin, Ghidra, VirusTotal, IOC, and malware family tools with specialized agents (surface, decompiler, asm, intel, reporter) and the "full-analysis" workflow.

### TOML Plugins

Defined in `~/.af/plugins/` (or `AF_PLUGINS_DIR`). A TOML plugin is a directory containing tool, agent, and workflow TOML files. Loaded at startup — no recompilation needed.

Individual TOML tools, agents, and workflows can also be placed directly in `~/.af/tools/`, `~/.af/agents/`, `~/.af/workflows/`.

### Source Tracking

Every tool, agent, and workflow is tagged with its origin source:

| Label | Meaning |
|---|---|
| `"builtin"` | Compiled-in file analysis tools, default agent |
| `"re"` | Compiled Rust plugin (reverse engineering) |
| `"<plugin-name>"` | TOML plugin from `~/.af/plugins/` |
| `"local"` | TOML files from `~/.af/tools/` etc. |
| `"user"` | Created via API at runtime |

Source labels are tracked in the `SourceMap` side-map (not embedded in core types) and flow from plugin_runner → bootstrap → CliConfig → AppState → API → UI. The `source_plugin` column in the DB persists source for agents and workflows.

The `GET /api/v1/plugins` endpoint returns a plugin inventory — each plugin with its tools, agents, and workflows.

## Distribution Binaries

### `af` (Reverse Engineering)

The primary distribution binary. Compiles the RE plugin (Rizin, Ghidra, VirusTotal, IOC, malware families) directly in. Only serves its compiled plugin by default — TOML plugins loaded only when explicitly requested:

```bash
af chat --agent surface --project p1     # only RE plugin
af --plugin fuzzer serve                  # RE + fuzzer TOML plugin
af serve --bind 0.0.0.0:8080             # RE plugin, HTTP API
```

### `af` (Generic)

The generic binary for TOML-only deployments. No compiled plugins — all functionality comes from TOML tools/agents/workflows:

```bash
af chat --agent assistant --project p1                # all TOML plugins
af --plugin personal-assistant chat ...               # specific plugin
af --plugin pa --plugin fuzzer serve --bind 0:9090    # two TOML plugins
```

## Remote CLI Access

The CLI can operate against a remote Arbeiterfarm API server instead of a local Postgres database:

```bash
# Via CLI args
af --remote https://af.example.com --api-key af_xxxx project list

# Via environment variables
export AF_REMOTE_URL=https://af.example.com
export AF_API_KEY=af_xxxx
af project list
```

All data commands (project, artifact, conversation, agent, workflow, hook, audit, user) work remotely. Local-only commands (serve, worker, tick, chat, tool) reject `--remote`.

HTTPS is enforced by default — `http://` URLs are rejected unless `--allow-insecure` is passed. Requests have a 120s timeout (10s connect timeout).

The `Backend` trait (`af-cli/src/backend/mod.rs`) abstracts the data layer:
- **DirectDb** — wraps PgPool, calls `af_db::*` directly (default, no `--remote`). Runs as `af` DB owner (no RLS) — trusted local access only.
- **RemoteApi** — wraps `reqwest::Client` with Bearer auth, maps to REST endpoints. All tenant isolation enforced server-side via RLS.

## Tool Calling

Tools communicate via stdin/stdout JSON — two protocols:

### Simple Protocol
Tool reads flat JSON from stdin (artifact UUIDs replaced with file paths), writes flat JSON to stdout. No envelope.

### OOP Protocol
Tool receives `OopEnvelope` on stdin (includes scratch dir, context), writes `OopResponse` on stdout. Supports `produced_files` and `context_extra`.

### Sandbox Execution

All non-trusted tools run in bwrap sandbox:
- `--tmpfs /` (empty filesystem root)
- `--ro-bind` only specific artifact files + system paths
- `--unshare-all` (PID/IPC/UTS/net/cgroup isolation)
- `--cap-drop ALL` (strip all capabilities)
- `--die-with-parent` (cleanup on crash)
- Fail-closed when bwrap unavailable (unless `AF_ALLOW_UNSANDBOXED=1`)

### LLM Tool Calling

Two modes, auto-selected based on LLM backend capabilities:

**Mode A (Text-based)**: For local models without native tool calling. Tool descriptions injected as JSON blocks in the system prompt. Agent parses `<tool_call>` JSON from model output, executes tool, appends result to context, submits new request.

**Mode B (Native API)**: For cloud LLMs (Claude, GPT-4o, Gemini). Tool descriptions sent via the API's native tool-calling mechanism. Same request-response loop — model returns tool call, agent executes, appends result, new request. Up to `MAX_TOOL_CALLS=20` iterations.

Both modes use the same agent loop: send messages → get response → if tool call, execute + loop → if done, return.

## Agent Orchestration

### Single Agent

```bash
af chat --agent surface --project p1
```

One agent with its own system prompt, tool allowlist, and LLM route. Runs an independent tool-call loop.

### Workflow (Multi-Agent)

```bash
af chat --workflow full-analysis --project p1
```

A pipeline of agents organized into groups:
- **Groups execute sequentially** (Group 1 completes before Group 2 starts)
- **Agents within a group run in parallel**
- **All agents share the same conversation thread** — later agents see earlier agents' output
- **Agent attribution**: Messages tagged with `agent_name`, prefixed with `[agent_name]:` in history

Example "full-analysis" workflow:
```
Group 1: surface + intel (parallel — quick triage + threat intel)
Group 2: decompiler (deep analysis using Ghidra)
Group 3: reporter (synthesize findings into report)
```

### Thinking Thread (Autonomous Orchestration)

```bash
af think --project p1 --goal "Determine if this sample is APT29-related"
```

A supervisor "thinker" agent autonomously decides which specialist agents to invoke, reads their results, iterates, and synthesizes findings. Unlike workflows (predefined pipeline), thinking threads let the LLM decide the analysis strategy at runtime.

5 internal tools (`meta.*`, all `SandboxProfile::Trusted`):
- `meta.invoke_agent` — spawns child agent on a new child thread, runs to completion
- `meta.read_thread` — reads messages from a child thread (project-scoped)
- `meta.list_agents` — lists available specialists with sanitized descriptions (first sentence, 120 char cap) and tools; does not expose raw system prompts or routes
- `meta.list_artifacts` — lists project artifacts
- `meta.read_artifact` — reads artifact content (text or hex dump, 128KB per read)

Anti-recursion: `meta.*` tools stripped from child agents. Budget: 30 meta-tool calls. Timeout hierarchy: child (300s) < meta-tool (660s) < thinker (1800s).

Custom thinkers: any TOML agent with `tools = ["internal.*"]` becomes a thinker.

### Fan-Out

When a tool extracts multiple files from a container (zip, tar), each child artifact gets its own thread running the full workflow. Parent blocks until all children complete. Depth capped at 3 (with SHA256 cycle detection), width at 50 per fan-out event.

### Context Compaction (Two-Tier)

Long conversations (especially thinking threads) can exceed the LLM's context window. The compaction strategy differs based on model type:

**Cloud models** (Claude, GPT-4o, Gemini):
- Token estimation: heuristic (~4 chars/token + framing overhead)
- Trigger: when estimated tokens exceed `context_window * 0.85 - max_output_tokens`
- Strategy: preserve system prompt (head) + recent messages (tail), LLM summarizes middle
- Redaction: conversation text redacted via `RedactionLayer` before sending to non-local summarization backends
- Optional separate summarization backend via `config.toml` `[compaction] summarization_route`

**Local models** (gpt-oss, Qwen3, DeepSeek, etc.) — three-layer defense, zero LLM cost:
1. **Sliding window trim** (50% of budget): Groups messages into atomic "turns" that cannot be split. Keeps minimum 6K tail tokens + 2 turns. Thread memory provides the summary — no LLM call needed.
2. **Local context reset** (60% threshold): Deterministic rebuild from system prompt + thread memory + recent tail. Instant, zero cost.
3. **LLM compaction fallback**: If local context reset fails, delegates to cloud-style summarization.

Common:
- Persistence: compacted messages flagged in DB, summary inserted as system message
- Check points: before first LLM call + after each tool result batch
- Graceful degradation: on failure, proceeds with full context

### Thread Memory (Local LLM Reliability)

Per-thread persistent key/value memory store for deterministic context rotation. Designed for local LLMs (20B params, 32K–131K context) that degrade in reliability as conversations grow.

- **Deterministic extraction**: After each tool call, `extract_from_tool_result()` persists compact findings (256 char cap) keyed by tool name (e.g., `finding:ghidra_analyze`). No LLM call required.
- **Memory injection**: Rendered as User message at `messages[1]` (after system prompt). Goal first, then alphabetical, total capped at 2KB.
- **Goal anchoring**: Local models get goal reminder appended to tool result nudge to prevent task drift.
- Enables 5-6+ message tool-calling conversations with a 20B parameter model without hallucination.

### Deterministic Artifact Scoping

Threads optionally target a specific sample via `target_artifact_id` (migration 033). When set:
- Context window contains ONLY the target sample + its generated children (DB-level filtering)
- Tool `artifact_id` auto-injection always uses the target — no heuristic guessing
- Generated artifact filenames prefixed with parent sample stem (e.g., `amixer_decompiled.json`)
- Parent-child linkage resolved via `tool_run_artifacts` FK chain

## Multi-Tenancy & Security

### Authentication
- API keys: `af_<32 random alphanumeric>`, stored as SHA-256 hashes
- Bearer token authentication on all API routes

### Authorization
- **RBAC**: owner > manager > collaborator > viewer
- **Project membership**: `project_members` table with role-based access
- **@all public access**: Sentinel UUID makes projects visible to all users
- **Admin-only**: Agent/workflow CRUD restricted to admin role

### Tenant Isolation
- **Application-level**: `require_project_access()` checks in every route handler
- **Postgres RLS**: Policies on all 12+ tenant-scoped tables, enforced via `SET LOCAL ROLE af_api`
- **Scoped transactions**: `begin_scoped(pool, user_id)` sets role + user context
- **Worker scoping**: Jobs execute with `actor_user_id` from the original request
- **Plugin table RLS**: Plugin schemas (e.g., `re.iocs`, `re.ghidra_function_renames`, `re.yara_rules`) have their own RLS policies
- **NDA flag**: Project-level `nda` column prevents cross-project data leakage via `af_shareable_projects()`. Audit trail on every NDA flag change (immutable `audit_log` entry with actor, old/new values). UI shows blueish tint and NDA badge when viewing NDA-covered projects

### Quotas & Rate Limiting
- Per-user daily LLM token limits
- Per-user concurrent analysis limits
- Postgres-backed API rate limiting (consistent across N server instances)
- Atomic storage quota enforcement

### Per-Tool Restriction Grants
- `restricted_tools` table: tool patterns that require explicit grants (e.g., `web.*`)
- `user_tool_grants` table: per-user grants matching tool patterns
- Pattern matching: exact (`web.fetch`), wildcard (`web.*`), universal (`*`)
- No restrictions for a tool = unrestricted (backward compatible)
- Fail-closed: if restriction DB load fails with a user_id, all restricted tools are blocked
- `web.*` tools seeded as restricted by default at bootstrap

### Per-User Model Access Control
- `user_allowed_routes` table: no rows = unrestricted, 1+ rows = allowlist mode
- Route formats: exact (`openai:gpt-4o-mini`), wildcard (`openai:*`), special (`auto`, `local`)
- Enforced in `AgentRuntime::check_route_access()` before route resolution
- `GET /llm/backends` filtered for restricted users
- Admin CRUD: `GET/POST/DELETE /api/v1/admin/users/{id}/routes`
- CLI: `af user routes <user_id> [--add route] [--remove route] [--clear]`

### Per-Plugin DB Isolation

Single-plugin deployments can use separate databases:
```bash
# Auto-derived: af --plugin foo → connects to af_foo DB
af --plugin foo serve

# Explicit:
AF_DATABASE_URL=postgres://af:af@localhost/my_plugin_db af serve
```

Server instance = isolation boundary. One `PgPool` per process. Multi-plugin instances share one DB.

## Configuration

Persistent configuration via `~/.af/config.toml` (or `AF_CONFIG_PATH` env var). Auto-created with all options commented out on first run (`0600` permissions on Unix).

```toml
[database]
# url = "postgres://af:af@localhost/af"

[storage]
# storage_root = "/tmp/af/storage"
# scratch_root = "/tmp/af/scratch"

[server]
# bind_addr = "127.0.0.1:8080"

[compaction]
# threshold = 0.85
# summarization_route = "openai:gpt-4o-mini"
```

Override hierarchy: environment variables > config.toml > compiled defaults.

## Deployment

```
Clients → Load Balancer → N × (af serve) → Postgres
                                    ↓
                          N × Worker threads (in-process)
                                    ↓
                          VT Gateway (embedded or standalone)

Remote CLI ···· HTTPS + Bearer ····→ Load Balancer (same path as web clients)
```

Key properties:
- **Horizontal scaling**: N server instances via `FOR UPDATE SKIP LOCKED` job claiming
- **Worker daemon**: `af worker start --concurrency N` for standalone worker processes
- **TLS**: `--tls-cert`/`--tls-key` or `AF_TLS_CERT`/`AF_TLS_KEY` env vars
- **CORS**: `AF_CORS_ORIGIN` env var (disabled by default)

## Web UI

Vanilla JS SPA (no build step) served from `ui/`:
- **Projects**: Create, list, manage members, upload artifacts. NDA badge on NDA-flagged projects
- **Conversations**: Send messages, view streaming responses, run workflows
- **Tools**: List with source labels, enable/disable
- **Agents**: CRUD with system prompt editor
- **Workflows**: List, validate, execute
- **Plugins**: Overview of loaded plugins with their tools/agents/workflows
- **Audit Log**: Immutable activity history
- **Themes**: Default, Lab, Print, and Dark themes (toggle from top bar)
- **NDA Visual Indicator**: Blueish background tint on all project-scoped views when working with NDA-covered projects, with theme-appropriate colors (light blue for light themes, dark blue-grey for dark theme)

## API

REST API at `/api/v1/` with SSE streaming:

```
POST   /api/v1/projects                    # create project
GET    /api/v1/projects                    # list projects
GET    /api/v1/projects/:id                # get project
POST   /api/v1/projects/:id/artifacts      # upload artifact
GET    /api/v1/projects/:id/artifacts      # list artifacts
GET    /api/v1/artifacts/:id/download      # download
POST   /api/v1/projects/:id/threads        # create thread
GET    /api/v1/projects/:id/threads        # list threads
POST   /api/v1/threads/:id/messages        # send message (SSE)
POST   /api/v1/threads/:id/messages/sync   # send message (blocking)
POST   /api/v1/threads/:id/messages/queue  # queue message (no LLM invocation)
GET    /api/v1/threads/:id/messages        # list messages
GET    /api/v1/threads/:id/export          # export conversation
POST   /api/v1/threads/:id/workflow        # execute workflow (SSE)
GET    /api/v1/threads/:id/children        # list child threads
POST   /api/v1/projects/:id/thinking       # start thinking thread (SSE)
GET    /api/v1/tools                       # list tools
GET    /api/v1/agents                      # list agents
POST   /api/v1/agents                      # create agent (admin)
GET    /api/v1/workflows                   # list workflows
POST   /api/v1/workflows                   # create workflow (admin)
GET    /api/v1/plugins                     # list loaded plugins
GET    /api/v1/llm/backends                # list LLM backends (filtered)
GET    /api/v1/projects/:id/members        # list project members
POST   /api/v1/projects/:id/members        # add member
DELETE /api/v1/projects/:id/members/:uid   # remove member
GET    /api/v1/quota                       # user quota + allowed routes
POST   /api/v1/admin/users                 # create user (admin)
GET    /api/v1/admin/users                 # list users (admin)
POST   /api/v1/admin/api_keys             # create API key (admin)
GET    /api/v1/admin/users/:id/routes      # list user's allowed routes
POST   /api/v1/admin/users/:id/routes      # add route to user
DELETE /api/v1/admin/users/:id/routes      # remove route from user
GET    /api/v1/audit                       # audit log (admin)
GET    /api/v1/web-rules                   # list URL rules
POST   /api/v1/web-rules                   # add URL rule
DELETE /api/v1/web-rules/:id               # remove URL rule (admin)
GET    /api/v1/web-rules/countries         # list blocked countries
POST   /api/v1/web-rules/countries         # block country (admin)
DELETE /api/v1/web-rules/countries/:code   # unblock country (admin)
GET    /api/v1/admin/restricted-tools      # list restricted patterns
POST   /api/v1/admin/restricted-tools      # add restricted pattern
DELETE /api/v1/admin/restricted-tools      # remove restricted pattern
GET    /api/v1/admin/users/:id/tool-grants # list user's tool grants
POST   /api/v1/admin/users/:id/tool-grants # grant tool access
DELETE /api/v1/admin/users/:id/tool-grants # revoke tool grant
GET    /api/v1/health                      # health check (no auth)
```

## Build

```bash
cargo build --release    # produces: af, af, af-builtin-executor, af-executor
cargo test --workspace   # 303 tests
make help                # show all targets
```

## Status

All 16 phases complete + web gateway + security hardening + NDA audit-grade hardening. 0 errors, 0 warnings, 303 tests pass.
