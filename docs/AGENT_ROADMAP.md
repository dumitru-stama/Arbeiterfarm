# Agent Roadmap

Proposed agents and tool families to make Arbeiterfarm a comprehensive multi-agent workstation beyond reverse engineering. All external-service agents follow the existing UDS gateway pattern (dedicated daemon holds credentials, tools communicate via Unix socket, restricted by default).

---

## Communication

### Email agent (`email.*` — in-process) ✅ IMPLEMENTED

| Tool | Description | Calls/run | Timeout |
|---|---|---|---|
| `email.send` | Compose and send (supports dry_run validation) | 5 | 30s |
| `email.draft` | Create draft in provider's drafts folder | 10 | 30s |
| `email.schedule` | Schedule email for future send (processed by `af tick`) | 5 | 15s |
| `email.list_inbox` | List recent emails with summaries | 5 | 30s |
| `email.read` | Fetch full message body + attachment metadata | 10 | 30s |
| `email.reply` | Reply (or reply-all) in-thread with proper headers | 5 | 30s |
| `email.search` | Search emails by query string | 5 | 30s |

- **Architecture**: in-process executors (not UDS gateway) — simpler than originally planned; email doesn't need SSRF protection. Credentials loaded from DB on demand via `email_credentials` table
- **Providers**: Gmail (REST API with OAuth2 token refresh, base64url RFC 2822 messages) and ProtonMail Bridge (lettre SMTP, localhost plain transport; IMAP stubs for future implementation)
- **Recipient restrictions**: DB-backed allowlist/blocklist — exact_email, domain, domain_suffix patterns. Block-wins semantics (identical to web fetch rules). Global + project-scoped. Fail-closed on DB errors
- **Tone presets**: 8 builtins (brief, formal, informal, technical, executive_summary, friendly, urgent, diplomatic) + custom presets. Validated at tool call time, recorded in email_log. Builtins protected from overwrite
- **Scheduling**: `email.schedule` → DB row → `af tick` polls due emails → atomic claim → re-check recipient rules → send → retry on failure (up to max_attempts)
- **Rate limiting**: token-bucket (global + per-user), configurable via `AF_EMAIL_RATE_LIMIT` / `AF_EMAIL_PER_USER_RPM`
- **Security**: `email.*` restricted by default (requires admin grant), Gmail message IDs sanitized (alphanumeric-only), URL parameters encoded, body size limits enforced, credentials never in output/logs
- **Logging**: email_log (operational) + audit_log (immutable) + tracing (structured) — all three fire on every operation
- **Agent**: `email-composer` — `["email.*"]`, budget 20, timeout 300s. Prefers drafts before sending
- **CLI**: `af email setup/accounts/remove-account/tones/scheduled/cancel`, `af email-rule add/remove/list`
- **DB**: migration 030 — 5 tables (email_recipient_rules, email_tone_presets, email_scheduled, email_log, email_credentials), partial unique index for global rules
- **Crate**: `af/crates/af-email/` — 12 source files, 9 unit tests
- **Use cases**: summarize unread emails, draft responses, phishing triage workflows, automated report delivery, scheduled digests

### Notification agent (`notify.*` — in-process) ✅ IMPLEMENTED

| Tool | Description | Calls/run | Timeout |
|---|---|---|---|
| `notify.send` | Send notification to a named channel (webhook, email, matrix, webdav) | 10 | 30s |
| `notify.upload` | Upload artifact to a WebDAV channel | 10 | 30s |
| `notify.list` | List notification channels for the current project | 10 | 30s |
| `notify.test` | Send test notification to verify channel configuration | 10 | 30s |

- **Architecture**: in-process executors (`SandboxProfile::Trusted`) — same pattern as email. New crate `af/crates/af-notify/` with 7 source files. Agents enqueue by channel name; delivery is asynchronous via PostgreSQL queue
- **Channel types**: 4 delivery backends, project-scoped named channels with JSONB config:
  - **Webhook**: POST/PUT to HTTPS URL, custom headers (with blocklist for Host/Content-Length/Cookie etc.), 15s timeout, no redirect following
  - **Email**: placeholder (not yet integrated with af-email; recommends webhook-to-email bridge)
  - **Matrix**: PUT to `/_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}`, percent-encoded room_id, Bearer auth, idempotent via queue ID as txn_id, 15s timeout
  - **WebDAV**: artifact blob upload or text notification as `.txt` file, basic auth, 60s timeout, 256 MB upload limit, filename sanitization
