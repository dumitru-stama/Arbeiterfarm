# Using Reverse-Arbeiterfarm

## Quick Start

```bash
# 1. Create a project
af project create malware-sample-42

# 2. Upload a binary for analysis
af artifact add /path/to/suspicious.exe --project <project-id>

# 3. Start an interactive analysis session
af chat --project <project-id>
```

The agent will have access to all registered tools and can analyze the uploaded file on your behalf.

---

## Projects and Artifacts

Projects are containers for related analysis work. Each project holds artifacts (uploaded files), threads (conversations), and extracted IOCs.

```bash
# Create a project
af project create "apt29-campaign"

# List projects
af project list

# Upload files into a project
af artifact add sample.exe --project <project-id>
af artifact add config.bin --project <project-id>

# List artifacts in a project
af artifact list --project <project-id>

# Show artifact details (hashes, size, mime type)
af artifact info <artifact-id>
```

---

## Interactive Chat

Start a conversation with an AI agent that can use analysis tools:

```bash
af chat --project <project-id>
```

### Chat Options

| Flag | Purpose |
|---|---|
| `--agent <name>` | Use a specific agent (default: `default`) |
| `--thread <id>` | Resume an existing conversation |
| `--workflow <name>` | Run a multi-agent workflow instead of interactive chat |

### In-Chat Commands

| Command | Action |
|---|---|
| `/tools` | List available tools |
| `/history` | Show conversation history |
| `/thread` | Show current thread ID |
| `/help` | Show available commands |
| `/quit` | Exit the session |

---

## Agents

Agents are specialized AI personas with different system prompts, tool access, and LLM routing. Seven agents are built in, and you can create custom agents at runtime without recompiling.

### Built-in Agents

| Agent | Focus | Key Tools |
|---|---|---|
| `default` | General reverse engineering | All tools |
| `surface` | Quick triage (imports, exports, strings) | rizin.bininfo, strings.extract, file.info, file.strings |
| `decompiler` | Code analysis and pseudocode | ghidra.analyze, ghidra.decompile, rizin.disasm, rizin.xrefs |
| `asm` | Low-level assembly verification | rizin.disasm, rizin.xrefs (forces local LLM) |
| `intel` | Threat intelligence lookups | vt.file_report, re-ioc.list, re-ioc.pivot |
| `reporter` | Structured report writing | re-ioc.list, re-ioc.pivot |
| `researcher` | Web research and OSINT | web.fetch, web.search (requires web gateway) |
| `tracer` | Dynamic analysis and behavior | sandbox.trace, sandbox.hook, sandbox.screenshot, file.* (requires sandbox gateway) |

### Using a Specific Agent

```bash
# Triage a sample quickly
af chat --project <id> --agent surface

# Deep code analysis
af chat --project <id> --agent decompiler

# Threat intelligence lookup
af chat --project <id> --agent intel
```

### Creating Custom Agents

Agents are stored in the database. Create them via CLI or API — no recompilation needed. The `--prompt` field is free-form text; structure it however you want.

```bash
af agent create \
  --name "malware-specialist" \
  --prompt "You are an expert malware analyst specializing in API hooking and process injection techniques." \
  --tools "file.*,rizin.*,ghidra.*,vt.*,re-ioc.*" \
  --route auto
```

The `--tools` flag accepts comma-separated glob patterns matching tool names.

The `--route` flag controls which LLM backend the agent uses:
- `auto` — use the first registered backend (default)
- `local` — force local backend only (no data sent to cloud)
- `backend:<name>` — target a specific backend (e.g. `backend:openai:gpt-4o-mini`)

Use it immediately — no restart required:

```bash
af chat --project <id> --agent malware-specialist
```

### Real-World Agent Examples

**Ransomware analyst** — focused on encryption routines and C2 communication:

```bash
af agent create \
  --name "ransomware-analyst" \
  --prompt "## Role
You are a ransomware specialist. Your goal is to identify encryption
schemes, key derivation, C2 protocols, and potential decryption vectors.

## Methodology
1. Check imports for crypto APIs (CryptEncrypt, BCryptEncrypt, AES/RSA libs)
2. Identify file enumeration routines (FindFirstFile, recursive directory walks)
3. Look for C2 beacon patterns (HTTP POST, DNS TXT, hardcoded IPs/domains)
4. Check for shadow copy deletion (vssadmin, wmic, WMI calls)
5. Identify the encryption implementation — look for key scheduling, IV generation
6. Determine if key material is recoverable (hardcoded keys, weak PRNG, key escrow)

## Output
Always end with an actionable assessment:
- Encryption algorithm and mode identified
- Key recovery feasibility (possible / unlikely / impossible)
- C2 infrastructure IOCs extracted
- Recommended decryptor approach if applicable" \
  --tools "file.*,rizin.*,ghidra.*,strings.extract,vt.*,re-ioc.*" \
  --route auto
```

**Packer identifier** — minimal tools, fast triage of packed samples:

```bash
af agent create \
  --name "packer-id" \
  --prompt "## Role
You identify packers, protectors, and obfuscation in PE/ELF binaries.

## Approach
1. Check section names and entropy (UPX, Themida, VMProtect signatures)
2. Look at import table — packed binaries have minimal imports (LoadLibrary, GetProcAddress)
3. Check entry point location — is it in a non-standard section?
4. Look for known packer signatures in strings and byte patterns
5. Identify the original entry point (OEP) if possible

## Output
Report: packer name, version if detectable, unpacking difficulty (trivial/moderate/hard),
and whether the sample can be statically unpacked." \
  --tools "rizin.bininfo,strings.extract,file.info,file.strings,file.hexdump,file.grep" \
  --route auto
```

**IoT firmware analyst** — for embedded device binaries:

```bash
af agent create \
  --name "firmware-analyst" \
  --prompt "## Role
You analyze IoT and embedded firmware images (ARM, MIPS, RISC-V).

## Focus Areas
- Hardcoded credentials (passwords, API keys, certificates)
- Backdoor accounts and debug interfaces (telnet, UART, JTAG references)
- Known vulnerable libraries (versions of OpenSSL, busybox, libcurl)
- Network services and their configurations
- Update mechanism security (signature verification, plaintext HTTP)
- Command injection sinks in CGI/web handlers

## Constraints
Do NOT assume x86 conventions. Pay attention to the architecture field
from rizin.bininfo and adjust your analysis accordingly." \
  --tools "file.*,rizin.*,ghidra.*,strings.extract,re-ioc.*" \
  --route auto
```

**Local-only analyst** — forces all queries through a local LLM (no data sent to cloud):

```bash
af agent create \
  --name "local-only" \
  --prompt "You are a reverse engineering assistant. Analyze binaries thoroughly.
Never summarize or skip details — the user needs complete technical output." \
  --tools "file.*,rizin.*,ghidra.*,strings.extract" \
  --route local
```

The `--route local` flag ensures this agent only uses the locally-configured LLM backend (e.g., Ollama). Useful for classified or air-gapped environments.

### Managing Agents

```bash
# List all agents (builtin + custom)
af agent list

# Show agent details (system prompt, tools, route)
af agent show ransomware-analyst

# Delete a custom agent (builtins cannot be deleted)
af agent delete packer-id
```

---

## Workflows

Workflows orchestrate multiple agents in a defined pipeline. Agents in the same group run in parallel; groups execute sequentially. All agents share a single thread, so later agents see everything earlier agents produced.

This is the key feature for complex analysis: instead of running one agent at a time, you define a pipeline where specialized agents collaborate automatically.

### How Workflows Work

```
Group 1:  [surface] ──────┐
          [intel]   ──────┤  (run in parallel, same thread)
                          │
Group 2:  [decompiler] ───┤  (waits for group 1, sees all their output)
                          │
Group 3:  [reporter] ─────┘  (sees everything, writes final report)
```

- **Same group number = parallel execution.** Agents run concurrently via separate async tasks.
- **Ascending group numbers = sequential.** Group 2 starts only after all Group 1 agents finish.
- **Shared thread.** Every agent reads the full message history. The decompiler sees what surface and intel found. The reporter sees everything.
- **Agent attribution.** Each message is tagged with its agent name (e.g., `[surface]: Found UPX-packed PE32...`), so downstream agents know who said what.

### Built-in Workflow: full-analysis

```bash
af chat --project <id> --workflow full-analysis
```

