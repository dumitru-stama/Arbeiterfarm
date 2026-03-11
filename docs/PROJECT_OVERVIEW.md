# Arbeiterfarm & Reverse-Arbeiterfarm: Project Overview

## 1. What Is This? (The 30-Second Version)

**Arbeiterfarm** is an SDK for building AI-powered technical workstations. Think of it as a factory for creating specialized AI assistants that can use real tools — not just chat.

**Reverse-Arbeiterfarm** is the first product built on Arbeiterfarm. It's a reverse engineering workstation where AI agents analyze malware, decompile code, look up threat intelligence, and write reports — all with full audit trails and enterprise-grade security.

The key idea: the AI doesn't just *talk about* analyzing a binary. It actually runs Ghidra, reads disassembly, queries VirusTotal, and produces verifiable results you can trace back to raw tool output.

**Who Is This For?**
- Reverse engineers and malware analysts who need reproducible analysis
- Security teams that want multi-tenant isolation and audit trails
- Platform engineers building tool-using AI workflows with strict controls

---

## 2. The Hospital Analogy (Big Picture)

Think of **Arbeiterfarm** as a **hospital system** you build once, and then reuse for different specialties.

- **The Hospital Infrastructure (Arbeiterfarm SDK)**: Triage process, isolation rooms, lab pipeline, compliance rules. You build this once.
- **The Infection Ward (Reverse-Arbeiterfarm)**: Specialized tools (Rizin, Ghidra), labs (VirusTotal), and staff (agents trained in reverse engineering).
- **A New Specialty Tomorrow**: Swap the tools and specialists (code review, compliance, finance). For example, **Radiology/Pathology** maps to vulnerability research — deep scans and root-cause analysis to identify weaknesses before they cause incidents. Same hospital, same safety systems.

---

## 3. How Agents Work (Soul, Brain, Hands)

An agent has three layers:

**The Soul (System Prompt)** — *Who am I and how do I think?*

This is the agent's personality, expertise, and methodology. It's free-form text that you write — no code changes needed. You can structure it however you want: role descriptions, step-by-step methodology, output format instructions, constraints.

```
## Role
You are a ransomware specialist...

## Methodology
1. Check imports for crypto APIs
2. Identify file enumeration routines
...

## Output Format
Always end with an actionable assessment...
```

**The Brain (LLM Route)** — *Which AI model powers me?*

Each agent can be routed to a different LLM backend:
- `auto` — use the best available backend
- `local` — force local model only (for classified work)
- `backend:anthropic` — use a specific named backend

**The Hands (Tool Access)** — *What can I actually do?*

A whitelist of tool patterns the agent is authorized to use. Even if the LLM tries to call a tool outside this list, the runtime rejects it.

```
file.*           → all file analysis tools
rizin.*          → all rizin tools
ghidra.*         → Ghidra analyze + decompile
vt.*             → VirusTotal lookups
re-ioc.*         → IOC listing and pivoting
```

### Dynamic Agent Creation

Agents are stored in the database, not compiled into the binary. Create, modify, or delete agents at runtime via CLI or API — changes take effect immediately, no restart needed.

```bash
af agent create \
  --name "firmware-analyst" \
  --prompt "You analyze IoT firmware. Focus on hardcoded credentials..." \
  --tools "file.*,rizin.*,ghidra.*,strings.extract" \
  --route local
```

Six agents are built in (surface, decompiler, asm, intel, reporter, default). Builtins cannot be deleted but can be updated. A built-in "thinker" agent is also available for autonomous orchestration (see Section 4b).

---

## 4. Workflows: The AI Assembly Line

### The Triage Analogy (How Workflows Run)

A workflow is like an **emergency room triage process**:

1. **Triage nurse + Lab tech** (Group 1, parallel): The nurse does a quick physical assessment while the lab tech draws blood and runs tests. They work simultaneously — neither waits for the other.

2. **Specialist doctor** (Group 2, sequential): The doctor arrives *after* both the nurse's notes and the lab results are ready. She reads everything before making her diagnosis.

