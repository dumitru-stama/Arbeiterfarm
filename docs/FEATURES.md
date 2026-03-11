# Arbeiterfarm & Reverse-Arbeiterfarm: Project Features

## Executive Summary

**Arbeiterfarm** is a Rust-based SDK for building multi-agent AI workstations. It provides the infrastructure for running specialized AI agents that collaborate on analysis tasks using domain-specific tools — with full audit trails, sandbox isolation, and enterprise-grade multi-tenancy.

**Reverse-Arbeiterfarm** is the first distribution built on Arbeiterfarm: an AI-powered reverse engineering workstation where agents analyze malware, decompile code, query threat intelligence, perform dynamic analysis in sandboxed VMs, write YARA rules, and produce structured reports.

The key idea: **the AI doesn't just talk about analyzing a binary — it actually runs Ghidra, reads disassembly, queries VirusTotal, executes samples in a sandboxed VM, and produces verifiable results traceable back to raw tool output.**

---

## Architecture at a Glance

```
┌────────────────────────────────────────────────────────────────┐
│                    Distribution Binary                          │
│  af (compiled RE plugin)  or  af (TOML plugins only)   │
├────────────┬──────────┬───────────┬──────────┬─────────────────┤
│  af-api  │af-agents│ af-jobs │ af-llm │   af-auth    │
│  HTTP API  │  agent    │ job queue │   LLM    │  API key auth  │
│  routes    │  runtime  │  worker   │  router  │  RBAC          │
│  SSE       │  orchestr.│  OOP exec │ redaction│  project access│
├──────┬─────┴──────────┼──────────┬────────────┴─────────────────┤
│ af-│ af-email     │af-     │         af-core             │
│  web │  7 email tools │ notify   │ Types, traits, registries     │
│  gw  │  Gmail+Proton  │ 4 tools  │ (ToolSpec, ToolExecutor)      │
├──────┼────────────────┼────────────────────────────────────────┤
│   af-db             │          af-storage                   │
│   PostgreSQL          │  Content-addressed blobs (SHA256)       │
│   37 migrations       │  Scratch directories                    │
└───────────────────────┴────────────────────────────────────────┘
```

- **Single Cargo workspace** — ~16 crates, 4 binaries, 286+ tests, 0 warnings
- **Written entirely in Rust** (except the VM guest agent in Python + Frida JavaScript hooks)
- **Web UI**: Vanilla JavaScript SPA — zero build step, zero dependencies, no React/Vue/npm

---

## Three Collaboration Modes

### 1. Single-Agent Chat

Direct Q&A with a specialist agent. The agent uses tools in a loop (up to 20 calls), analyzing results and deciding next steps.

```bash
af chat --agent decompiler --project p1
> "Decompile the main function and explain what it does"
```

### 2. Multi-Agent Workflows

Predefined pipelines where groups of agents execute in sequence, sharing one conversation thread:

```
Group 1 (parallel):  [surface] + [intel]    → triage + threat intel
Group 2 (sequential): [decompiler]           → deep code analysis
Group 3 (sequential): [reporter]             → synthesize report
```

All agents read the same thread — the decompiler sees what the surface analyst found, and the reporter sees everything. No manual copy-paste between sessions.

**Dynamic signals**: Agents can influence the remaining pipeline by emitting signal markers (`signal:request:agent`, `signal:skip:agent`, `signal:priority:agent`).

**Fan-out**: When a tool extracts files from a container (zip, archive), the orchestrator automatically creates child threads, one per extracted file, and runs the full workflow on each. Parent waits for all children. Safety: max depth 3, max 50 children, SHA256 cycle detection.

**Repivot**: When a tool produces a replacement artifact (e.g., UPX unpacker), eligible agents re-run with the updated binary.

### 3. Thinking Threads (Autonomous Orchestration)

An AI research director that decides at runtime which specialists to invoke:

```bash
af think --project p1 --goal "Determine if this sample is APT29-related"
```

The thinker agent has 5 meta-tools:
- `meta.invoke_agent` — spawn a specialist on a child thread
- `meta.read_thread` — read results from child threads
- `meta.list_agents` — discover available specialists
- `meta.list_artifacts` / `meta.read_artifact` — browse project content

Anti-recursion ensures child agents cannot spawn further children. Budget: 30 tool calls. Timeout cascade: child (5 min) < meta-tool (11 min) < thinker (30 min).

---

## Tool Inventory

### File Analysis Tools (Built-in, Domain-Agnostic)

| Tool | Description | Output |
|------|-------------|--------|
| `file.info` | Size, MD5/SHA256, magic byte detection (ELF, PE, PNG, ZIP, PDF, Mach-O...) | Inline |
| `file.read_range` | Read byte ranges or line ranges from any artifact | Inline |
| `file.strings` | Extract printable strings with configurable encoding/length | Artifact + summary |
| `file.hexdump` | Hex + ASCII dump at any offset | Inline |
| `file.grep` | Regex search with context lines, 2M line cap | Artifact + summary |