| Group | Agents | What Happens |
|---|---|---|
| 1 (parallel) | `surface` + `intel` | Quick triage and threat intelligence run simultaneously |
| 2 | `decompiler` | Deep code analysis of key functions identified in group 1 |
| 3 | `reporter` | Synthesizes all findings into a structured report |

### Creating Custom Workflows

Workflows are created via the API. Each step specifies an agent name, a group number, and a task-specific prompt that gets appended to the agent's system prompt.

**Example: Ransomware response pipeline**

```bash
# First, create the specialized agents (if not already created)
af agent create --name "crypto-hunter" \
  --prompt "You specialize in identifying cryptographic implementations.
Focus on: key generation, encryption algorithms, IV/nonce handling,
key storage, and any weaknesses that could enable decryption." \
  --tools "ghidra.*,rizin.*,file.*,strings.extract" --route auto

af agent create --name "c2-tracker" \
  --prompt "You specialize in command-and-control infrastructure analysis.
Focus on: network indicators, domain generation algorithms, protocol
analysis, beacon intervals, and communication encryption." \
  --tools "file.*,rizin.*,strings.extract,vt.*,re-ioc.*" --route auto

# Then create the workflow via API
curl -X POST http://localhost:8080/api/v1/workflows \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ransomware-response",
    "description": "Specialized ransomware analysis pipeline",
    "steps": [
      {"agent": "surface",       "group": 1, "prompt": "Identify the ransomware family. Check for known signatures, ransom notes, and mutex names."},
      {"agent": "intel",         "group": 1, "prompt": "Look up all hashes on VirusTotal. Report detection names, tags, and known family associations."},
      {"agent": "crypto-hunter", "group": 2, "prompt": "Based on the surface analysis, find and analyze all cryptographic functions. Determine the encryption scheme and key derivation method. Assess decryption feasibility."},
      {"agent": "c2-tracker",    "group": 2, "prompt": "Based on the surface analysis, identify all C2 communication mechanisms. Extract network IOCs (IPs, domains, URLs). Analyze the C2 protocol."},
      {"agent": "reporter",      "group": 3, "prompt": "Write a ransomware incident response report. Include: family identification, encryption assessment, decryption feasibility, C2 infrastructure, all IOCs, and recommended containment actions."}
    ]
  }'
```

Run it:

```bash
af chat --project <id> --workflow ransomware-response
# Prompt: "Analyze this ransomware sample"
```

What happens:
1. **Group 1** (parallel): `surface` triages the binary while `intel` queries VirusTotal — both run simultaneously
2. **Group 2** (parallel): `crypto-hunter` analyzes encryption routines while `c2-tracker` maps network infrastructure — both see Group 1's findings
3. **Group 3**: `reporter` reads everything and produces a structured incident response report

**Example: Quick triage pipeline** (fast, minimal)

```bash
curl -X POST http://localhost:8080/api/v1/workflows \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "quick-triage",
    "description": "Fast parallel triage for high-volume sample processing",
    "steps": [
      {"agent": "surface", "group": 1, "prompt": "Quick triage only. Report: file type, architecture, packer, notable imports, suspicious strings. Keep it brief."},
      {"agent": "intel",   "group": 1, "prompt": "Hash lookup only. Report detection ratio and family name if known."},
      {"agent": "reporter","group": 2, "prompt": "Write a one-paragraph triage summary. Verdict: clean / suspicious / malicious. Include confidence level."}
    ]
  }'
```

**Example: Supply chain audit pipeline**

```bash
curl -X POST http://localhost:8080/api/v1/workflows \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "supply-chain-audit",
    "description": "Audit a binary for supply chain compromise indicators",
    "steps": [
      {"agent": "surface",     "group": 1, "prompt": "Catalog all imports, exports, and embedded resources. Flag any unexpected network-related APIs, debug artifacts, or test certificates."},
      {"agent": "intel",       "group": 1, "prompt": "Check all hashes against VirusTotal. Flag if first-seen date is very recent or if signing certificate changed from known-good."},
      {"agent": "decompiler",  "group": 2, "prompt": "Examine initialization routines, DllMain, and any code that runs before main(). Look for: suspicious thread creation, injected code paths, environment checks, delayed execution, anti-analysis."},
      {"agent": "decompiler",  "group": 3, "prompt": "Examine all network-related functions identified in prior analysis. Look for: unexpected outbound connections, data exfiltration patterns, unusual DNS queries, hardcoded infrastructure."},
      {"agent": "reporter",    "group": 4, "prompt": "Write a supply chain risk assessment. Include: binary provenance, code signing status, suspicious code paths found, network behavior, IOCs, and overall risk rating (low/medium/high/critical)."}
    ]
  }'
```