- **Queue**: PostgreSQL-backed state machine (pending → processing → completed/failed/cancelled) with pg_notify trigger for near-real-time delivery. `notification_channels` + `notification_queue` tables (migration 036). PgListener in `af serve` delivers immediately; `af tick` as fallback processor (batch of 20, 2-minute stale recovery)
- **Permanent vs transient errors**: `PermanentError` type for config-level failures (email not integrated, unsupported channel type) — skips retry, sets status to `failed` immediately. Transient failures retry up to `max_attempts=5`
- **Security**: HTTPS enforcement at creation + delivery time, webhook header blocklist (Host/Content-Length/Cookie/Proxy-Authorization), no redirect following, credential leak prevention in error messages, input length validation (channel name 1-100, subject 1-500, body max 100KB), WebDAV filename sanitization, `notify.*` seeded as restricted tool (admin grant required)
- **API**: 8 endpoints under `/projects/{id}/notification-channels` and `/projects/{id}/notifications` — create/list/update/delete channels (Manager+), test channel, list/cancel/retry queue (Viewer+ for list, Manager+ for mutations). Config JSON intentionally omitted from API responses (may contain secrets). Project-scoped DB operations for defense-in-depth
- **Agent**: `notifier` — `["notify.*"]`, budget 10, timeout 120s. Lists channels, sends notifications
- **CLI**: `af notify channel add/list/remove/test`, `af notify queue list/cancel/retry`
- **UI**: Notifications page (`#/notifications`) — project selector, channels table with Test/Delete, add channel dialog with type-specific config fields, queue table with Cancel/Retry
- **DB**: migration 036 — 2 tables (notification_channels, notification_queue), 3 indexes, pg_notify trigger, restricted_tools seed
- **Crate**: `af/crates/af-notify/` — 7 source files (lib.rs, specs.rs, executor.rs, channels.rs, queue.rs, listener.rs)
- **Use cases**: workflow tail step (surface → intel → reporter → **notifier** posts to Slack), automated alert delivery, WebDAV report upload, Matrix team notifications

---

## Knowledge & Documents

### Document agent (`doc.*` — OOP sandboxed) ✅ MOSTLY IMPLEMENTED

| Tool | Description | Status |
|---|---|---|
| `doc.parse` | PDF/DOCX/XLSX/EPUB/HTML/Markdown/CSV/JSON/YAML/TOML/XML → structured text | ✅ Implemented |
| `doc.chunk` | Split large documents into overlapping chunks for embedding | ✅ Implemented |
| `doc.ingest` | All-in-one: parse → chunk → auto-enqueue for background embedding | ✅ Implemented |
| `doc.ocr` | Image or scanned PDF → text (via tesseract) | Not implemented |
| `doc.metadata` | Extract document metadata (author, dates, revision history) | Not implemented |

- **Architecture**: 3 OOP-sandboxed tools in `af-re-tools` crate (`doc_parse.rs`, `doc_chunk.rs`, `doc_ingest.rs`), dispatched by `af-executor`
- **Format detection**: magic bytes + extension-based (PDF magic `%PDF`, DOCX/XLSX/EPUB ZIP magic with manifest sniffing). 11+ formats supported
- **Chunking**: smart boundary detection (paragraph `\n\n` > line `\n` > sentence `. ` > word ` ` > hard cut), 100-10000 byte chunks, configurable overlap, UTF-8 char-boundary safe, max 10,000 chunks
- **Auto-embedding**: `doc.ingest` produces `parsed_text.txt` + `chunks.json`. The OOP executor auto-enqueues `chunks.json` to `embed_queue` when `tool_name.starts_with("doc.")` — background embedding via `af tick`, searchable via `embed.search`
- **Security**: 64 MB max text output, bounded reads for DOCX/EPUB ZIP entries, PDF page range selection
- **Sandbox**: NoNetReadOnly (file access only, no network)
- **Tests**: 14 unit tests (format detection, extraction, chunking boundaries, overlap, UTF-8 safety, infinite loop prevention)
- **Pairs with**: `embed.*` for RAG workflows — parse → chunk → embed → search
- **Use cases**: ingest vendor reports, contracts, datasheets, threat intel PDFs; ask questions via embedding search