### Binary Analysis Tools (Rizin)

| Tool | Description | Output |
|------|-------------|--------|
| `rizin.bininfo` | Architecture, security flags (NX/PIE/canary), imports, exports, sections | Artifact + summary |
| `rizin.disasm` | Disassemble N instructions at hex address | Artifact + summary |
| `rizin.xrefs` | Cross-references to/from any address | Artifact + summary |
| `strings.extract` | Multi-encoding string extraction (ASCII, UTF-8, UTF-16LE/BE) | Artifact + summary |

### Decompilation Tools (Ghidra)

| Tool | Description | Output |
|------|-------------|--------|
| `ghidra.analyze` | Headless Ghidra analysis, cached per SHA256 (30-240s first run, ~2s on cache hit) | Artifact + summary |
| `ghidra.decompile` | Decompile 1-20 functions by name or address to C pseudocode | Artifact + summary |
| `ghidra.rename` | Rename functions in DB overlay (max 50 pairs per call, applied during decompilation) | Inline |
| `ghidra.suggest_renames` | Surface function renames from other non-NDA projects for the same binary | Inline |

### Threat Intelligence Tools

| Tool | Description | Output |
|------|-------------|--------|
| `vt.file_report` | VirusTotal hash lookup with rate limiting and 24h DB cache | Inline |
| `re-ioc.list` | List extracted IOCs (IPs, domains, URLs, hashes, emails, mutexes, registry keys) | Inline |
| `re-ioc.pivot` | Pivot on an IOC across all project artifacts and tool runs | Inline |
| `re-ioc.search` | Cross-project IOC search (NDA-excluded) | Inline |

### Malware Family Tracking

| Tool | Description |
|------|-------------|
| `family.tag` | Tag artifact with malware family name + confidence (low/medium/high/confirmed) |
| `family.list` | List families in current project |
| `family.search` | Cross-project family search (respects NDA) |
| `family.untag` | Remove misattribution |

### YARA Rule Management

| Tool | Description |
|------|-------------|
| `yara.scan` | Scan artifact against YARA rules, returns matches with offsets |
| `yara.generate` | Validate YARA syntax and persist rule to DB |
| `yara.test` | Test rule against project/artifact/MIME-type scope, returns match matrix |
| `yara.list` | List rules from filesystem, DB, and artifact sources |

### Dynamic Analysis (Sandbox)

| Tool | Description | Output |
|------|-------------|--------|
| `sandbox.trace` | Execute PE in QEMU/KVM VM with ~60 Frida API hooks | Artifact + summary |
| `sandbox.hook` | Execute PE with custom Frida JavaScript hooks | Artifact + summary |
| `sandbox.screenshot` | QMP screendump of VM display | Inline (base64) |

**Default hooks cover 10 categories (~60 Windows APIs)**:

| Category | Key APIs |
|----------|----------|
| File Operations | CreateFileW/A, ReadFile, WriteFile, DeleteFileW/A, CopyFileW/A |
| Registry | RegOpenKeyExW, RegSetValueExW, RegQueryValueExW, RegCreateKeyExW |
| Process Injection | CreateProcessW/A, VirtualAllocEx, WriteProcessMemory, CreateRemoteThread |
| Network | connect, send/recv, InternetConnectW, HttpSendRequestW, DnsQuery_W |
| Library Loading | LoadLibraryW/A, GetProcAddress, LdrLoadDll |
| Crypto | CryptEncrypt/Decrypt, BCryptEncrypt/Decrypt |
| Services | CreateServiceW, StartServiceW, ChangeServiceConfigW |
| COM | CoCreateInstance, CoGetClassObject |
| Memory | VirtualAlloc, VirtualProtect, NtAllocateVirtualMemory |
| Anti-Debug | IsDebuggerPresent, CheckRemoteDebuggerPresent, GetTickCount |

Each trace entry records: timestamp, API name, parsed arguments (strings truncated at 256 chars), return value, thread ID, 3-frame backtrace. Max 10,000 entries per run. VM is snapshot-restored before every execution.

### Web Tools

| Tool | Description |
|------|-------------|
| `web.fetch` | URL to plaintext with SSRF protection, DNS pinning, redirect re-validation, caching |
| `web.search` | DuckDuckGo search with structured results |

### Email Tools

| Tool | Description |
|------|-------------|
| `email.send` | Send email (supports dry_run validation) |
| `email.draft` | Create draft in provider's drafts folder |
| `email.schedule` | Schedule future send (processed by `af tick`) |
| `email.list_inbox` | List recent emails with summaries |
| `email.read` | Full message body + attachment metadata |
| `email.reply` | Reply in-thread with proper headers |
| `email.search` | Search emails by query |