3. **Discharge coordinator** (Group 3, sequential): Writes the final patient report after the doctor's diagnosis is complete, pulling together the nurse's observations, lab results, and the doctor's findings into one coherent document.

The critical property: **everyone reads the same chart**. The doctor doesn't start from scratch — she sees exactly what the nurse wrote and what the lab found. The discharge coordinator sees everything.

### How This Maps to Reverse-Arbeiterfarm

Replace "patient" with "malware sample":

```
Group 1 (parallel):
  [surface agent]  — Quick triage: file type, imports, strings, packer detection
  [intel agent]    — VirusTotal lookup, IOC extraction
                     ↓ both finish
Group 2:
  [decompiler]     — Reads triage findings, decompiles suspicious functions
                     ↓ finishes
Group 3:
  [reporter]       — Reads everything, writes the final analysis report
```

All agents share one **thread** (the patient chart). Messages are tagged with agent names (`[surface]: Found UPX packing...`) so downstream agents know who said what.

### Why This Matters

Without workflows, you'd manually copy-paste between separate chat sessions:
- "Hey decompiler agent, the surface agent found suspicious function at 0x401000, can you look at it?"
- "Hey reporter, here's what the decompiler found..."

With workflows, this collaboration happens automatically. The decompiler *sees* what surface found. The reporter *sees* everything. You upload a file, type "analyze this," and get a complete multi-perspective report.

### Custom Workflows

Define your own pipelines — any combination of agents, any grouping:

```json
{
  "name": "ransomware-response",
  "steps": [
    {"agent": "surface",       "group": 1, "prompt": "Identify ransomware family and packer."},
    {"agent": "intel",         "group": 1, "prompt": "Look up all hashes. Report detections and family."},
    {"agent": "crypto-hunter", "group": 2, "prompt": "Find encryption routines. Assess decryption feasibility."},
    {"agent": "c2-tracker",    "group": 2, "prompt": "Map C2 infrastructure. Extract network IOCs."},
    {"agent": "reporter",      "group": 3, "prompt": "Write incident response report with containment actions."}
  ]
}
```

The same agent can appear in multiple groups with different prompts. You can create workflows via the API without touching code.

### Fan-Out: Automatic Container Analysis

When a tool extracts multiple files from a container (zip, tar, self-extracting archive), the orchestrator automatically **fans out** — creating a child thread per extracted file and running the full workflow on each child independently. The parent blocks until all children finish, then continues with the remaining groups (e.g., the reporter sees all child findings).

**Analogy**: If a patient arrives with a sealed bag containing three unknown substances, the ER doesn't analyze the bag as one item. It opens the bag, sends each substance to its own lab for independent analysis, and the attending doctor reviews all three lab reports together before making a diagnosis.

Safety limits prevent abuse: max depth of 3 (recursive fan-out with SHA256 cycle detection), max 50 children per fan-out event, and all children inherit the parent's project/tenant scope.

---

## 4b. Thinking Threads: The AI Research Director

Workflows are powerful but predefined — you decide the agent pipeline in advance. **Thinking threads** are the third execution mode: an autonomous "thinker" agent that decides at runtime which specialists to invoke, reads their results, iterates, and synthesizes findings.

**Analogy**: Instead of following a fixed triage protocol, a senior research director examines the case, decides "I need the surface analyst first, then depending on what they find, maybe the decompiler, maybe the intel analyst," reads their reports, asks follow-up questions, and writes a synthesis.

```bash
af think --project p1 --goal "Determine if this sample is related to APT29"
```

The thinker agent has 5 internal tools:
- `meta.invoke_agent` — spawns a specialist agent on a new child thread
- `meta.read_thread` — reads results from a child thread
- `meta.list_agents` — discovers available specialists
- `meta.list_artifacts` — browses project artifacts
- `meta.read_artifact` — reads artifact content directly (text or hex dump)

**Safety**: Child agents cannot invoke further agents (anti-recursion — `meta.*` tools stripped). Budget of 30 tool calls. Timeouts cascade: child (5 min) < meta-tool (11 min) < thinker (30 min).