### Knowledge base / RAG agent ✅ MOSTLY IMPLEMENTED

**Implemented**:
- **Document tools**: `doc.parse`, `doc.chunk`, `doc.ingest` — full document-to-embedding pipeline
- **URL ingestion pipeline**: `url_ingest_queue` table (migration 035) + `process_url_queue()` processor in `af tick`. Managers submit URLs (API or CLI) → HTTPS fetch (5MB, 30s timeout) → html2text → store text artifact → shared chunking algorithm → store chunks.json → auto-enqueue in `embed_queue` → background embedding → searchable via `embed.search`
- **Shared chunking module**: `af-builtin-tools/src/chunking.rs` — extracted from `doc_chunk.rs` for reuse by URL ingest and embed queue
- **Embed queue management**: Admin API endpoints (`/admin/embed-queue`) + CLI (`af embed-queue list/cancel/retry`) + UI page
- **Knowledge agent TOML**: `examples/local-agents/knowledge.toml` — `["doc.parse", "doc.chunk", "doc.ingest", "embed.*", "file.*", "artifact.describe"]`, budget 25, timeout 600s. Ingestion + question-answering workflow with source citation
- **UI**: Knowledge page combines URL import form + URL ingest queue + embed queue status
- **CLI**: `af url-ingest submit/list/cancel/retry`

**Not yet implemented**:
- `doc.ocr` — image/scanned PDF text extraction (requires tesseract)
- `doc.metadata` — document metadata extraction
- Thinker variant that autonomously decides whether to search embeddings, fetch a URL, or read an artifact
- `web.search` integration in knowledge agent (requires web gateway)

---

## Security & Threat Intelligence

### YARA agent (`yara.*` — OOP sandboxed) ✅ IMPLEMENTED

| Tool | Description |
|---|---|
| `yara.scan` | Run YARA rules against project artifacts, return matches with offsets |
| `yara.generate` | LLM drafts YARA rule from IOCs/decompiled code, validates syntax via `yara -C` |
| `yara.test` | Test a rule against known samples in the project (true/false positive check) |
| `yara.list` | List available rule sets (builtin + user-supplied in `~/.af/yara/`) |

- **Binary**: requires `yara` CLI installed (auto-detected like rizin)
- **Sandbox**: NoNetReadOnly (reads artifacts + rule files only)
- **Use case**: "write a YARA rule for this malware family based on the strings and decompiled functions", then test it against all project artifacts
- **Agent integration**: decompiler + intel agents get `yara.scan`; a dedicated `yara-writer` agent gets all 4 tools

### Local dynamic analysis agent (`sandbox.*` — UDS gateway) ✅ IMPLEMENTED

| Tool | Description |
|---|---|
| `sandbox.trace` | Execute PE in QEMU VM with ~60 default Windows API hooks (Frida), return behavioral trace |
| `sandbox.hook` | Execute PE with custom Frida JavaScript hook script for targeted instrumentation |
| `sandbox.screenshot` | Capture VM display (PPM format, useful for GUI malware — droppers, installers, dialog boxes) |

**Architecture — Frida + QEMU/KVM**:

```
┌──────────────────────────────────────────────────┐
│ Rust host (af)                              │
│ ┌──────────────┐   UDS    ┌────────────────────┐│
│ │ sandbox.*    ├──────────► SandboxGateway      ││
│ │ executors    │           │ ├─ QMP client      ││
│ └──────────────┘           │ └─ Agent TCP client││
│                            └────────┬───────────┘│
└─────────────────────────────────────┼────────────┘
                                      │ QMP + TCP
                              ┌───────▼───────────┐
                              │ QEMU VM (Windows)  │
                              │ ┌─────────────────┐│
                              │ │ agent.py :9111  ││
                              │ │ └─ Frida engine  ││
                              │ └─────────────────┘│
                              └────────────────────┘
```