Providers: **Gmail** (REST API, OAuth2 refresh) and **ProtonMail Bridge** (SMTP via lettre).

### Notification Tools

| Tool | Description |
|------|-------------|
| `notify.send` | Send notification to a named channel (webhook, matrix, webdav) |
| `notify.upload` | Upload artifact to a WebDAV channel |
| `notify.list` | List notification channels for the current project |
| `notify.test` | Send test notification to verify channel configuration |

**4 channel types**: Webhook (POST/PUT to HTTPS with custom headers), Email (placeholder — use webhook bridge), Matrix (idempotent PUT via txn_id), WebDAV (artifact blob or text file upload).

**Delivery**: PostgreSQL queue with `pg_notify()` trigger → PgListener in `af serve` for near-real-time delivery. `af tick` as fallback processor (batch of 20). Stale recovery (2 min), retry up to 5 attempts, permanent error detection for config-level failures.

### Embedding / Vector Search

| Tool | Description |
|------|-------------|
| `embed.text` | Generate and store vector embedding for text |
| `embed.search` | Cosine similarity search over stored embeddings (pgvector HNSW) |
| `embed.batch` | Batch-embed up to 100 texts |
| `embed.list` | List stored embeddings |

### URL Ingestion (RAG Knowledge Base)

| Tool/Feature | Description |
|------|-------------|
| URL Import | Managers paste URLs → system fetches, HTML→text, chunks, auto-enqueues for embedding |
| `af tick` processing | Up to 5 URLs per cycle: HTTPS fetch (5MB limit, 30s timeout) → html2text → artifact → chunk → embed_queue |
| Embed Queue | Background embedding of chunks.json artifacts, batched (100/batch), resumable, retry on failure |

### Cross-Project Analysis

| Tool | Description |
|------|-------------|
| `dedup.prior_analysis` | Surface prior analysis of same binary by SHA256 across non-NDA projects |
| `artifact.describe` | Set human-readable annotation on any artifact |
| `artifact.search` | Cross-project search by filename, description, SHA256, MIME type |

---

## Built-in Agents

| Agent | Role | Route | Key Tools |
|-------|------|-------|-----------|
| **surface** | Binary triage analyst | auto | rizin.bininfo, file.strings, ghidra.analyze, family.tag, yara.scan |
| **decompiler** | Code reverse engineer | auto | ghidra.analyze/decompile/rename, rizin.disasm/xrefs |
| **asm** | Assembly specialist | local | rizin.disasm, rizin.xrefs only |
| **intel** | Threat intelligence analyst | auto | vt.file_report, re-ioc.list/pivot, family.tag/search, yara.scan |
| **reporter** | Technical report writer | auto | re-ioc.list/pivot, artifact.describe/search, family.list/search |
| **tracer** | Dynamic analysis specialist | auto | sandbox.trace/hook/screenshot, file.*, family.tag |
| **yara-writer** | YARA rule expert | auto | yara.*, file.*, ghidra.analyze/decompile, strings.extract |
| **researcher** | Web research / OSINT | auto | web.fetch, web.search |
| **email-composer** | Email operations | auto | email.* (prefers drafts before sending) |
| **notifier** | Notification delivery | auto | notify.* (list channels, send, upload, test) |
| **thinker** | Autonomous orchestrator | auto | meta.* (internal.*) |

Each agent has a carefully crafted system prompt encoding its methodology, anti-hallucination instructions, and evidence-citation requirements.

---

## LLM Backend Support

### Three Provider Backends

| Provider | Config | Default Model |
|----------|--------|---------------|
| **OpenAI-compatible** | `AF_OPENAI_ENDPOINT` | gpt-4o |
| **Anthropic** | `AF_ANTHROPIC_API_KEY` | claude-sonnet-4-20250514 |
| **Vertex AI** | `AF_VERTEX_ENDPOINT` | -- |

### Multi-Model Routing

- `auto` — use the best available backend
- `local` — force local model (for classified/air-gapped work)
- `openai:gpt-4o-mini` — specific backend and model
- Per-request route overrides propagate through workflows and fan-out

### Static Model Catalog (~20 models)

GPT-4o/4.1/5, Claude Opus/Sonnet 4.6, Haiku 4.5, Gemini 2.x/3.x, o3/o4-mini — with context windows, output limits, costs, vision support, and knowledge cutoffs.

### Local Model Cards (~57 TOML files)

Custom model specs in `~/.af/models/*.toml` for local deployments (Ollama, vLLM). Override the static catalog at startup. Includes: gpt-oss, Qwen3, DeepSeek-R1, Llama 3/4, Mistral, Gemma, Phi, and more.

### Dual Tool-Calling Modes