Custom thinker agents can be defined via TOML — any agent with `tools = ["internal.*"]` becomes a thinker.

---

## 4c. Context Compaction: Long Conversations Without Limits

In long analysis sessions (especially thinking threads with many tool calls), the conversation context can exceed the LLM's window. The system handles this with **two strategies** depending on the model type.

### Cloud Models (Claude, GPT-4o, Gemini)

1. Before each LLM call, the runtime estimates token usage (~4 chars/token)
2. When estimated tokens exceed 85% of the context window, older messages in the middle are summarized by an LLM
3. The summary preserves artifact UUIDs, evidence references, and key findings
4. Original messages stay in the database (flagged as compacted) for audit and export

Optionally, compaction can use a cheaper model (e.g., `gpt-4o-mini`) instead of the agent's own backend:

```toml
[compaction]
threshold = 0.85
summarization_route = "openai:gpt-4o-mini"
```

### Local Models (gpt-oss, Qwen3, DeepSeek, etc.)

Local models need a different approach — they can't afford to call another LLM for summarization (they often *are* the only LLM), and their smaller context windows fill up much faster. Three layers of defense, all at **zero LLM cost**:

1. **Sliding window trim** (fires at 50% of token budget): Groups messages into atomic "turns" (a user message + its tool calls + tool results). Walks back-to-front keeping a minimum tail of 6,000 tokens and 2 turns. Everything before the boundary is trimmed, and thread memory (see 4f) provides the compressed context.

2. **Local context reset** (fires at 60% threshold): A more aggressive trim — marks all body messages as compacted, rebuilds context from scratch: system prompt + thread memory + recent tail. Instant, deterministic, zero cost.

3. **LLM compaction fallback**: If the local context reset somehow fails, delegates to the cloud-style LLM summarization as a safety net.

The system prompt and recent messages (including complete tool-call groups) are never compacted — only the middle section.

---

## 4d-extra. Artifact-First Tool Output

Local models get overwhelmed by large inline tool results (24KB+ grep output causes a 20B model to lose track of its analysis plan). All tools that produce large output now store the full result as a project artifact and return a compact summary inline. The model uses `file.read_range` or `file.grep` to inspect specific details on demand.

**Example**: `ghidra.analyze` used to return 50KB of function listings inline. Now it stores `functions.json` as an artifact and returns: "847 functions found. Top functions: main (0x401000), parse_header (0x401500)... Use file.read_range to inspect details."

This works for all RE tools (rizin, Ghidra, strings, grep, sandbox) and is transparent to the user — artifacts appear in the project automatically.

---

## 4d. Malware Family Tracking

Agents can tag artifacts with malware family names during analysis:

```
family.tag    — "This sample belongs to LockBit 3.0 (high confidence)"
family.list   — "What families are in this project?"
family.search — "Which projects have BlackCat samples?" (cross-project)
family.untag  — Remove a family association
```

The intel agent is the primary family tagger. When `family.tag` is in an agent's tool list, the prompt builder automatically appends instructions for attribution. Family evidence references (`evidence:re:family:<id>`) are verified against the database and project scope.

---

## 4f. Thread Memory: How a 20B Model Remembers Its Task

Local models (especially 20B parameter models like gpt-oss) have a critical weakness: as the conversation grows with tool calls and results, they lose track of what they were doing. By message 4, a 20B model starts hallucinating tool results instead of calling tools. By message 6, it forgets the original task entirely.

**Thread memory** solves this with persistent, deterministic key/value storage per conversation:

- After every tool call, the system extracts a compact finding (256 chars max) and stores it in the database, keyed by tool name. For example, after `ghidra.analyze` returns 50KB of output, the system stores: `finding:ghidra_analyze → "847 functions, main at 0x401000, Go-compiled, stripped"`
- The user's original goal is stored from the first message: `goal → "Analyze the malware sample for behavioral indicators"`
- The current task is always tracked: `latest_request → "Show me the entry point decompiled"`

This memory is **injected as a message** at position [1] (right after the system prompt) on every LLM call. Even after context trimming removes old messages, the model always sees:

```
[Thread Memory]
TASK: Analyze the malware sample for behavioral indicators
CURRENT: Show me the entry point decompiled

FINDINGS:
- ghidra_analyze: 847 functions, main at 0x401000, Go-compiled
- strings: 1,247 strings, notable: "cmd.exe", "/tmp/.hidden"

Use these findings. Do not repeat completed work.
```

**Why this matters**: With thread memory + sliding window + goal anchoring, a 20B local model can maintain a coherent 5-6+ message tool-calling conversation without a single hallucination. The model stays focused because it's constantly reminded of what it knows and what it should do next.

All extraction is **deterministic** — no LLM call needed. Cloud models don't use thread memory (they handle long context natively).

---

## 4g. Deterministic Artifact Scoping: Precision Analysis

When you click "Analyze" on a specific malware sample, everything about that conversation is scoped to that sample:

- **Context**: Only that sample's artifacts enter the LLM's context window (not every artifact in the project)
- **Tool calls**: When a tool needs an `artifact_id`, the system always injects the target sample — no guessing
- **Generated output**: Artifacts produced during analysis are named with the sample prefix (e.g., `amixer_decompiled.json` instead of generic `decompiled.json`)
- **Parent tracking**: Every generated artifact is linked to its parent sample through the tool run chain

**Why this matters**: Without scoping, a project with 10 samples would have all 10 samples' decompilation results in context, all named `decompiled.json`. The model would inevitably analyze the wrong sample's results. With scoping, it's deterministic — there's only one possible target.

General-purpose threads (not targeting a specific sample) still see all artifacts, grouped under their parent samples with clear attribution.

---

## 4e. Web Gateway: Secure Internet Access for Agents

Agents can fetch web content and search the internet through a secure **web gateway** — a UDS-based daemon that enforces SSRF protection, URL rules, GeoIP blocking, rate limiting, and response caching.

Two tools are provided:
- `web.fetch` — fetch a URL, convert HTML to plaintext, optionally extract links
- `web.search` — search the web via DuckDuckGo, return results with snippets

**Security layers** (evaluated in order):
1. **SSRF protection** — blocks private/reserved IP ranges (IPv4 and IPv6, including IPv6 embedding formats like 6to4, Teredo, NAT64)
2. **GeoIP filtering** — optional country-level blocking via MaxMind database
3. **URL rules** — configurable allow/block lists (domain, domain_suffix, url_prefix, url_regex, ip_cidr)
4. **DNS pinning** — validated IPs are pinned in the HTTP client to prevent DNS rebinding
5. **Redirect re-validation** — full security checks on every redirect hop
6. **Rate limiting** — global and per-user token-bucket rate limits
7. **Response caching** — TTL-based DB cache, automatically purged by the tick command

**Tool restrictions**: Web tools are **restricted by default** — users need an explicit admin grant before agents can use them. This uses a generic restriction system (pattern-based grants: `web.*` matches `web.fetch` and `web.search`) that can be extended to any tools.

A built-in `researcher` agent is registered when the web gateway is available — specialized for internet research with search → fetch → synthesize → cite workflow.

---

## 5. Technical Architecture (IT Manager's Brief)

### Core Infrastructure

| Component | What It Does |
|---|---|
| **af-db** | PostgreSQL backend. Stores threads, messages, artifacts, agents, workflows, audit logs. Runtime SQL queries (no compile-time macros). |
| **af-storage** | Content-addressed blob store. Files are stored by SHA256 hash — immutable, deduplicated. |
| **af-jobs** | Transactional job queue using `FOR UPDATE SKIP LOCKED`. Supports N concurrent workers. |
| **af-llm** | LLM router with OpenAI, Anthropic, and Vertex AI backends. Built-in PII redaction for cloud backends. |
| **af-agents** | Agent runtime: tool-call loop, prompt builder, evidence parser, schema validation. Orchestrator for multi-agent workflows. Automatic context compaction. Meta-tools for thinking threads. |
| **af-auth** | API key authentication (SHA-256 hashed) and role-based access control. |
| **af-api** | HTTP API server with SSE streaming, rate limiting, multipart upload. |
| **Backend Trait** | Abstracts CLI data access. DirectDb for local Postgres, RemoteApi for HTTP API. Supports `--remote` + `--api-key` for headless remote operation. |