- **Gateway**: `SandboxGateway` daemon manages VM lifecycle. Serializes operations via `vm_lock`. Before each run, restores clean snapshot via QMP `loadvm`. Communicates with Python guest agent via TCP
- **Guest agent**: `sandbox-agent/agent.py` — Python Frida orchestrator running inside Windows VM on port 9111. Commands: `trace`, `hook`. Spawns target process via Frida, injects hooks, collects trace for `timeout_secs`, kills process
- **Default hooks** (60 Windows APIs in `hooks.rs`): File I/O (CreateFileW/A, ReadFile, WriteFile, DeleteFile, CopyFile), Registry (RegOpenKeyEx, RegSetValueEx, RegQueryValueEx, RegDeleteKey, RegCreateKeyEx), Process (CreateProcess, OpenProcess, TerminateProcess, VirtualAllocEx, WriteProcessMemory, CreateRemoteThread, NtCreateThreadEx), Network (ws2_32 connect/send/recv, WinINet InternetOpen/HttpOpenRequest/HttpSendRequest, URLDownloadToFile, DnsQuery_W), Libraries (LoadLibrary, GetProcAddress, LdrLoadDll), Crypto (CryptEncrypt/Decrypt, CryptHashData, BCryptEncrypt/Decrypt), Services, COM, Memory, Anti-debug (IsDebuggerPresent, CheckRemoteDebuggerPresent, NtQueryInformationProcess, GetTickCount)
- **Hook features**: per-call backtrace (top 3 frames), thread ID logging, bounded trace (max 10,000 entries), string truncation (256 chars), error handling for invalid pointers
- **Snapshot-based isolation**: clean VM snapshot restored before every run — deterministic starting state, no cross-contamination
- **Artifact-first output**: `sandbox.trace` → `trace.json` (full trace), `sandbox.hook` → `hook_results.json`. Inline summary: API call count, unique APIs, top 20 by frequency, process tree
- **Agent preset**: `tracer` — `["sandbox.*", "file.*", "artifact.describe", "family.*"]`, budget 15, timeout 600s. Workflow: file.info → sandbox.trace → analyze trace → sandbox.hook (custom) → family.tag
- **Crate**: `arbeiterfarm/crates/af-re-sandbox/` — gateway.rs, executor.rs, qmp.rs, agent_client.rs, hooks.rs, specs.rs
- **Use cases**: behavioral triage, persistence detection, C2 extraction, encryption key capture, process injection detection, anti-debug evasion analysis

### Sandbox submission agent (external) — NOT IMPLEMENTED

| Tool | Description |
|---|---|
| `sandbox.submit` | Submit artifact to Any.Run / Joe Sandbox / CAPE / Triage |
| `sandbox.poll` | Check job status, return progress |
| `sandbox.report` | Retrieve completed analysis report |
| `sandbox.ingest` | Parse sandbox report → extract behavioral IOCs, tag families, store as artifact |

- **Status**: Deferred — local Frida+QEMU sandbox covers the primary use case. External submission useful for cloud-based ML classification and cross-sandbox correlation, but requires data egress
- **Gateway**: would hold API keys for sandbox services, manage async polling
- **Restriction**: restricted by default (submits potentially sensitive files to third-party services)

### Interactive debugging agent (`dbg.*` — microVM gateway) — NOT IMPLEMENTED

| Tool | Description |
|---|---|
| `dbg.start` | Load artifact into x64dbg inside a microVM, return session ID |
| `dbg.breakpoint` | Set breakpoint by address, symbol, API name, or condition |
| `dbg.run` | Resume execution until breakpoint, exception, or timeout |
| `dbg.step` | Single-step (into/over/out) N instructions, return disassembly + register state |
| `dbg.registers` | Read all registers (GPR, flags, SSE) at current IP |
| `dbg.memory` | Read/dump memory at address (hex + ASCII, up to 64KB per read) |
| `dbg.stack` | Dump stack with annotated return addresses and frame pointers |
| `dbg.trace` | Record execution trace (instruction log) for N steps or until breakpoint |
| `dbg.modules` | List loaded modules with base addresses and sizes |
| `dbg.strings` | Extract strings from memory region (heap, stack, specific module) |
| `dbg.callstack` | Walk call stack with symbol resolution |
| `dbg.watchpoint` | Set hardware watchpoint on memory address (read/write/execute) |
| `dbg.script` | Execute an x64dbg script (.txt) or command sequence |
| `dbg.screenshot` | Capture VM display (covered by `sandbox.screenshot` for now) |
| `dbg.stop` | Terminate debug session and destroy microVM |