- **Mode A** (text-based): For local models without native function calling. Tool schemas injected as JSON blocks in the system prompt. Model outputs `<tool_call>` JSON.
- **Mode B** (native API): For cloud LLMs. Tool descriptions sent via the API's native tool-calling mechanism.

Auto-selected based on `BackendCapabilities.supports_tool_calls`.

### Vision / Multi-Modal

`ContentPart` enum supports Text and Image. Backend-specific serialization: OpenAI image_url, Anthropic base64, Vertex inlineData.

---

## Local LLM Reliability Engineering

14 specific changes to make 20B-parameter local models work reliably as tool-calling agents.

### Phase 1 — Tool Calling Mechanics (Changes 1-8)

1. **Full schemas always sent** — removed "compact mode" that sent empty `{"type": "object"}`; `$ref` pointers resolved inline for models that don't understand JSON Schema indirection
2. **Stronger system prompt** — local: "You MUST use your tools. Do NOT fabricate"; cloud: gentler phrasing
3. **Lower temperature** — local: 0.1 (JSON precision), cloud: 0.3
4. **Malformed JSON repair loop** — returns error with parse message + raw input + retry instruction instead of silently substituting `{}`
5. **Continuation nudge** — appended to tool result content (not a separate message) to prevent "Understood, I'll proceed" hallucination
6. **Argument fixup** — automatic singular-to-plural, string-to-array, integer-to-string (hex address formatting), float truncation
7. **Minimum-constraint fixup** — optional fields with violated minimums stripped; required fields clamped
8. **Better validation errors** — JSON pointer paths in error messages: `'line_count': 0 is less than the minimum of 1`

### Phase 2 — Multi-Turn Reliability (Changes 9-14)

9. **Thread memory** — per-thread key/value store in DB. Deterministic extraction after every tool call (no LLM needed). Injected at `messages[1]` before every LLM request. Capped at 2KB.
10. **Goal anchoring** — user's goal appended to every tool result nudge for local models to prevent task drift
11. **Artifact-first tool output** — all large results stored as artifacts with compact inline summaries (24KB grep output → 200 bytes)
12. **Token-budget sliding window** — atomic turns (user + tool calls + results), walks back-to-front keeping 6K tail tokens + 2 turns. Zero LLM cost.
13. **Local context reset** — deterministic rebuild from system prompt + thread memory + tail at 60% threshold. Instant, zero cost.
14. **Deterministic artifact scoping** — `target_artifact_id` on threads eliminates heuristic guessing

**Result**: A 20B local model can maintain coherent 5-6+ message tool-calling conversations without hallucination.

---

## Context Management (Two-Tier)

### Cloud Models (Claude, GPT-4o, Gemini)

- Trigger at 85% of context window
- LLM summarizes middle messages, preserves head + tail
- Compaction redaction: conversation text sanitized before sending to non-local backends
- Optional cheaper summarization route: `[compaction] summarization_route = "openai:gpt-4o-mini"`

### Local Models — Three-Layer Defense (Zero LLM Cost)

**Token budget math** (gpt-oss 131K context / 16K output):
```
budget = 131072 * 0.60 - 16384 = 62,259 tokens
sliding_window_trigger = 50% of budget = ~31,130 tokens
local_reset_trigger   = 60% of budget = ~37,355 tokens
```

1. **Sliding Window** (50%) — groups messages into atomic turns, trims oldest, thread memory provides the summary
2. **Local Context Reset** (60%) — marks all body messages compacted, rebuilds: system + memory + tail
3. **LLM Fallback** — cloud-style summarization as last resort

**Pre-flight invariant check** (local models only): validates system prompt at [0], memory at [1], at least 1 real user message, no orphaned tool messages. Auto-repairs.

---

## Artifact-First Tool Output

Large tool results are stored as project artifacts with compact summaries inline. The model uses `file.read_range` or `file.grep` to inspect details on demand.

| Tool | Artifact | Inline Summary |
|------|----------|----------------|
| `rizin.bininfo` | `bininfo.json` | Architecture, security flags, import/export/section counts |
| `rizin.disasm` | `disasm.json` | Address, instruction count, first 10 instructions preview |
| `ghidra.analyze` | `functions.json` | Program info, function index (name + address), thunk list |
| `ghidra.decompile` | `decompiled.json` | Function names, addresses, line counts per function |
| `file.grep` | `grep_results.json` | Match count, top 5 matches with context |
| `file.strings` | `strings.json` | Total count, top 20 strings |
| `sandbox.trace` | `trace.json` | API call count, unique APIs, top 20 by frequency, process tree |

**Context reduction**: A `file.grep` result goes from 24KB inline to ~200 bytes summary.

**Filename prefixing**: Generated artifacts are prefixed with the parent sample stem: `amixer_decompiled.json` instead of generic `decompiled.json`. Permanent, distinguishable across samples.

---

## Security Model