Note that the same agent (`decompiler`) can appear in multiple groups with different prompts. In Group 2 it examines initialization code; in Group 3 it focuses on network functions — each time with full context of everything before it.

### Fan-Out: Container Extraction

When a tool extracts multiple files from a container (zip, tar, self-extracting archive), the orchestrator automatically creates **child threads** — one per extracted file — and runs the full workflow on each child independently. The parent workflow blocks until all children complete.

```
Parent workflow (thread A):
  Group 1: [surface] + [intel]     ← discovers zip with 3 files
           ↓ fan-out detected
  Child thread B: full workflow on file1.exe
  Child thread C: full workflow on file2.dll
  Child thread D: full workflow on file3.sys
           ↓ all children complete
  Group 2: [decompiler]            ← sees parent + child findings
  Group 3: [reporter]              ← synthesizes everything
```

**How it works**:
- Tools mark extracted children with `metadata: {"fan_out_from": "<parent-artifact-id>"}`
- The orchestrator detects these after each group completes
- Each child gets its own thread with `parent_thread_id` linking back to the parent
- Child threads run the **same workflow steps** as the parent
- Events from child workflows stream through the parent's event channel

**Safety limits**:
- **Max depth = 3**: Recursive fan-out (zip-in-zip-in-zip) supported up to 3 levels deep
- **SHA256 cycle detection**: If the same file (by SHA256) has already been analyzed in the current tree, it's skipped to prevent infinite loops
- **Max children = 50**: Excess artifacts per fan-out event are logged and skipped
- **Same tenant scope**: Child threads inherit the parent's project and user context

Fan-out events appear in the CLI output:

```
  [FAN-OUT] artifact:<parent-id> -> 3 child threads
    thread:<child-id-1>
    thread:<child-id-2>
    thread:<child-id-3>
  [FAN-OUT COMPLETE] parent:<thread-id> — 3 succeeded, 0 failed (3 total)
```

### Managing Workflows

```bash
# List all workflows
af workflow list

# Show workflow details (agents, groups, steps)
af workflow show ransomware-response
```

---

## Tools

Tools are the capabilities available to agents. Each tool runs in a sandboxed subprocess by default.

### Listing Tools

```bash
af tool list
```

### Available Tools

**File Analysis** (always available):

| Tool | Description |
|---|---|
| `file.info` | File metadata, hashes (MD5/SHA1/SHA256), magic bytes |
| `file.read_range` | Read byte or line ranges from a file |
| `file.strings` | Extract printable strings |
| `file.hexdump` | Hex + ASCII dump of byte ranges |
| `file.grep` | Regex search with context lines |

**Binary Analysis** (requires rizin):

| Tool | Description |
|---|---|
| `rizin.bininfo` | Imports, exports, sections, entry point, architecture |
| `rizin.disasm` | Disassemble address ranges |
| `rizin.xrefs` | Cross-references (to/from/both) |
| `strings.extract` | Advanced string extraction with encoding detection |

**Decompilation** (requires Ghidra):

| Tool | Description |
|---|---|
| `ghidra.analyze` | Headless analysis, function list extraction (shared cache for non-NDA, isolated for NDA projects) |
| `ghidra.decompile` | Decompile specific functions to C pseudocode |
| `ghidra.rename` | Rename functions in DB overlay (max 50 pairs per call, applied during decompile) |
| `ghidra.suggest_renames` | Surface function renames from other non-NDA projects for the same binary |

**Threat Intelligence** (requires VT API key):

| Tool | Description |
|---|---|
| `vt.file_report` | VirusTotal hash lookup: detections, tags, families, first-seen |

**Dynamic Analysis** (requires sandbox gateway):

| Tool | Description |
|---|---|
| `sandbox.trace` | Execute sample in QEMU/KVM VM with Frida API hooking (~60 default hooks) |
| `sandbox.hook` | Inject custom Frida hooks for targeted instrumentation |
| `sandbox.screenshot` | Capture VM screen state |

**Web Research** (requires web gateway):