- **Status**: Deferred — high value but high effort. Requires x64dbg plugin (C++), Windows guest agent (C#/C++), virtio-serial transport, session lifecycle management. The existing `sandbox.hook` with custom Frida scripts covers ~90% of dynamic analysis use cases at API level
- **Use cases** (unique to instruction-level debugging): step through decryption loops, dump registers at specific offsets, set conditional breakpoints on memory writes, trace execution flow through packed/obfuscated code
- **Architecture**: reuses existing QEMU infrastructure from `sandbox.*`, adds x64dbg + dbg-agent.exe inside guest, JSON-RPC over virtio-serial

### MITRE ATT&CK mapping agent (`attack.*` — Trusted in-process)

| Tool | Description |
|---|---|
| `attack.lookup` | Search ATT&CK by technique ID, name, or keyword |
| `attack.map` | Given a list of behaviors, return matching technique IDs + tactics |
| `attack.navigator` | Generate ATT&CK Navigator layer JSON for visualization |

- **Data source**: local ATT&CK STIX/JSON database (downloaded at setup, refreshed periodically)
- **No gateway needed**: pure in-process lookups against local data
- **Use case**: reporter agent auto-populates ATT&CK table in final reports; intel agent maps IOCs to techniques
- **Agent integration**: reporter + intel agents get `attack.lookup` + `attack.map`

### SIEM / log query agent (`siem.*` — UDS gateway)

| Tool | Description |
|---|---|
| `siem.query` | Run Splunk SPL / Elasticsearch DSL / Sigma queries |
| `siem.alert_context` | Given an alert ID, pull surrounding events and timeline |
| `siem.count` | Count matching events (lightweight, for triage) |
| `siem.export` | Export query results as artifact (CSV/JSON) |

- **Gateway**: holds SIEM API credentials, enforces query timeout and result limits
- **Restriction**: heavily restricted — admin grants per-user, possibly per-index
- **Use case**: "find all network connections to this C2 domain in the last 30 days", "pull the full alert timeline for incident INC-2024-1337"
- **Multi-backend**: gateway config selects Splunk vs Elastic vs generic HTTP

---

## Code & DevOps

### Source code audit agent (`code.*` — OOP sandboxed)

| Tool | Description |
|---|---|
| `code.clone` | Shallow-clone a git repo into project as artifact |
| `code.search` | ripgrep / tree-sitter search across source files |
| `code.diff` | Show diff between commits, branches, or tags |
| `code.audit` | LLM-driven vulnerability pattern search (SQL injection, XSS, hardcoded creds) |
| `code.tree` | Directory tree with file sizes and language detection |

- **Sandbox**: NoNetReadOnly for search/diff/audit; NetworkAllowed for clone (or pre-clone via gateway)
- **Use case**: "audit this repo for OWASP top 10 vulnerabilities", "find all uses of eval() in the JavaScript files"
- **Agent preset**: `code-auditor` with `code.*` + `file.*` tools

### Container / image agent (`container.*` — OOP sandboxed)

| Tool | Description |
|---|---|
| `container.inspect` | Extract layers, entrypoint, env vars, labels from OCI/Docker images |
| `container.extract` | Unpack filesystem layers as artifacts (feeds into fan-out pipeline) |
| `container.sbom` | Generate SBOM via syft or trivy |
| `container.vulnscan` | Run vulnerability scan against image (trivy/grype) |

- **Sandbox**: NoNetReadOnly for inspect/extract; NetworkAllowed for sbom/vulnscan (needs vuln DB)
- **Fan-out integration**: extracted layers → child artifacts → full analysis pipeline
- **Use case**: "analyze this Docker image for supply chain risks"

### CI/CD agent (`ci.*` — UDS gateway)

| Tool | Description |
|---|---|
| `ci.trigger` | Kick off a build/scan pipeline (Jenkins, GitLab, GitHub Actions) |
| `ci.status` | Poll job status and stage progress |
| `ci.logs` | Retrieve build logs (store as artifact) |
| `ci.artifacts` | Download build artifacts into project |

- **Gateway**: holds CI API tokens, manages async polling
- **Restriction**: restricted by default (triggers external builds)
- **Use case**: "rebuild the firmware after patching", "download the latest nightly build artifacts"

---

## Data & Automation

### Database query agent (`db.*` — UDS gateway)

| Tool | Description |
|---|---|
| `db.query` | Run read-only SQL against a configured external database |
| `db.schema` | List tables, columns, types, row counts |
| `db.export` | Export query results as CSV/JSON artifact |

- **Gateway**: separate connection pool, **read-only user enforced**, query timeout (30s default)
- **Restriction**: restricted by default, admin grants per-user and per-database
- **Safety**: gateway validates queries are SELECT-only (rejects INSERT/UPDATE/DELETE/DROP/TRUNCATE)
- **Use case**: "find all users who logged in from this IP range", "export the transaction log for account X"

### Scheduler agent (extends existing hooks/tick)

| Tool | Description |
|---|---|
| `schedule.create` | Register a periodic job (cron expression + action) |
| `schedule.list` | List active schedules for a project |
| `schedule.cancel` | Cancel a scheduled job |
| `schedule.history` | Show execution history for a schedule |

- **DB-backed**: `scheduled_jobs` table, tick command checks and fires due jobs
- **Actions**: re-scan VT, re-run workflow, fetch URL, send notification
- **Restriction**: restricted by default (creates recurring automated actions)
- **Use case**: "re-check VirusTotal for this hash every 24 hours and notify me when detection count changes"
- **Already partially exists**: hooks + tick provide the foundation; this formalizes it as user-facing tools

### Data transform agent (`transform.*` — OOP sandboxed) ✅ IMPLEMENTED

| Tool | Description |
|---|---|
| `transform.decode` | Base64/base64url/hex/URL/XOR decode, gzip/zlib/bzip2 decompress. Decompression bomb protection (256 MB bounded reads) |
| `transform.unpack` | Extract ZIP (with password), tar, tar.gz, tar.bz2, 7z archives. Pre-flight size validation, path traversal protection, configurable file/size limits |
| `transform.jq` | Apply jq expressions to JSON via `jaq-interpret` with full standard library (~50 native builtins + jq prelude). 64 MB output cap |
| `transform.csv` | Parse (→ JSON), filter (regex on column), stats (column statistics). Configurable delimiter and header detection |
| `transform.convert` | Bidirectional conversion: JSON ↔ YAML ↔ TOML ↔ XML via serde |
| `transform.regex` | Extract patterns with named capture groups. 2 MB regex compiled size limit, configurable max_matches |

- **Architecture**: 6 pure-Rust modules in `af-re-tools` crate, all in `af-executor` OOP binary. No external binaries needed
- **Sandbox**: NoNetReadOnly (pure data transformation, no network, no writes outside scratch_dir)
- **Security**: decompression bomb protection (256 MB `.take()` limit), archive bomb protection (pre-flight declared size validation for 7z, per-file + total byte limits), path traversal rejection, regex DoS prevention (2 MB compiled size limit), jq output cap (64 MB), XOR key validation (hex-only, max 256 bytes)
- **Agent**: `transformer` — `["transform.*", "file.*", "artifact.describe"]`, budget 20, timeout 600s. Chains file inspection → transform → result examination. `transform.decode` also added to surface/decompiler/intel agents
- **Use case**: decode obfuscated payloads, extract archives, parse config files, query JSON data, extract structured data from logs
- **Crate dependencies**: base64, flate2, bzip2, tar, zip, sevenz-rust, csv, serde_yml, quick-xml, jaq-interpret, jaq-parse, jaq-syn

---

## Priority

Ordered by value-to-effort ratio, considering what's already built:

### Completed

| # | Agent | Status | Summary |
|---|---|---|---|
| 1 | **Email** ✅ | Done | 7 tools, Gmail + ProtonMail, recipient rules, tone presets, scheduling, rate limiting |
| 2 | **YARA** ✅ | Done | 4 tools (scan, generate, test, list), OOP sandboxed, yara-writer agent preset |
| 3 | **Data transform** ✅ | Done | 6 tools (decode, unpack, jq, csv, convert, regex), pure Rust, security-hardened, transformer agent preset |
| 4 | **Sandbox (local)** ✅ | Done | 3 tools (trace, hook, screenshot), Frida + QEMU/KVM, 60 default API hooks, tracer agent preset |
| 5 | **Document / RAG** ✅ | Mostly done | 3 doc tools (parse, chunk, ingest) + URL ingestion pipeline + embed queue + knowledge agent TOML. Remaining: doc.ocr, doc.metadata, web.search integration |
| 6 | **Notification** ✅ | Done | 4 tools (send, upload, list, test), webhook/matrix/webdav channels, PostgreSQL queue + PgListener near-real-time delivery, notifier agent preset |

### Next up — ranked by value-to-effort

| # | Agent | Effort | Value | Why next? |
|---|---|---|---|---|
| 7 | **MITRE ATT&CK** | Low | Medium | Grounds LLM output in structured framework. Pure in-process lookups against local STIX/JSON data — no gateway, no credentials. Immediately improves report quality by adding ATT&CK technique IDs |
| 8 | **Scheduler** | Low | Medium | Formalizes existing hooks/tick as user-facing tools. Foundation already built; this is mostly a thin wrapper exposing schedule.create/list/cancel/history to agents |
| 9 | **Source code audit** | Medium | Medium | Extends Arbeiterfarm to AppSec domain. Core tools (clone, search, diff, tree) are straightforward OOP; `code.audit` is LLM-driven pattern matching using existing infrastructure |
| 10 | **Container/image** | Medium | Medium | Supply chain security. Leverages existing fan-out architecture for layer extraction → analysis pipeline |
| 11 | **Database query** | Medium | Medium | Cross-domain utility. Needs careful security (read-only enforcement, query validation) |
| 12 | **Sandbox (external)** | Medium | Medium | Any.Run/Joe/CAPE submission — deferred, local sandbox covers primary use case |
| 13 | **SIEM / log query** | High | High | High value for SOC teams, but complex multi-backend gateway (Splunk/Elastic/Sigma) |
| 14 | **CI/CD** | Medium | Low | Niche use case, mostly useful for firmware RE workflows |
| 15 | **Debugging** (`dbg.*`) | High | Very High | Interactive x64dbg in microVM — highest value but highest effort. `sandbox.hook` covers ~90% of dynamic analysis use cases |

---

## Implementation patterns

All new agents follow established patterns:

- **UDS gateway** (sandbox, siem, ci, db): separate daemon process, tools communicate via Unix socket, credentials never reach LLM. Same architecture as VT and web gateways.
- **In-process with DB credentials** (email, notify): executors run in the API server process, credentials stored in DB per-user/per-project, loaded on demand. Simpler than UDS gateway when SSRF protection is not needed. Notification channels use asynchronous PostgreSQL queue with PgListener for near-real-time delivery.
- **MicroVM gateway** (dbg): UDS gateway that manages VM lifecycle. Guest agent communicates via virtio-serial/vsock. Snapshot-based fast boot, network-isolated, resource-capped.
- **OOP sandboxed** (doc, yara, code, container, transform): bwrap sandbox with selective bind mounts. NoNetReadOnly for file-only tools, NetworkAllowed where needed.
- **Trusted in-process** (attack, schedule): direct DB or local data access via `ScopedPluginDb`, no sandbox overhead.
- **Tool restrictions**: all external-service tools (`email.*`, `notify.*`, `sandbox.*`, `siem.*`, `ci.*`, `db.*`, `dbg.*`) seeded as restricted in migration, require admin grant per-user.
- **TOML agents**: each tool family ships with a preset agent TOML in `examples/local-agents/`.