### Bubblewrap Sandbox (Zero Trust for Tools)

All non-trusted tools run in bwrap sandboxes:

```
bwrap
  --tmpfs /                    # empty root filesystem
  --ro-bind /usr /usr          # system libraries (read-only)
  --ro-bind <artifact> <path>  # only the specific input artifact
  --bind <scratch> <scratch>   # writable scratch directory
  --unshare-all                # PID/IPC/UTS/net/cgroup isolation
  --cap-drop ALL               # no Linux capabilities
  --die-with-parent            # cleanup on crash
  --clearenv                   # no API key leakage
```

**Fail-closed**: if bwrap can't initialize, the tool doesn't run. No fallback to unsandboxed execution. Override only via `AF_ALLOW_UNSANDBOXED=1` (dev/testing).

**Sandbox profiles**:

| Profile | Used By | Isolation Level |
|---------|---------|-----------------|
| `NoNetReadOnly` | Rizin, strings, file tools | Full namespace isolation, no network |
| `PrivateLoopback` | Ghidra tools | No namespace unsharing (JVM needs loopback) |
| `NetEgressAllowlist` | Tools needing specific UDS endpoints | Filtered network |
| `Trusted` | Email, web gateway, IOC tools | In-process, no bwrap |

### Multi-Tenant Isolation

- **Application-level RBAC**: owner > manager > collaborator > viewer
- **PostgreSQL Row-Level Security**: 12+ tenant-scoped tables with RLS policies
- **Scoped transactions**: every DB operation sets `af.current_user_id` via `begin_scoped()`
- **Worker scoping**: jobs execute with `actor_user_id` from the original request
- **Per-plugin schema RLS**: plugin tables (e.g., `re.ioc_findings`) have their own policies
- **NDA projects**: excluded from cross-project queries (family search, dedup, IOC pivot, Ghidra rename suggestions). NDA flag changes are audited in the immutable audit log with actor identity, old/new values, and timestamp. RLS defense-in-depth on `re.ghidra_function_renames` and `re.yara_rules` (migration 007)
- **@all sentinel**: public access for open projects

### Per-User Access Controls

- **Model access**: `user_allowed_routes` table — wildcard prefix matching (e.g., `openai:*`), enforced before every LLM call
- **Tool restrictions**: `restricted_tools` + `user_tool_grants` tables — `web.*` and `email.*` restricted by default, require admin grant
- **Rate limiting**: per-API-key (60 req/min default), per-user email rate limits (global + per-user token-bucket)

### Web Gateway Security (7 Layers)

1. **SSRF protection** — blocks private/reserved IPs (IPv4 + IPv6 including 6to4, Teredo, NAT64 embedding formats)
2. **GeoIP filtering** — optional MaxMind country blocking
3. **URL rules** — allow/block lists (domain, domain_suffix, url_prefix, url_regex, ip_cidr), block-wins
4. **DNS pinning** — validated IPs pinned in HTTP client to prevent rebinding
5. **Redirect re-validation** — full security checks on every redirect hop
6. **Rate limiting** — global and per-user token-bucket
7. **Response caching** — TTL-based DB cache

### Email Security

- Recipient allowlist/blocklist (exact_email, domain, domain_suffix), block-wins, fail-closed
- RFC 2822 header injection prevention (CRLF sanitization)
- Credentials never in tool output or logs
- Gmail message ID sanitization (alphanumeric only)
- Restricted by default — requires admin grant

### Chain of Custody

- Artifacts are content-addressed (SHA256) — immutable, tamper-evident
- Tool outputs linked to `tool_run_id` records with full input/output capture
- Evidence citations (`evidence:tool_run:<id>`, `evidence:artifact:<id>`) verified against DB and project scope
- Agent messages tagged with `agent_name` for attribution
- Audit log is **append-only** — database trigger rejects UPDATE and DELETE

### PII Redaction

`RedactionLayer` scrubs sensitive data before sending to cloud LLMs. Disabled for local models. Compaction redaction for non-local summarization backends.

---

## Deterministic Artifact Scoping

When analyzing a specific sample, everything is scoped to that sample:

- **Context**: Only that sample's artifacts enter the LLM context (not every artifact in the project)
- **Tool calls**: `artifact_id` is always the target — no heuristic guessing
- **Generated output**: Filenames prefixed with sample stem (`amixer_decompiled.json`)
- **Parent tracking**: Every generated artifact linked to its parent through the tool run chain

Resolution chain: `generated artifact → source_tool_run_id → tool_run_artifacts(role='input') → parent uploaded sample`

---

## Plugin System

### Compiled Rust Plugins

Implement the `Plugin` trait: custom tool executors, database schemas, post-tool hooks, evidence resolvers, agent presets, workflows. Full framework access.

### TOML Plugins (No Recompile)

Drop-in tools, agents, workflows, and model cards in `~/.af/`:

```toml
# ~/.af/agents/ransomware-analyst.toml
name = "ransomware-analyst"
route = "auto"
tools = ["file.*", "rizin.*", "ghidra.*", "yara.*"]
timeout_secs = 300

[prompt]
text = """You are a ransomware specialist. Focus on encryption routines,
key derivation, and ransom note generation patterns."""
```

### Two Protocol Modes for External Tools

- **Simple**: flat JSON stdin/stdout
- **OOP**: `OopEnvelope`/`OopResponse` with produced files support and context injection

### Post-Tool Hooks

Automatic processing after every tool execution:
- **IOC extraction**: regex-extracts IPs, domains, hashes from all tool output, upserts to DB
- **YARA persistence**: auto-saves generated rules and scan results to DB

### Source Tracking

Every tool, agent, and workflow tagged with its origin: `builtin`, `re` (compiled plugin), `local` (TOML), `user` (API-created).

---

## Data Model

### 36 PostgreSQL Migrations

Progressive schema evolution from core entities to full feature set:

| Feature | Key Tables |
|---------|------------|
| Core | projects, blobs, artifacts, tool_runs, tool_run_artifacts, threads, messages |
| Auth | users, api_keys, project_members, audit_log |
| Agents | agents, workflows (JSONB steps) |
| Lineage | parent_thread_id, target_artifact_id |
| Intelligence | re.ioc_findings, re.family_tags, re.yara_rules, re.yara_scan_results, re.ghidra_function_renames |
| Web | web_url_rules, web_fetch_cache |
| Email | email_credentials, email_recipient_rules, email_tone_presets, email_scheduled, email_log |
| Access Control | user_allowed_routes, restricted_tools, user_tool_grants |
| Context | thread_memory, compacted flag on messages |
| Embeddings | embeddings (pgvector HNSW indexes) |
| Knowledge/RAG | url_ingest_queue, embed_queue |
| Notifications | notification_channels, notification_queue (pg_notify trigger) |
| Usage | llm_usage_log (per-request token tracking) |

**Design decisions**: runtime `sqlx::query_as()` (no live DB at build time), JSONB for extensibility (settings, metadata, tool configs), `FOR UPDATE SKIP LOCKED` for the job queue.

### Content-Addressed Storage

Blobs stored at `{storage_root}/data/{xx}/{yy}/{sha256}`. Two-level directory sharding. Atomic writes (temp → rename). Deduplication across projects. Per-tool-run scratch directories with automatic cleanup.

---

## Web UI

Vanilla JavaScript SPA — **zero build step, zero npm, no framework**.

### Features

- **Dashboard** — system health, recent conversations, recent artifacts, quick links
- **Projects** — create, list, manage members, upload artifacts
- **Conversations** — send messages with real-time SSE streaming, per-message agent and route selection
- **Workflow builder** — group-aware step editor with visual columns
- **Thinking threads** — goal-driven autonomous analysis
- **Agent CRUD** — create/edit with system prompt editor and tool selector
- **Tool browser** — list all tools by source plugin, interactive "Run Tool" form
- **Plugins** — inventory of loaded plugins with their tools/agents/workflows
- **Audit log** — immutable activity history
- **Cost tracker** — live per-session LLM cost in USD, breakdown by provider and model
- **Four themes** — Default, "Lab", "Print" (light/readable), and "Dark" (dark grey with teal accent)
- **Keyboard shortcuts** — `/` to focus, `Escape` to abort stream, `Enter` to send
- **Export** — Markdown and JSON download
- **Knowledge** — URL import form + embed queue management (combined RAG admin page)
- **YARA Management** — dedicated YARA rules listing, rule source viewer, scan results lookup by artifact
- **Web Rules** — URL allow/block rules + GeoIP country blocks management
- **Email Admin** — credentials, tone presets, recipient rules, scheduled emails management
- **Notifications** — channel management (add/test/delete) with type-specific config, notification queue with cancel/retry
- **NDA Visual Indicator** — blueish background tint on all project-scoped views when working with NDA-covered projects (theme-appropriate: light blue for light themes, dark blue-grey for dark theme). NDA badge on project listings and project detail headers

### Conversation View

- Three view modes: Full (grouped with markdown rendering), Timeline (compact chronological)
- Tool calls as collapsible `<details>` blocks
- Expandable reasoning/thinking blocks
- Real-time streaming with 80ms throttled rendering
- Message queue: messages sent during streaming shown as pending bubbles
- System prompt preview with per-message override
- Child thread panel for fan-out/thinking threads
- Context usage annotation per assistant turn

---

## HTTP API

REST API at `/api/v1/` with SSE streaming. ~55 endpoints covering:

- **Projects**: CRUD, settings, cost tracking, NDA flag (with audit trail on toggle)
- **Artifacts**: upload/download, metadata, generated cleanup
- **Threads**: create with optional target artifact, export (markdown/JSON)
- **Messages**: send (SSE streaming), sync (blocking), queue (no LLM)
- **Workflows**: CRUD, execute on thread (SSE)
- **Thinking**: start autonomous analysis (SSE)
- **Tools**: list, run interactively
- **Agents**: CRUD with system prompt + tool allowlist
- **Hooks**: project-scoped event triggers
- **Admin**: user management, API keys, quotas, route allowlists, tool grants/restrictions
- **Web rules**: URL allow/block lists, country blocking
- **Email rules**: recipient allow/block lists
- **Audit**: immutable activity log
- **URL Ingest**: submit URLs for RAG, list/cancel/retry queue items (project-scoped)
- **Embed Queue**: admin list/cancel/retry embed queue items
- **Notifications**: create/list/update/delete channels (Manager+), test channels, list/cancel/retry queue (project-scoped)
- **Health**: `GET /health` (no auth)

### Infrastructure

- Per-user SSE stream limiter with RAII guards
- Rate limiting middleware on all API routes
- CORS: opt-in via `AF_CORS_ORIGIN`
- TLS support: `--tls-cert`/`--tls-key`
- Upload size limit: configurable (default 100MB)

---

## CLI

Comprehensive command-line interface with ~35 commands:

```bash
# Core operations
af project create/list/delete [--nda]
af project nda <id> --on|--off
af project settings <id> [--set key=value]
af artifact add/list/info/delete
af chat --project <id> [--agent <name>] [--workflow <name>]
af think --project <id> --goal "..."
af conversation list/show/export

# Tool management
af tool list/run/enable/disable/reload

# Agent management
af agent list/show/create/delete/promote

# Administration
af user create/list
af user api-key create/list/revoke
af user routes <user-id> [--add] [--remove]
af grant tool/revoke/list/restrict/unrestrict

# Web & email rules
af web-rule add/remove/list/block-country/unblock-country
af email setup/accounts/tones/scheduled/cancel
af email-rule add/remove/list

# Knowledge / RAG
af url-ingest submit/list/cancel/retry
af embed-queue list/cancel/retry

# Notifications
af notify channel add/list/remove/test
af notify queue list/cancel/retry

# Ghidra renames
af ghidra-renames list/suggest/import

# Operations
af serve [--bind] [--tls-cert] [--tls-key]
af worker start [--concurrency N]
af tick                    # fire all due tick hooks
af audit list [--limit] [--type]
```

### Remote CLI Mode

```bash
export AF_REMOTE_URL=https://af.example.com
export AF_API_KEY=af_xxxx
af project list    # operates against remote server
```

HTTPS enforced by default. `Backend` trait abstracts DirectDb (local PgPool) vs RemoteApi (reqwest + Bearer auth).

---

## Job System

### PostgreSQL-Backed Job Queue

- **Claim**: `FOR UPDATE SKIP LOCKED` — safe multi-worker claiming, no thundering herd
- **Heartbeat**: 30s interval, extends 120s lease. On heartbeat failure, execution aborted via `tokio::sync::Notify`
- **Reaper**: background task every 30s — reclaims expired jobs (up to 3 retries), permanently fails exhausted ones
- **Worker daemon**: N concurrent worker tasks + 1 reaper, `tokio::sync::watch` for shutdown

### OOP Executor

Out-of-process tool execution with handshake protocol:

1. **Handshake**: executor reports supported tools and protocol version
2. **Execution**: `OopEnvelope` (tool name, input, context with artifacts and scratch dir) → binary → `OopResponse` (output, produced files)
3. **Ingestion**: produced files validated (path traversal check), content-addressed stored, linked to tool run

Real-time stderr streaming: `mpsc::channel(64)` pipes executor stderr to `tool_run_events` DB table as events arrive.

---

## Hooks System

Auto-trigger analysis on events:

```bash
# Auto-triage every uploaded artifact
af hook create \
  --project <id> \
  --event artifact_uploaded \
  --workflow auto-triage \
  --prompt "Analyze artifact {{artifact_id}} ({{filename}}, sha256:{{sha256}})"

# Periodic re-check via cron + `af tick`
af hook create \
  --event tick \
  --agent intel \
  --interval 1440 \
  --prompt "Re-check VirusTotal for artifacts not scanned in 24 hours."
```

Template variables: `{{artifact_id}}`, `{{filename}}`, `{{sha256}}`, `{{project_id}}`.

---

## Deployment

```
Clients → Load Balancer → N × (af serve) → PostgreSQL
                                    ↓
                          N × Worker threads (in-process)
                                    ↓
                          VT Gateway + Sandbox Gateway + Web Gateway
                                    (embedded or standalone)

Remote CLI ·· HTTPS + Bearer ··→ Load Balancer
```