### Tool-to-Orchestrator Metadata Channel

Tools don't just return results — they can signal the orchestrator to take action by attaching metadata to output artifacts. This is a compile-time mechanism (Rust code in the tool executor), not something the LLM controls.

Two directives are supported:
- **Repivot** (`repivot_from`): "I produced a better version of this artifact" — orchestrator replaces the original and re-runs the group. Example: a UPX unpacker.
- **Fan-out** (`fan_out_from`): "I extracted N files from this container" — orchestrator creates N child threads, each running the full workflow. Example: a zip extractor.

Both use `ctx.output_store.store_with_metadata()` with helper functions `repivot_metadata()` / `fanout_metadata()` from `af-core`. The orchestrator detects these after each workflow group by querying the `artifacts.metadata` JSONB column.

### Why PostgreSQL?

PostgreSQL serves as the single backend for multiple concerns:

- **Job Queue**: `FOR UPDATE SKIP LOCKED` implements a high-performance, transactional job queue. No jobs are lost if a worker crashes mid-execution.
- **JSONB**: Stores complex tool outputs, agent configurations, and workflow definitions with full query capability.
- **Row-Level Security**: Multi-tenant isolation enforced at the database level — even if application code has a bug, one tenant cannot see another's data.
- **Audit Trail**: Append-only audit log protected by a database trigger that rejects UPDATE and DELETE operations.

### Future: Vector Search (pgvector)

Not yet implemented, but planned: turn decompiled code into vector embeddings and use similarity search to find related malware families. The query "show me functions that share 95% of their logic with this unknown sample" becomes a single SQL query.

---

## 6. Security Model

### The Sandbox (Zero Trust for Tools)

Tools analyze potentially malicious files. They run in **bubblewrap (bwrap) sandboxes** with:

- Empty root filesystem (`--tmpfs /`)
- Only the specific artifact files mounted read-only
- No network access
- All capabilities dropped (`--cap-drop ALL`)
- Process dies if parent dies (`--die-with-parent`)
- **Fail-closed**: if bwrap can't initialize, the tool doesn't run (it doesn't fall back to unsandboxed execution)
- **Explicit override only**: unsandboxed execution requires `AF_ALLOW_UNSANDBOXED=1` (dev/testing)

**Analogy**: if an isolation room loses power, the door should stay locked. If the sandbox has any issue, the system refuses to run the tool.

### Multi-Tenant Isolation

For shared deployments with multiple users:

- **Application-level RBAC**: Every API route checks project membership and role (owner/manager/collaborator/viewer)
- **Postgres Row-Level Security**: Database-enforced tenant isolation. Even a SQL injection in application code can't cross tenant boundaries.
- **Scoped transactions**: Long-running agent loops wrap every DB operation in per-call scoped transactions with RLS context — no "escape hatch" where the agent accidentally queries without tenant filtering.
- **Uniform error messages**: Auth failures return generic "access denied" — no information leakage about what exists.
- **NDA project isolation**: Projects flagged as NDA are excluded from all cross-project queries (family search, IOC pivot, dedup, Ghidra rename suggestions). Every NDA flag change is recorded in the immutable audit log with actor identity and old/new values. The UI shows a blueish tint and NDA badge as a visual reminder when working with NDA-covered projects.

### Per-User Model Access Control

Admins can restrict which LLM models each user is allowed to use:

- No restrictions by default (backward compatible)
- Add one or more allowed routes → user enters allowlist mode
- Supports exact routes (`openai:gpt-4o-mini`), wildcards (`openai:*`), and special values (`auto`, `local`)
- Enforced before route resolution — per-request route overrides are also checked
- API backend listings filtered for restricted users

### Privacy & Air-Gap Readiness

- Local LLM backends (Ollama, vLLM) work without any internet connection
- PII redaction layer scrubs sensitive data before sending to cloud LLMs
- Agents can be forced to `--route local` to guarantee no data leaves the network
- Sandbox blocks network access by default — tools can't phone home

### Chain of Custody

Every finding is traceable:

- Artifacts are content-addressed (SHA256) — immutable, tamper-evident
- Tool outputs are linked to specific `tool_run_id` records with full input/output capture
- Agent messages are tagged with `agent_name` for attribution
- Evidence citations (`evidence:tool_run:<id>`, `evidence:artifact:<id>`) link claims to raw data
- Plugin evidence references are verified against the DB *and project scope* before being stored
- Audit log is append-only (DB trigger rejects mutations)
- NDA flag changes are audited with actor, timestamp, and old/new values

**Analogy**: every sample and lab result is tagged like a patient chart. You can trace it back to see exactly which tool found it, when, and what the raw data looked like. This turns "AI said so" into "here's the proof."

---

## 7. Evolution: From CLI to Platform

| Capability | V2 (Original MVP) | V3 (Multi-Tenant) | V4 (Current) |
|---|---|---|---|
| **Access** | Local CLI only | HTTP API with SSE streaming | Same + config file (`~/.af/config.toml`) |
| **Auth** | OS user | API keys, RBAC (owner/editor/viewer) | RBAC (owner/manager/collaborator/viewer) + @all public |
| **Agent Creation** | Recompile | Dynamic via CLI/API | Same + thinking agents (autonomous orchestration) |
| **Isolation** | Basic sandbox | Hardened bwrap + Postgres RLS | Same + per-user model access control + tool restrictions |
| **Scaling** | Single process | Multi-worker daemon | Same |
| **Orchestration** | One agent | Multi-agent workflows | Workflows + thinking threads (LLM-driven strategy) |
| **Context** | Unbounded (fails on overflow) | Same | Two-tier: LLM compaction (cloud) + thread memory + sliding window (local, zero cost) |
| **Attribution** | None | Agent name tagging | Same + malware family tracking (cross-project search) |
| **Quotas** | None | Per-user LLM tokens + storage | Same + per-user model restrictions |
| **Web Access** | None | None | Web gateway with SSRF protection, URL rules, GeoIP, caching |
| **Audit** | None | Immutable append-only log | Same |
| **CLI Access** | Local only (direct DB) | Same | Local DB or remote API (`--remote` + `--api-key`) |

---

## 8. The Bottom Line

By investing in this architecture, an organization isn't buying a "cyber tool" — it's building **reusable AI infrastructure**. The security model, job queue, multi-tenant isolation, and agent framework are domain-agnostic. Reverse-Arbeiterfarm is the first product. The next one could be a code review workstation, a compliance auditor, or a financial document analyzer — same kitchen, different menu.

The workstation supports three collaboration modes: **single-agent chat** (direct Q&A), **workflows** (predefined multi-agent pipelines), and **thinking threads** (autonomous orchestration where the AI decides the strategy at runtime). Two-tier context management ensures long-running sessions don't hit context limits — cloud models get LLM-based compaction, while local models get zero-cost thread memory and sliding window trimming that enable even a 20B parameter model to maintain coherent multi-turn tool-calling conversations.

Agents can access the internet through a **web gateway** with defense-in-depth security: SSRF protection (including IPv6 embedding formats), DNS pinning to prevent rebinding, full security re-checks on each redirect hop, GeoIP country blocking, configurable URL allow/block rules, and rate limiting. Web tools are restricted by default — users need explicit admin grants, using a generic tool restriction system extensible to any tools.

The CLI can drive a remote Arbeiterfarm server via `--remote` + `--api-key`, enabling headless automation, CI/CD integration, and multi-user workflows without requiring direct database access on the client machine. Per-user model access control and per-tool restriction grants ensure cost and security governance.

Configuration is managed via `~/.af/config.toml` (auto-created with commented defaults), with environment variables available for production overrides.

Note: Some tools are optional and only available when their dependencies are installed (e.g., Rizin, Ghidra, VirusTotal, Web Gateway).

Next steps: see `INSTALL.md` for setup and `USAGE.md` for the CLI/API walkthrough.