| Tool | Description |
|---|---|
| `web.fetch` | Fetch a URL, convert HTML to plaintext, extract links |
| `web.search` | Search the web via DuckDuckGo, return results with snippets |

These tools are **restricted by default** — users need an explicit grant from an admin before they can use them. See [Tool Restrictions](#tool-restrictions) below.

**IOC Management** (requires DB):

| Tool | Description |
|---|---|
| `re-ioc.list` | List extracted IOCs (IPs, domains, URLs, hashes, emails, mutexes, registry keys) |
| `re-ioc.pivot` | Pivot on an IOC value across project artifacts and tool runs |

### Running a Tool Directly

```bash
af tool run file.info --project <id> --input '{"artifact_id": "<artifact-id>"}'
```

### Enabling and Disabling Tools

Disabled tools are hidden from agents and cannot be invoked. The setting persists across restarts.

```bash
af tool disable rizin.bininfo
af tool enable rizin.bininfo
```

---

## Writing Tools: Artifact Metadata Channel

Tools communicate with the orchestrator through **artifact metadata** — not through agent text output. When a tool creates an output artifact, it can attach metadata that the orchestrator detects after each workflow group completes.

### OutputStore API

Every tool executor receives a `ToolContext` with an `output_store` field. The `OutputStore` trait has two methods:

```rust
// Standard artifact storage (no orchestrator directives)
ctx.output_store.store(filename, data, mime_type).await?;

// Artifact storage with metadata (triggers orchestrator behavior)
ctx.output_store.store_with_metadata(filename, data, mime_type, metadata).await?;
```

### Repivot: 1:1 Replacement

When a tool produces a "better" version of its input (e.g., UPX unpacking), it tags the output with `repivot_from` metadata. The orchestrator replaces the original artifact and re-runs the current group's agents on the new version.

```rust
use af_core::context::repivot_metadata;

// In your tool executor:
let unpacked_data = upx_unpack(&input_bytes)?;
let metadata = repivot_metadata(original_artifact_id);
ctx.output_store.store_with_metadata(
    "unpacked.exe", &unpacked_data, Some("application/octet-stream"), metadata
).await?;
```

The metadata produced: `{"repivot_from": "<original-artifact-uuid>"}`

### Fan-Out: 1:N Extraction

When a tool extracts multiple files from a container (zip, tar, self-extracting archive), it tags each child with `fan_out_from` metadata. The orchestrator creates a child thread per extracted file and runs the full workflow on each.

```rust
use af_core::context::fanout_metadata;

// In your tool executor, for each extracted file:
let metadata = fanout_metadata(parent_artifact_id);
ctx.output_store.store_with_metadata(
    &child_filename, &child_data, Some("application/octet-stream"), metadata
).await?;
```

The metadata produced: `{"fan_out_from": "<parent-artifact-uuid>"}`

### Key Points

- **Agents don't know about these directives.** The LLM just calls a tool (e.g., `archive.extract`). The tool's compiled Rust code decides whether to tag artifacts.
- **Metadata is stored in the `artifacts.metadata` JSONB column.** The orchestrator queries for it after each group via `find_repivot_artifacts_since()` and `find_fanout_artifacts_since()`.
- **Multiple children per tool run are fine.** A zip extractor can call `store_with_metadata` once per extracted file, all with `fanout_metadata(zip_artifact_id)`.
- **Repivot and fan-out are orthogonal.** Both are checked after each group. A tool can trigger one or the other (not both on the same artifact).

---

## Threads and Exports

Threads are conversation histories. Each thread belongs to a project and records all messages, tool calls, and evidence citations.

### Thread Targeting

Threads can optionally target a specific artifact via `target_artifact_id`. When set, the thread's context window contains only that sample and its generated children — ensuring deterministic, scoped analysis. Created via API:

```json
{"title": "Analyze amixer", "target_artifact_id": "<artifact-uuid>"}
```

In the web UI, clicking "Analyze" on a sample automatically creates a targeted thread.

### Listing and Viewing Threads

```bash
# List threads in a project
af thread list --project <project-id>

# Show messages in a thread
af thread show <thread-id>
```

### Resuming a Thread

```bash
af chat --project <id> --thread <thread-id>
```

### Exporting a Thread

Export a complete analysis as a Markdown report or JSON document:

```bash
# Markdown (default) — suitable for reports
af thread export <thread-id> --format markdown > report.md

# JSON — suitable for integration with other tools
af thread export <thread-id> --format json > analysis.json
```

Exports include full message history with agent attribution, tool execution results, and evidence citations.

---

## Audit Log

All tool executions, agent operations, and configuration changes are logged.

```bash
# List recent audit entries
af audit list

# Limit results
af audit list --limit 20

# Filter by event type
af audit list --type tool_run
```

---

## Web UI

Reverse-Arbeiterfarm includes a built-in web interface. No build step or npm install needed — it's a vanilla JavaScript SPA served directly by the backend.

### Running

```bash
af serve --bind 127.0.0.1:8080
# Open http://localhost:8080 in your browser
```

Log in with an API key (create one via CLI if you don't have one yet):

```bash
af user create --name alice --roles operator
af user api-key create --user <user-id> --name "browser"
# Copy the raw key shown and paste it into the login screen
```

### Features

- **Dashboard** — system status, recent threads and artifacts
- **Projects** — create projects, upload artifacts, manage threads
- **Chat** — send messages with SSE streaming, select agent and route per-message
- **Workflows** — create, edit, and execute multi-agent workflows
- **Agents** — browse builtin agents, create custom agents with system prompt editor
- **Tools** — list registered tools, enable/disable
- **Admin** — user management, API key creation/revocation, quota management
- **Audit** — browse audit log entries
- **Search** — full-text search across artifacts and messages
- **Themes** — Default, Lab, Print, and Dark themes, toggle from the top bar
- **NDA Indicator** — blueish background tint on all project-scoped views when working with NDA-covered projects; NDA badge on project listings

### Architecture

The UI is three files in the `ui/` directory:

```
ui/
├── index.html    # Entry point (~400 B)
├── app.js        # Complete SPA (~100 KB)
└── styles.css    # All styling (~9 KB)
```

- No framework (no React, Vue, etc.), no build tools, no node_modules
- Hash-based routing (`#/projects`, `#/thread/uuid`, etc.)
- Session state (API key) stored in `sessionStorage` (clears on tab close); theme and selected agent in `localStorage`
- API base URL defaults to `/api/v1`, override with `window.REVERSE_AF_API_BASE` before loading

The Rust backend serves these files via `ServeDir::new("ui")` with a fallback to `index.html` for SPA routing.

### CORS

When running the UI from the same server (default), no CORS configuration is needed. If serving the UI from a different origin, set:

```bash
export AF_CORS_ORIGIN="http://localhost:3000"  # or "*" for permissive
```

---

## HTTP API Server

Reverse-Arbeiterfarm can run as an HTTP API server for integration with other systems.

```bash
af serve --bind 127.0.0.1:8080
```

### API Endpoints

**Projects**:
- `GET /api/v1/projects` — list projects
- `POST /api/v1/projects` — create project

**Threads**:
- `GET /api/v1/projects/:id/threads` — list threads
- `POST /api/v1/projects/:id/threads` — create thread
- `GET /api/v1/threads/:id/export` — export thread (`?format=markdown` or `?format=json`)

**Messages**:
- `GET /api/v1/threads/:id/messages` — list messages
- `POST /api/v1/threads/:id/messages` — send message (SSE streaming response)
- `POST /api/v1/threads/:id/messages/sync` — send message (non-streaming response)
- `POST /api/v1/threads/:id/messages/queue` — queue message without LLM invocation

Send message accepts an optional `route` field to override the agent's default LLM backend for that message only:

```json
{"content": "Analyze this sample", "route": "backend:openai:gpt-4o-mini"}
```

**Workflows**:
- `GET /api/v1/workflows` — list workflows
- `POST /api/v1/workflows` — create workflow
- `GET /api/v1/workflows/:name` — get workflow
- `PUT /api/v1/workflows/:name` — update workflow
- `DELETE /api/v1/workflows/:name` — delete workflow
- `POST /api/v1/threads/:id/workflow` — execute workflow (SSE streaming)

Workflow execute accepts an optional `route` field to override all agents' LLM backend for the entire workflow run (including fan-out children):

```json
{"workflow_name": "full-analysis", "content": "Analyze this", "route": "backend:anthropic:claude-haiku-4-5-20251001"}
```

**Agents**:
- `GET /api/v1/agents` — list agents
- `POST /api/v1/agents` — create agent
- `GET /api/v1/agents/:name` — get agent
- `PUT /api/v1/agents/:name` — update agent
- `DELETE /api/v1/agents/:name` — delete agent

**Artifacts**:
- `GET /api/v1/projects/:id/artifacts` — list artifacts
- `POST /api/v1/projects/:id/artifacts` — upload artifact (multipart)
- `GET /api/v1/artifacts/:id/download` — download artifact

**LLM Backends**:
- `GET /api/v1/llm/backends` — list registered backends and available routes

**Audit**:
- `GET /api/v1/audit` — list audit log entries

**Quotas**:
- `GET /api/v1/quota` — get own quota and usage
- `GET /api/v1/admin/quota/:user_id` — get user quota (admin only)
- `PUT /api/v1/admin/quota/:user_id` — update user quota (admin only)

**Admin: Users** (admin only):
- `GET /api/v1/admin/users` — list all users
- `POST /api/v1/admin/users` — create user
- `GET /api/v1/admin/users/:id` — get user details

**Admin: API Keys** (admin only):
- `POST /api/v1/admin/users/:id/api_keys` — create API key for user (returns raw key once)
- `GET /api/v1/admin/users/:id/api_keys` — list API keys for user
- `DELETE /api/v1/admin/api_keys/:id` — revoke API key

**Web Rules** (admin for global; project access for project-scoped):
- `GET /api/v1/web-rules` — list URL rules (`?project_id=uuid` for project-scoped)
- `POST /api/v1/web-rules` — add URL rule (domain, domain_suffix, url_prefix, url_regex, ip_cidr)
- `DELETE /api/v1/web-rules/:id` — remove URL rule (admin only)
- `GET /api/v1/web-rules/countries` — list blocked countries
- `POST /api/v1/web-rules/countries` — block a country (admin only, ISO 3166-1 alpha-2 code)
- `DELETE /api/v1/web-rules/countries/:code` — unblock a country (admin only)

**Admin: Restricted Tools** (admin only):
- `GET /api/v1/admin/restricted-tools` — list restricted tool patterns
- `POST /api/v1/admin/restricted-tools` — add restricted pattern (e.g., `web.*`)
- `DELETE /api/v1/admin/restricted-tools` — remove restricted pattern

**Admin: User Tool Grants** (admin only):
- `GET /api/v1/admin/users/:id/tool-grants` — list tool grants for a user
- `POST /api/v1/admin/users/:id/tool-grants` — grant tool access to a user
- `DELETE /api/v1/admin/users/:id/tool-grants` — revoke tool grant from a user

### Authentication

API requests require an API key passed via the `Authorization: Bearer <key>` header.

```bash
# Create a user and API key
af user create --name alice --roles operator
af user api-key create --user <user-id> --name "alice-laptop"

# Use the API key
curl -H "Authorization: Bearer <key>" http://localhost:8080/api/v1/projects
```

---

## Worker Daemon

For production deployments, the worker daemon processes tool executions concurrently in the background. Multiple workers can run against the same database for horizontal scaling.

```bash
af worker start --concurrency 4 --poll-ms 500
```

| Flag | Default | Purpose |
|---|---|---|
| `--concurrency` | 4 | Number of concurrent worker threads |
| `--poll-ms` | 500 | Poll interval when idle (milliseconds) |

The worker uses PostgreSQL `FOR UPDATE SKIP LOCKED` for job claiming, so multiple instances are safe to run concurrently. Graceful shutdown on Ctrl-C (in-progress jobs complete before exit).

---

## User Management

For multi-tenant deployments with the API server.

```bash
# Create a user
af user create --name alice --display "Alice" --email alice@example.com --roles operator

# List users
af user list

# Create an API key for a user
af user api-key create --user <user-id> --name "ci-pipeline"

# List API keys
af user api-key list --user <user-id>

# Revoke an API key
af user api-key revoke <key-id>
```

---

## Web Rules

Admins can control which URLs agents are allowed to fetch using URL rules. Rules can be global (apply to all projects) or project-scoped.

### Rule Types

| Type | Behavior |
|---|---|
| `allow` | Explicitly permit matching URLs |
| `block` | Explicitly block matching URLs (overrides allow rules) |

If **only block rules** exist, everything not blocked is allowed (blocklist mode). If **any allow rules** exist, only matching URLs are allowed (allowlist mode).

### Pattern Types

| Pattern | Example | Matches |
|---|---|---|
| `domain` | `example.com` | Exact domain match |
| `domain_suffix` | `.ru` | All domains ending in `.ru` (respects domain boundaries) |
| `url_prefix` | `https://api.example.com/v1/` | URLs starting with the prefix |
| `url_regex` | `^https://.*\.gov/.*` | Full regex match against URL |
| `ip_cidr` | `10.0.0.0/8` | Matches resolved IPs against CIDR range |

### Managing Web Rules

```bash
# Add a global block rule (admin only)
af web-rule add --block --domain-suffix .ru "Block Russian domains"

# Add a project-scoped allow rule
af web-rule add --allow --domain example.com --project <id>

# Add a CIDR block rule
af web-rule add --block --ip-cidr 10.0.0.0/8 "Block private IPs"

# List all rules
af web-rule list

# Remove a rule
af web-rule remove <rule-id>

# Block a country (GeoIP, requires MaxMind database)
af web-rule block-country RU

# Unblock a country
af web-rule unblock-country RU

# List blocked countries
af web-rule list-countries
```

---

## Tool Restrictions

Some tools require explicit admin grants before users can use them. By default, `web.*` tools (web.fetch, web.search) are restricted — users must be granted access by an admin.

### How It Works

1. A tool pattern (e.g., `web.*`) is added to the restricted list
2. Users without a matching grant see "requires a grant from your administrator" when the agent tries to use the tool
3. An admin grants a user access with a matching pattern (exact or wildcard)

### Managing Restrictions

```bash
# List restricted tool patterns
af grant restricted

# Add a new restricted pattern (admin only)
af grant restrict "custom.dangerous.*"

# Remove a restriction
af grant unrestrict "custom.dangerous.*"

# Grant a user access to web tools
af grant tool <user-id> "web.*"

# List a user's grants
af grant list <user-id>

# Revoke a grant
af grant revoke <user-id> "web.*"
```

### Pattern Matching

| Pattern | Matches |
|---|---|
| `web.fetch` | Only `web.fetch` (exact) |
| `web.*` | `web.fetch`, `web.search` (wildcard) |
| `*` | All tools (universal) |

---

## Command Reference

```
af project create <name> [--nda]
af project nda <id> --on|--off
af project settings <id> [--set key=value]
af project list

af artifact add <file> --project <id>
af artifact list --project <id>
af artifact info <artifact-id>

af chat --project <id> [--agent <name>] [--thread <id>] [--workflow <name>]

af thread list --project <id>
af thread show <thread-id>
af thread export <thread-id> [--format markdown|json]

af tool list
af tool run <tool> --project <id> --input <json>
af tool enable <tool>
af tool disable <tool>

af agent list
af agent show <name>
af agent create --name <n> --prompt <p> --tools <patterns> [--route auto|local|backend:<name>]
af agent delete <name>

af workflow list
af workflow show <name>

af think --project <id> --goal "..." [--agent <name>]

af audit list [--limit N] [--type <event-type>]

af web-rule add --block|--allow --domain|--domain-suffix|--url-prefix|--url-regex|--ip-cidr <pattern> [--project <id>] [description]
af web-rule remove <rule-id>
af web-rule list
af web-rule block-country <code>
af web-rule unblock-country <code>
af web-rule list-countries

af grant restricted
af grant restrict <pattern>
af grant unrestrict <pattern>
af grant tool <user-id> <pattern>
af grant list <user-id>
af grant revoke <user-id> <pattern>

af ghidra-renames list --project <id> --artifact <artifact-id>
af ghidra-renames suggest --project <id> --artifact <artifact-id>
af ghidra-renames import --project <id> --artifact <artifact-id> --from-project <source-project-id>

af serve [--bind 127.0.0.1:8080]

af worker start [--concurrency N] [--poll-ms MS]

af user create --name <subject> [--display <text>] [--email <addr>] [--roles <roles>]
af user list
af user api-key create --user <id> --name <desc>
af user api-key list --user <id>
af user api-key revoke <key-id>
af user routes <user-id> [--add route] [--remove route] [--clear]
```