- Horizontal scaling via `FOR UPDATE SKIP LOCKED` job claiming
- Standalone workers: `af worker start --concurrency N`
- TLS: built-in via `--tls-cert`/`--tls-key`
- Config: `~/.af/config.toml` with env var overrides

---

## Privacy & Air-Gap Readiness

- Local LLM backends (Ollama, vLLM) work with **zero internet connectivity**
- PII redaction layer scrubs data before cloud LLM calls
- Agents can be forced to `--route local` to guarantee no data leaves the network
- Sandbox blocks network access by default — tools cannot phone home
- Per-user model restrictions prevent unauthorized cloud model usage

---

## Build & Test

```bash
cargo build --release            # 4 binaries: af, af, af-builtin-executor, af-executor
cargo test --workspace           # 286+ tests
make setup-db                    # create af user + database (idempotent)
make setup-bwrap                 # install bubblewrap sandbox
make test-e2e                    # end-to-end tests (needs DB)
make serve                       # start API server
make serve-local OLLAMA_MODEL=gpt-oss  # start with Ollama
```

---

## Threat Model (STRIDE)

A comprehensive V4 threat model covers all attack surfaces with Mermaid data flow diagrams:

- **Spoofing**: API key brute force, stolen keys, TOML plugin injection
- **Tampering**: tool output manipulation, prompt injection via tool results, plugin supply chain
- **Repudiation**: append-only audit log, tool run provenance chain
- **Information Disclosure**: tenant isolation (RLS on all tables including `re.ghidra_function_renames` and `re.yara_rules`), PII redaction, NDA project exclusion with audited flag changes, compaction redaction
- **Denial of Service**: rate limiting, job timeouts, context budget limits, sandbox resource caps
- **Elevation of Privilege**: RBAC enforcement, tool restriction grants, sandbox escape prevention

---

## Agent Roadmap (Future)

Planned agent families ordered by value-to-effort ratio:

| Priority | Agent | Status | Description |
|----------|-------|--------|-------------|
| 1 | Email | **Implemented** | 7 tools, Gmail + ProtonMail |
| 2 | Document/RAG | **Partially Implemented** | URL ingest + embed queue + shared chunking |
| 3 | YARA | **Implemented** | 4 tools, rule generation + testing |
| 4 | Data Transform | **Implemented** | jq, CSV, base64/hex/XOR decode |
| 5 | Sandbox (local) | **Implemented** | Frida + QEMU/KVM, 60 API hooks |
| 6 | Notification | **Implemented** | 4 tools, webhook/matrix/webdav channels, PgListener delivery |
| 7 | MITRE ATT&CK | Planned | Local STIX/JSON lookup + Navigator layers |
| 8 | Sandbox Submission | Planned | Any.Run, Joe Sandbox, CAPE |
| 9 | Debugging | Planned | x64dbg in microVM — highest-value RE feature |
| 10 | Source Code Audit | Planned | ripgrep/tree-sitter based |
| 11 | Container/Image | Planned | OCI inspection + SBOM + vuln scan |
| 12 | Database Query | Planned | Read-only SQL via UDS gateway |
| 13 | SIEM/Log Query | Planned | Splunk/Elastic integration |

All new agents follow established patterns: UDS gateways for external services, bwrap sandbox for file processing, restricted by default for sensitive operations.

---

## Key Numbers

| Metric | Value |
|--------|-------|
| Rust crates | ~16 |
| Tests | 303+ |
| DB migrations | 36 |
| Built-in tools | 39+ |
| Built-in agents | 11 |
| Supported LLM models | ~20 (static) + 57 (local TOML) |
| Default Frida API hooks | ~60 (10 categories) |
| API endpoints | ~63 |
| CLI commands | ~42 |
| Lines of Rust (approx) | 26,000+ |
| Build warnings | 0 |

---

## Documentation

| Document | Description |
|----------|-------------|
| `PROJECT_OVERVIEW.md` | High-level overview with hospital analogy |
| `OVERVIEW.md` | Architecture, API reference, deployment guide |
| `USAGE.md` | CLI and API usage walkthrough |
| `INSTALL.md` | Installation guide |
| `CLAUDE.md` | Comprehensive developer reference |
| `AGENT_ROADMAP.md` | Future agent plans with architecture patterns |
| `THREAT_MODEL_V4.md` | Full STRIDE threat model with Mermaid diagrams |
| `docs/reliable-local-llm-tool-calling.md` | 14 changes for local LLM reliability |
| `docs/local-llm-context-window-tricks.md` | Six-system guide for context management |
| `docs/oop-sandbox-manual.md` | OOP executor + bwrap + artifact pipeline |
| `docs/sandbox-dynamic-analysis.md` | QEMU/KVM + Frida sandbox setup |
| `docs/postgresql-permissions.md` | PostgreSQL + pgvector setup reference |
