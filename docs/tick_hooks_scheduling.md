# Tick, Hooks & Scheduling Systems

This document provides a detailed reference for Arbeiterfarm's background processing infrastructure: the **tick system** (periodic maintenance and queue processing), the **hooks system** (event-driven automation), and the **scheduling/queue systems** (email delivery, URL ingestion, embedding, notifications).

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [The Tick System](#the-tick-system)
   - [Invoking Tick](#invoking-tick)
   - [Tick Operations (in order)](#tick-operations-in-order)
   - [Error Handling Philosophy](#error-handling-philosophy)
3. [The Hooks System](#the-hooks-system)
   - [Schema](#hooks-schema)
   - [Event Types](#event-types)
   - [Template Variables](#template-variables)
   - [Execution Flow](#hook-execution-flow)
   - [CAS-Based Tick Claiming](#cas-based-tick-claiming)
   - [Safety: Anti-Recursion](#hook-safety-anti-recursion)
   - [API Endpoints](#hooks-api-endpoints)
   - [CLI Commands](#hooks-cli-commands)
   - [Examples](#hooks-examples)
4. [Background Queue Systems](#background-queue-systems)
   - [Shared Patterns](#shared-patterns)
   - [Email Scheduling](#email-scheduling)
   - [URL Ingestion Queue](#url-ingestion-queue)
   - [Embedding Queue](#embedding-queue)
   - [Notification Queue](#notification-queue)
5. [Job Queue (Tool Execution)](#job-queue-tool-execution)
   - [Claiming with FOR UPDATE SKIP LOCKED](#claiming-with-for-update-skip-locked)
   - [Heartbeat and Lease](#heartbeat-and-lease)
   - [Reaper](#reaper)
6. [Concurrency and Safety](#concurrency-and-safety)
7. [Configuration Reference](#configuration-reference)
8. [Operational Examples](#operational-examples)

---

## Architecture Overview

Arbeiterfarm uses a **cron-driven tick model** rather than persistent background daemons. All periodic work is performed by a single CLI command:

```
af tick
```

This command is designed to be called from cron every minute. Each invocation performs a fixed sequence of maintenance tasks, processes background queues, and fires any due hooks. The process exits after completing all work.

```
┌──────────────────────────────────────────────────────────┐
│                    af tick                           │
│                                                          │
│  1. Cache purge (web fetch)                              │
│  2. Scratch dir cleanup                                  │
│  3. Blob garbage collection                              │
│  4. Thread/message TTL purge                             │
│  5. Scheduled email delivery                             │
│  6. URL ingestion queue processing                       │
│  7. Embedding queue processing                           │
│  8. Notification queue processing                         │
│  9. Project hooks (tick events)                           │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

Separately, the **job queue** (tool execution) uses a persistent worker process with `FOR UPDATE SKIP LOCKED` claiming, heartbeats, and a reaper for stale leases. This runs alongside the API server, not via tick.

---

## The Tick System

**Source file**: `af/crates/af-cli/src/commands/tick.rs`

### Invoking Tick

```bash
# Run once (typical usage from cron)
af tick

# Crontab entry (every minute)
* * * * * /usr/local/bin/af tick
```

Tick establishes a database connection pool, performs all 9 operations sequentially, then exits. The process is designed to be short-lived and idempotent — running it twice in quick succession is safe.

### Tick Operations (in order)

#### 1. Web Fetch Cache Purge

```
[tick] purged 42 expired cache entries
```

Removes expired entries from the web fetch response cache. This is cheap and idempotent.

- **Function**: `af_db::web_fetch::cache_purge_expired(&pool)`
- **Frequency**: Every tick
- **Impact**: Low — small DELETE on indexed column

#### 2. Scratch Directory Cleanup

```
[tick] removed 3 stale scratch dir(s)
```

Removes orphaned scratch directories left behind by crashed workers. Only directories older than 2 hours are removed.

- **Function**: `af_storage::scratch::cleanup_stale_dirs(scratch_root, Duration::from_secs(7200))`
- **Threshold**: 2 hours (7200 seconds)
- **Impact**: Low — filesystem cleanup only

#### 3. Blob Garbage Collection

```
[tick] blob GC: 5 removed, 1 re-referenced, 0 file error(s)
```

Two-phase garbage collection for content-addressed blobs:

1. **Phase 1 — Find candidates**: `af_db::blobs::find_unreferenced_blobs(&pool)` scans for blobs not referenced by any artifact.
2. **Phase 2 — Delete with re-check**: For each candidate:
   - Delete the file from disk (`delete_blob_file`)
   - Atomically delete the DB row only if still unreferenced (`delete_blob_if_unreferenced`)
   - This double-check prevents TOCTOU races where a concurrent `store_blob` references the blob between the scan and delete.

If the file delete fails, the DB row is preserved (skip that blob). If the blob was re-referenced between scan and delete, it's silently skipped (`re-referenced` counter).

#### 4. Thread/Message TTL Purge

```
[tick] purged 12 expired thread(s)
```

Deletes threads past their project's configured retention period.

- **Function**: `af_db::threads::purge_expired_threads(&pool)`
- **Cascade**: Thread deletion also cascades to messages and thread memory (via CTE)

#### 5. Scheduled Email Delivery

```
[tick] sent 3 scheduled email(s)
```

Processes due scheduled emails. See [Email Scheduling](#email-scheduling) for full details.

- **Function**: `af_email::scheduler::process_due_emails(&pool)`
- **Batch limit**: 50 emails per tick
- **Providers**: Gmail (REST API) and ProtonMail (SMTP)
- **Safety**: Recipient rules re-checked at send time (fail-closed)

#### 6. URL Ingestion Queue

```
[tick] ingested 2 URL(s)
```

Fetches pending URLs, converts HTML to text, chunks for embedding. See [URL Ingestion Queue](#url-ingestion-queue) for full details.

- **Function**: `af_builtin_tools::url_ingest::process_url_queue(&pool, &storage_root)`
- **Batch limit**: 5 URLs per tick (URL fetching is slow)
- **Output**: Text artifact + chunks artifact, auto-enqueued for embedding

#### 7. Embedding Queue

```
[tick] embedded 4 queued chunk set(s)
```

Embeds pending chunk sets via the configured embedding backend (Ollama/OpenAI). See [Embedding Queue](#embedding-queue) for full details.

- **Function**: `af_builtin_tools::embed_queue::process_embed_queue(&pool, &backend)`
- **Prerequisite**: Only runs if `AF_EMBEDDING_ENDPOINT` is configured
- **Batch limit**: 10 items per tick, 100 chunks per batch within each item

#### 8. Notification Queue

```
[tick] delivered 3 notification(s)
```

Delivers pending notifications via configured channels (webhook, email, matrix, webdav). See [Notification Queue](#notification-queue) for full details.

- **Function**: `af_notify::queue::process_notification_queue(&pool, &storage_root)`
- **Batch limit**: 20 items per tick (notifications are fast)
- **Stale recovery**: Items stuck in `processing` for > 2 minutes
- **Delivery**: Webhook (POST), email (Gmail/ProtonMail), Matrix (HTTP PUT), WebDAV (HTTP PUT)
- **Near-real-time**: Also delivered via PgListener in `af serve` (tick is fallback)

#### 9. Project Hooks (Tick Events)

```
[tick] 3 due hook(s), firing...
[tick] done
```

Fires all due tick hooks across all projects. See [The Hooks System](#the-hooks-system) for full details.

- **Function**: `af_api::hooks::fire_tick_hooks_blocking(&state)`
- **Prerequisite**: At least one LLM backend must be configured
- **Execution**: Hooks fire in parallel (one tokio task per hook), tick waits for all to complete

### Error Handling Philosophy

Every tick operation follows the same pattern: **warn and continue**. A failure in one operation never blocks subsequent operations.

```rust
match some_operation(&pool).await {
    Ok(n) if n > 0 => println!("[tick] did {n} thing(s)"),
    Ok(_) => {}  // nothing to do — silent
    Err(e) => eprintln!("[tick] WARNING: operation failed: {e}"),
}
```

- Success with results: printed to stdout with `[tick]` prefix
- Success with nothing to do: silent
- Failure: printed to stderr as WARNING, execution continues

The only exception is hooks: if no LLM backend is configured and there are due hooks, tick returns an error (hooks require an LLM to execute agents/workflows).

---

## The Hooks System

Hooks are event-driven automation rules attached to projects. When an event occurs (artifact uploaded, periodic tick), the hook fires a workflow or agent with a templated prompt.

### Hooks Schema

**Migration**: `af/crates/af-db/migrations/020_project_hooks.sql`

```sql
CREATE TABLE IF NOT EXISTS project_hooks (
    id                    UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id            UUID NOT NULL REFERENCES projects(id),
    name                  TEXT NOT NULL,
    enabled               BOOLEAN NOT NULL DEFAULT true,
    event_type            TEXT NOT NULL CHECK (event_type IN ('artifact_uploaded', 'tick')),
    workflow_name         TEXT,
    agent_name            TEXT,
    prompt_template       TEXT NOT NULL,
    route_override        TEXT,
    tick_interval_minutes INTEGER CHECK (tick_interval_minutes > 0),
    last_tick_at          TIMESTAMPTZ,
    tick_generation       BIGINT NOT NULL DEFAULT 0,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Exactly one target: workflow XOR agent
    CONSTRAINT chk_hook_target CHECK (
        (workflow_name IS NOT NULL AND agent_name IS NULL)
        OR (workflow_name IS NULL AND agent_name IS NOT NULL)
    ),

    -- Tick hooks must have an interval
    CONSTRAINT chk_tick_interval CHECK (
        event_type != 'tick' OR tick_interval_minutes IS NOT NULL
    ),

    UNIQUE (project_id, name)
);
```

Key constraints:
- **XOR target**: A hook targets either a workflow or an agent, never both
- **Tick interval required**: Tick hooks must specify `tick_interval_minutes`
- **Unique name**: Hook names are unique within a project
- **Partial index**: `idx_project_hooks_tick` covers only enabled tick hooks for fast due-hook lookups

### Event Types

#### `artifact_uploaded`

Fires when a user uploads an artifact to the project. Only fires on user-initiated uploads (API/CLI), **not** on tool-produced artifacts (see [Safety: Anti-Recursion](#hook-safety-anti-recursion)).

Available template variables:

| Variable | Type | Description |
|----------|------|-------------|
| `{{artifact_id}}` | UUID | The uploaded artifact's ID |
| `{{filename}}` | String | Original filename |
| `{{sha256}}` | String | Content hash |
| `{{project_id}}` | UUID | Project ID |
| `{{project_name}}` | String | Project name |

#### `tick`

Fires periodically based on `tick_interval_minutes`. The hook is due when:
- It has never fired (`last_tick_at IS NULL`), **or**
- Enough time has elapsed: `last_tick_at + interval <= now()`

Available template variables:

| Variable | Type | Description |
|----------|------|-------------|
| `{{project_id}}` | UUID | Project ID |
| `{{project_name}}` | String | Project name |
| `{{hook_name}}` | String | The hook's name |
| `{{tick_count}}` | Integer | Number of times this hook has fired (generation + 1) |

### Template Variables

Template expansion uses simple `{{variable}}` replacement:

```rust
fn expand_template(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{{{key}}}}}"), value);
    }
    result
}
```

Example template:
```
Analyze the newly uploaded file {{filename}} (SHA256: {{sha256}}).
Focus on identifying malware families and IOCs.
```

### Hook Execution Flow

**Source file**: `af/crates/af-api/src/hooks.rs`

When a hook fires, the execution engine:

1. **Creates a dedicated thread** for the hook execution, tagged with `hook:{hook_name}`:
   ```rust
   af_db::threads::create_thread_typed(
       &state.pool, hook.project_id, agent_name,
       Some(&format!("hook:{}", hook.name)), thread_type,
   )
   ```

2. **Determines thread type**:
   - Workflow hooks → `"workflow"` thread type
   - Agent hooks with `internal.*` tools → `"thinking"` thread type
   - Other agent hooks → `"agent"` thread type

3. **Writes an audit log entry** (non-blocking, spawned as background task)

4. **Executes the target**:
   - **Workflow target**: Creates an `OrchestratorRuntime`, executes the workflow. Results are drained (no streaming to clients).
   - **Agent target**: Creates an `AgentRuntime`, sends the expanded prompt as a message.

5. **Route override**: If `route_override` is set on the hook, it's propagated to the workflow/agent runtime (e.g., `openai:gpt-4o`).

#### Blocking vs Non-Blocking

Two variants exist for each event type:

| Function | Behavior | Used By |
|----------|----------|---------|
| `fire_artifact_hooks()` | Non-blocking (spawn and return) | API server (upload handler) |
| `fire_artifact_hooks_blocking()` | Waits for all hooks to complete | CLI upload |
| `fire_tick_hooks()` | Non-blocking | API server (if needed) |
| `fire_tick_hooks_blocking()` | Waits for all hooks to complete | `af tick` CLI |

#### Max Hooks Per Event

A safety cap of **10 hooks per event** (`MAX_HOOKS_PER_EVENT`) prevents hook storms from misconfigured projects. If a project has more hooks for an event, only the first 10 fire and a warning is logged.

### CAS-Based Tick Claiming

Tick hooks use Compare-And-Swap (CAS) on `tick_generation` to prevent double-firing when multiple tick processes run concurrently (e.g., cron overlap):

```sql
UPDATE project_hooks
SET last_tick_at = now(),
    tick_generation = tick_generation + 1,
    updated_at = now()
WHERE id = $1 AND tick_generation = $2
```

The flow:
1. `list_due_tick_hooks()` fetches all due hooks with their current `tick_generation`
2. For each hook, `claim_tick(hook_id, expected_generation)` attempts the CAS
3. If `rows_affected > 0`, the claim succeeded — this process fires the hook
4. If `rows_affected == 0`, another process already claimed it — skip

This is safe under concurrent tick executions: exactly one process claims each hook.

### Hook Safety: Anti-Recursion

Hooks fire **only** on user-initiated uploads, **not** on tool-produced artifacts. This prevents indirect recursion:

```
hook → workflow → tool → produces artifact → hook → workflow → ... (infinite loop)
```

The safety boundary is architectural:
- **User uploads** go through the API route or CLI, which calls `fire_artifact_hooks()`
- **Tool outputs** call `af_db::artifacts::create_artifact()` directly, bypassing hook firing

This is documented in the hooks module header:
```rust
//! Safety: hooks fire ONLY on user-initiated uploads (API route + CLI), NOT on
//! tool-produced artifacts. Tools create artifacts via `af_db::artifacts::create_artifact()`
//! directly, bypassing the hook-firing path.
```

### Hooks API Endpoints

All endpoints require Bearer authentication. Hook management requires **Manager+** (Action::Write) access to the project.

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/api/v1/projects/{id}/hooks` | Write | Create a new hook |
| `GET` | `/api/v1/projects/{id}/hooks` | Read | List all hooks for project |
| `GET` | `/api/v1/hooks/{id}` | Read | Get a single hook |
| `PUT` | `/api/v1/hooks/{id}` | Write | Update hook (enable/disable, change template, etc.) |
| `DELETE` | `/api/v1/hooks/{id}` | Write | Delete a hook |

#### Create Hook Request

```json
{
  "name": "auto-analyze",
  "event_type": "artifact_uploaded",
  "workflow_name": "full-analysis",
  "prompt_template": "Analyze {{filename}} (artifact: {{artifact_id}})",
  "route_override": "openai:gpt-4o"
}
```

Validation rules:
- `event_type` must be `"artifact_uploaded"` or `"tick"`
- Exactly one of `workflow_name` or `agent_name` is required
- Tick hooks require `tick_interval_minutes > 0`
- `name` must be 1-200 characters
- Referenced workflow/agent must exist in the database

#### Update Hook Request

```json
{
  "enabled": false,
  "prompt_template": "New prompt: {{filename}}",
  "tick_interval_minutes": 60,
  "route_override": null
}
```

All fields are optional. Setting `route_override` to `null` removes the override.

### Hooks CLI Commands

```bash
# List hooks for a project
af hook list --project <PROJECT_ID>

# Create an artifact_uploaded hook
af hook create --project <PROJECT_ID> \
  --name "auto-triage" \
  --event artifact_uploaded \
  --workflow full-analysis \
  --template "Triage {{filename}}"

# Create a tick hook (fires every 60 minutes)
af hook create --project <PROJECT_ID> \
  --name "hourly-scan" \
  --event tick \
  --agent researcher \
  --interval 60 \
  --template "Check for new threats related to project {{project_name}}"

# Update a hook
af hook update <HOOK_ID> --enabled false
af hook update <HOOK_ID> --template "New template: {{filename}}"
af hook update <HOOK_ID> --interval 120

# Delete a hook
af hook delete <HOOK_ID>
```

### Hooks Examples

#### Example 1: Auto-Analyze Uploaded Samples

Automatically run the full analysis workflow when a new binary is uploaded:

```bash
af hook create --project $PROJECT_ID \
  --name "auto-analyze" \
  --event artifact_uploaded \
  --workflow full-analysis \
  --template "Analyze the uploaded file {{filename}} (SHA256: {{sha256}}). \
Identify the architecture, extract strings, decompile key functions, and search for IOCs."
```

When a user uploads `malware.exe`, the hook:
1. Creates a new thread tagged `hook:auto-analyze`
2. Expands the template with the artifact details
3. Runs the `full-analysis` workflow (surface → decompiler → intel → reporter)
4. Results are stored in the thread for later review

#### Example 2: Periodic Threat Intelligence Check

Every 4 hours, query for new threat intelligence:

```bash
af hook create --project $PROJECT_ID \
  --name "threat-intel-refresh" \
  --event tick \
  --agent researcher \
  --interval 240 \
  --route "openai:gpt-4o" \
  --template "This is tick #{{tick_count}} for project {{project_name}}. \
Search the web for any new threat intelligence, CVEs, or IOCs related to \
the malware families tracked in this project. Summarize findings."
```

#### Example 3: Daily Summary Report

Generate a daily summary using the reporter agent:

```bash
af hook create --project $PROJECT_ID \
  --name "daily-summary" \
  --event tick \
  --agent reporter \
  --interval 1440 \
  --template "Generate a daily summary report for project {{project_name}}. \
List all artifacts analyzed in the last 24 hours, key findings, and any \
new IOCs or family attributions."
```

---

## Background Queue Systems

### Shared Patterns

All four background queues (email, URL ingest, embed, notification) follow the same state machine and concurrency pattern:

#### State Machine

```
pending ──→ processing ──→ completed
  ↑              │
  │              ↓
  └─── (retry) ─── failed ──→ (manual retry) ──→ pending
                     │
                     └──→ cancelled
```

States:
- **pending**: Waiting to be processed
- **processing**: Claimed by a worker, actively being processed
- **completed**: Successfully processed
- **failed**: Permanently failed after exhausting retries
- **cancelled**: Manually cancelled by a user

#### Atomic Claiming

Every queue uses atomic claiming to prevent duplicate processing when multiple tick processes run concurrently:

```sql
UPDATE queue_table SET status = 'processing', updated_at = NOW()
WHERE id = $1 AND status = 'pending'
```

The `WHERE status = 'pending'` clause ensures only one worker can claim an item. If `rows_affected == 0`, another worker already claimed it.

#### Stale Recovery

If a tick process crashes while processing an item (leaving it stuck in `processing`), the next tick recovers it:

```sql
UPDATE queue_table SET
    attempt_count = attempt_count + 1,
    status = CASE
        WHEN attempt_count + 1 >= max_attempts THEN 'failed'
        ELSE 'pending'
    END,
    error_message = CASE
        WHEN attempt_count + 1 >= max_attempts
        THEN 'permanently failed: exceeded max retries after stale recovery'
        ELSE 'recovered from stale processing state'
    END,
    updated_at = NOW()
WHERE status = 'processing'
  AND updated_at < NOW() - make_interval(mins := $1)
RETURNING *
```

Default stale threshold: **2 minutes** for URL ingest, embed, and notification queues.

#### Retry Logic

On failure, `attempt_count` is incremented. If under `max_attempts`, the item resets to `pending` for retry. If at or above `max_attempts`, it's permanently marked `failed`.

Manual retry (via CLI or API) resets both `status` and `attempt_count`:

```sql
UPDATE queue_table SET status = 'pending', error_message = NULL,
    attempt_count = 0, updated_at = NOW()
WHERE id = $1 AND status = 'failed'
```

### Email Scheduling

**Source files**:
- Scheduler: `af/crates/af-email/src/scheduler.rs`
- DB layer: `af/crates/af-db/src/email.rs` (`email_scheduled` table)

#### How It Works

1. An agent calls `email.schedule` with a future send time
2. The email is stored in the `email_scheduled` table with status `pending`
3. `af tick` calls `process_due_emails()` which:
   - Lists up to 50 due emails (`send_at <= NOW()` and status = `pending`)
   - For each email:
     a. **Atomically claims** it (`claim_scheduled` — pending → processing)
     b. **Loads credentials** for the user's configured provider
     c. **Re-checks recipient rules** at send time (fail-closed if rules can't be loaded)
     d. **Sends** via the appropriate provider (Gmail REST API or ProtonMail SMTP)
     e. **Marks completed** or **fails** with error message
     f. **Logs** to both `email_log` (operational) and `audit_log` (compliance)

#### Recipient Rule Re-Check

A critical safety feature: recipient rules are re-checked at send time, not just at scheduling time. This means if an admin blocks a recipient after an email was scheduled, the email will be rejected:

```rust
// Re-check recipient rules — rules may have changed since scheduling time
match af_db::email::list_recipient_rules(pool, None, Some(scheduled.project_id)).await {
    Ok(rules) => {
        if let Err(reason) = recipient_rules::evaluate_all_recipients(&to, &cc, &bcc, &rules) {
            // BLOCKED — fail the scheduled email
        }
    }
    Err(e) => {
        // Fail-closed: cannot load rules = reject send for safety
    }
}
```

#### Example: Scheduling an Email

```bash
# Via the email-composer agent:
# "Schedule a report email for Monday at 9am"

# The agent calls email.schedule which stores to DB.
# On Monday at 9am, the next `af tick` picks it up and sends it.
```

### URL Ingestion Queue

**Source files**:
- Processor: `af/crates/af-builtin-tools/src/url_ingest.rs`
- DB layer: `af/crates/af-db/src/url_ingest.rs`
- Migration: `af/crates/af-db/migrations/035_url_ingest_queue.sql`
- API routes: `af/crates/af-api/src/routes/url_ingest.rs`

#### Pipeline

```
URL submitted ──→ pending ──→ claim ──→ fetch ──→ html→text ──→ chunk
                                                                  │
              ┌───────────────────────────────────────────────────┘
              ↓
        store text artifact ──→ store chunks artifact ──→ enqueue for embedding ──→ complete
```

#### Processing Steps (per URL)

1. **Recover stale**: Items stuck in `processing` for > 2 minutes are recovered
2. **List pending**: Up to 5 per tick (URL fetching is slow)
3. **Claim atomically**: `pending → processing`
4. **Validate URL**: Must be `http://` or `https://` scheme
5. **Auto-upgrade**: `http://` → `https://`
6. **Fetch with reqwest**:
   - 30-second timeout
   - Max 10 redirects
   - User-Agent: `af-url-ingest/1.0`
   - Streaming body read with 5 MB hard cap (prevents OOM)
7. **Extract title**: Case-insensitive `<title>` tag search
8. **Convert HTML to text**: `html2text::from_read(&bytes[..], 120)` (120-column width)
9. **Store text artifact**: Content-addressed blob + artifact record, description = URL
10. **Chunk text**: `chunk_text(&text, 1000, 200, label_prefix)` — 1000-char chunks, 200-char overlap
11. **Store chunks artifact**: JSON blob + artifact record
12. **Enqueue for embedding**: `embed_queue::enqueue()` — non-fatal if this fails
13. **Mark completed**: Stores title, content_length, artifact IDs, chunk_count

#### Deduplication

A partial unique index prevents duplicate active entries for the same URL within a project:

```sql
CREATE UNIQUE INDEX idx_url_ingest_project_url
    ON url_ingest_queue(project_id, url) WHERE status NOT IN ('failed', 'cancelled');
```

The INSERT uses `ON CONFLICT DO NOTHING` to silently skip duplicates.

#### API Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/api/v1/projects/{id}/url-ingest` | Write | Submit URLs for ingestion (max 50) |
| `GET` | `/api/v1/projects/{id}/url-ingest` | Read | List queue items (optional `?status=` filter) |
| `DELETE` | `/api/v1/projects/{id}/url-ingest/{queue_id}` | Write | Cancel a pending item |
| `POST` | `/api/v1/projects/{id}/url-ingest/{queue_id}/retry` | Write | Retry a failed item |

URL validation in the POST handler:
- Must start with `http://` or `https://`
- Max 2048 characters per URL
- Max 50 URLs per request
- Empty list rejected

#### CLI Commands

```bash
# Submit URLs for ingestion
af url-ingest submit <PROJECT_ID> https://example.com/blog https://docs.example.com/guide

# List queue items
af url-ingest list --project <PROJECT_ID>
af url-ingest list --project <PROJECT_ID> --status failed

# Cancel a pending item
af url-ingest cancel <QUEUE_ID>

# Retry a failed item
af url-ingest retry <QUEUE_ID>
```

### Embedding Queue

**Source files**:
- Processor: `af/crates/af-builtin-tools/src/embed_queue.rs`
- DB layer: `af/crates/af-db/src/embed_queue.rs`
- Migration: `af/crates/af-db/migrations/034_embed_queue.sql`

#### How Items Get Enqueued

Items are enqueued from two sources:

1. **Tool-produced chunks**: When `doc.ingest` or `doc.chunk` produces a `chunks.json` artifact, the OOP executor auto-enqueues it:
   ```rust
   // In oop_executor.rs, after ingest_produced_file() succeeds:
   if pf.filename == "chunks.json" && tool_name.starts_with("doc.") {
       embed_queue::enqueue(pool, project_id, chunks_artifact_id, source_artifact_id, "doc.ingest")
   }
   ```

2. **URL ingestion**: After URL content is chunked, `url_ingest.rs` enqueues the chunks artifact:
   ```rust
   embed_queue::enqueue(pool, project_id, chunks_artifact_id, Some(text_artifact_id), "url.ingest")
   ```

Both use `ON CONFLICT DO NOTHING` — enqueue failures are non-fatal.

#### Processing Steps (per item)

1. **Recover stale**: Items stuck in `processing` for > 2 minutes
2. **List pending**: Up to 10 per tick
3. **Claim atomically**: `pending → processing`
4. **Load chunks.json**: Read the artifact blob, parse as `Vec<Chunk>`
5. **Update progress**: Record total `chunk_count`
6. **Resume from partial**: Skip already-embedded chunks (`chunks_embedded` counter)
7. **Embed in batches of 100**:
   - Call `backend.embed(texts)` to get vector embeddings
   - Validate dimension count on first embedding
   - Insert each embedding into pgvector within a transaction
   - Update `chunks_embedded` counter after each batch (enables resume)
8. **Mark completed**

#### Source Artifact Tracking

The `source_artifact_id` field tracks the original document (not the chunks.json). When embeddings are stored in pgvector, they reference this source artifact so that `embed.search` results trace back to the original file:

```rust
let embed_artifact_id = item.source_artifact_id;
// ...
af_db::embeddings::insert_embedding(&mut *tx, item.project_id, embed_artifact_id, ...)
```

#### Resumability

Embedding is resumable. If a tick crashes mid-batch:
- The `chunks_embedded` counter records progress after each batch
- The stale recovery resets the item to `pending`
- The next tick skips already-embedded chunks:
  ```rust
  let skip = item.chunks_embedded as usize;
  let remaining: Vec<&Chunk> = chunks.iter().skip(skip).collect();
  ```

#### Prerequisite: Embedding Backend

The embed queue only processes when `AF_EMBEDDING_ENDPOINT` is configured:

```rust
// In tick.rs:
if let Some(eb) = crate::bootstrap::build_embedding_backend() {
    match af_builtin_tools::embed_queue::process_embed_queue(&pool, &*eb).await {
        // ...
    }
}
```

If no embedding backend is configured, the entire step is silently skipped.

#### CLI Commands

```bash
# List embed queue items
af embed-queue list
af embed-queue list --project <PROJECT_ID>
af embed-queue list --status pending

# Cancel a pending item
af embed-queue cancel <QUEUE_ID>

# Retry a failed item
af embed-queue retry <QUEUE_ID>
```

### Notification Queue

**Source files**:
- Crate: `af/crates/af-notify/`
- Queue processor: `af/crates/af-notify/src/queue.rs`
- Channel delivery: `af/crates/af-notify/src/channels.rs`
- PgListener: `af/crates/af-notify/src/listener.rs`
- DB layer: `af/crates/af-db/src/notifications.rs`
- Migration: `af/crates/af-db/migrations/036_notifications.sql`
- API routes: `af/crates/af-api/src/routes/notifications.rs`

#### Dual Delivery Model

Unlike the other queues which rely solely on tick for processing, notifications use a **dual delivery model**:

1. **Near-real-time (PgListener)**: A `pg_notify()` trigger on `notification_queue` INSERT fires a PostgreSQL notification. When `af serve` is running, a PgListener task picks up new items and delivers them immediately (typically < 1 second).

2. **Fallback (tick)**: `af tick` processes any pending items that the PgListener missed (server not running, delivery failure, PgListener reconnecting).

```
Agent calls notify.send ──→ INSERT into notification_queue
                                      │
                            ┌─────────┴──────────┐
                            │                    │
                     pg_notify() trigger    af tick (fallback)
                            │                    │
                     PgListener picks up    list_pending(20)
                            │                    │
                     claim ──→ deliver    claim ──→ deliver
                            │                    │
                     complete/fail        complete/fail
```

#### How Items Get Enqueued

Items are enqueued from two sources:

1. **Agent tool call**: `notify.send` resolves channel by name + project_id, inserts into `notification_queue`:
   ```rust
   af_db::notifications::enqueue(
       pool, project_id, channel_id, &subject, &body,
       None, // attachment_artifact_id
       ctx.submitted_by,
   )
   ```

2. **API test endpoint**: `POST /projects/{id}/notification-channels/{ch_id}/test` enqueues a test message for verification.

#### Channel Types and Delivery

| Type | Protocol | Timeout | Details |
|------|----------|---------|---------|
| `webhook` | HTTPS POST | 15s | JSON body, custom headers (blocklist enforced), no redirects |
| `email` | Gmail REST / ProtonMail SMTP | 30s | Reuses `af-email` infrastructure, credential-based |
| `matrix` | HTTPS PUT | 15s | `m.room.message`, txn_id = queue item ID (idempotent) |
| `webdav` | HTTPS PUT | 60s | File uploads (256 MB cap), basic auth, filename sanitization |

Security measures applied to all channels:
- **HTTPS enforcement**: URLs validated at both creation time (API) and delivery time (defense-in-depth)
- **No redirects**: `redirect::Policy::none()` prevents SSRF via open redirects
- **Webhook header blocklist**: `host`, `content-length`, `transfer-encoding`, `connection`, `upgrade`, `te`, `trailer`, `proxy-authorization`, `cookie`
- **Error sanitization**: HTTP error bodies are truncated and sanitized to prevent credential leaks

#### Permanent vs Transient Failures

The notification queue distinguishes between permanent and transient failures:

- **Transient failures** (network timeout, 5xx response): Increment `attempt_count`, reset to `pending` for retry (up to `max_attempts=5`)
- **Permanent failures** (invalid config, unsupported channel type, 4xx response): Immediately set to `failed` via `fail_permanent()`, skipping remaining retries

```rust
pub struct PermanentError(pub String);

pub fn is_permanent(e: &anyhow::Error) -> bool {
    e.downcast_ref::<PermanentError>().is_some()
}
```

Email delivery always returns `PermanentError` on failure (credential issues, recipient rejections are not transient).

#### Processing Steps (per tick)

1. **Recover stale**: Items stuck in `processing` for > 2 minutes
2. **List pending**: Up to 20 per tick
3. **For each item**:
   a. **Claim atomically**: `pending → processing` (non-fatal, skip on failure)
   b. **Load channel config**: Look up channel by ID (non-fatal, fail item on error)
   c. **Deliver**: Dispatch to channel-specific handler
   d. **Mark completed** or **fail** (with permanent/transient distinction)

All per-item DB operations are non-fatal — a failure on one item doesn't prevent processing of subsequent items.

#### PgListener (Near-Real-Time)

When `af serve` is running, a background task listens for PostgreSQL notifications:

```rust
pub async fn run_notification_listener(
    pool: PgPool,
    storage_root: PathBuf,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen("notification_queue").await?;

    loop {
        tokio::select! {
            notif = listener.recv() => {
                // Parse UUID from payload → claim → deliver
            }
            _ = shutdown.changed() => break,
        }
    }
}
```

- Auto-reconnects on PgListener disconnect (handled by sqlx)
- Graceful shutdown via watch channel
- Same delivery path as tick (claim → load channel → deliver → complete/fail)

#### API Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/projects/{id}/notification-channels` | Write | Create channel (validate type + config) |
| `GET` | `/projects/{id}/notification-channels` | Read | List channels (config redacted) |
| `PUT` | `/projects/{id}/notification-channels/{ch_id}` | Write | Update channel config/enabled |
| `DELETE` | `/projects/{id}/notification-channels/{ch_id}` | Write | Delete channel |
| `POST` | `/projects/{id}/notification-channels/{ch_id}/test` | Write | Send test notification |
| `GET` | `/projects/{id}/notifications` | Read | List queue with `?status=` filter |
| `DELETE` | `/projects/{id}/notifications/{queue_id}` | Write | Cancel pending notification |
| `POST` | `/projects/{id}/notifications/{queue_id}/retry` | Write | Retry failed notification |

Config validation per channel type:
- **Webhook**: `url` required, must be `https://`
- **Email**: `to` required (non-empty array), `credential_id` required
- **Matrix**: `homeserver`, `room_id`, `access_token` required
- **WebDAV**: `url` required, must be `https://`

#### CLI Commands

```bash
# Channel management
af notify channel add <PROJECT_ID> <NAME> <TYPE> --config '{"url":"https://..."}'
af notify channel list <PROJECT_ID>
af notify channel remove <CHANNEL_ID>
af notify channel test <CHANNEL_ID>

# Queue management
af notify queue list [--project <UUID>] [--status pending|completed|failed]
af notify queue cancel <UUID>
af notify queue retry <UUID>
```

---

## Job Queue (Tool Execution)

The job queue handles tool execution (rizin, ghidra, file analysis, etc.). Unlike the tick-based queues above, this uses a persistent worker process.

**Source files**:
- Worker: `af/crates/af-jobs/src/worker.rs`
- Reaper: `af/crates/af-jobs/src/reaper.rs`

### Claiming with FOR UPDATE SKIP LOCKED

The worker claims jobs using PostgreSQL's `FOR UPDATE SKIP LOCKED` advisory locking:

```sql
SELECT * FROM tool_runs
WHERE status = 'queued'
ORDER BY created_at
LIMIT 1
FOR UPDATE SKIP LOCKED
```

This pattern:
- **Lock-free**: Multiple workers can query simultaneously without blocking
- **Skip locked**: Workers skip rows being processed by other workers
- **Fair**: Oldest job first (FIFO ordering)
- **Atomic**: The selected row is locked for the duration of the transaction

### Heartbeat and Lease

Each claimed job has a **120-second lease** (`LEASE_DURATION_SECS = 120`). The worker extends the lease every **30 seconds** (`HEARTBEAT_INTERVAL_SECS = 30`):

```rust
let heartbeat_handle = tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        if let Err(e) = af_db::tool_runs::heartbeat(&pool, run_id, LEASE_DURATION_SECS).await {
            heartbeat_cancel_tx.notify_one(); // Signal execution to abort
            break;
        }
    }
});
```

If the heartbeat fails (job was reaped by another process), the worker aborts execution:

```rust
tokio::select! {
    res = exec_fut => res,
    _ = heartbeat_cancel.notified() => {
        Err(ToolError {
            code: "heartbeat_lost",
            message: "execution aborted: job heartbeat failed (likely reaped)",
            ...
        })
    }
}
```

### Reaper

The reaper runs as a background task alongside the API server, checking every **30 seconds** for stale jobs:

```rust
pub async fn run_reaper(pool: PgPool, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Reclaim expired leases (reset to 'queued' for retry)
                af_db::tool_runs::reclaim_expired(&pool, MAX_ATTEMPTS).await;
                // Permanently fail runs that exceeded 3 retry attempts
                af_db::tool_runs::fail_exhausted(&pool, MAX_ATTEMPTS).await;
            }
            _ = shutdown.changed() => return,
        }
    }
}
```

Two operations per cycle:
1. **Reclaim expired**: Jobs with expired leases (lease_expires_at < now) are reset to `queued` with incremented attempt count
2. **Fail exhausted**: Jobs with `attempt_count >= 3` are permanently failed

The reaper responds to a shutdown signal (`watch::Receiver<bool>`) for graceful termination.

### Worker Security Model

The worker uses a layered security model:

- **Claiming**: Runs as `af` (DB table owner, bypasses RLS) — needs to see all queued jobs across tenants
- **Post-execution**: Uses `begin_scoped(pool, actor_user_id)` for defense-in-depth RLS enforcement on tenant-scoped tables
- **Audit log**: Runs unscoped (not RLS-protected)
- **Heartbeat**: Runs unscoped (specific row update)
- **Scratch dir**: Created per-job, cleaned up after execution (even on error/panic)

---

## Concurrency and Safety

### Multiple Tick Processes

It's safe to run multiple `af tick` processes concurrently (e.g., cron overlap). Each queue system uses atomic claiming:

| System | Claiming Mechanism |
|--------|--------------------|
| Email scheduling | `claim_scheduled()` — `UPDATE WHERE status = 'pending'` |
| URL ingestion | `claim()` — `UPDATE WHERE status = 'pending'` |
| Embedding queue | `claim()` — `UPDATE WHERE status = 'pending'` |
| Notification queue | `claim()` — `UPDATE WHERE status = 'pending'` |
| Tick hooks | `claim_tick()` — CAS on `tick_generation` |
| Job queue | `FOR UPDATE SKIP LOCKED` |

### Data Integrity

- **Blob GC**: Two-phase delete with re-check prevents TOCTOU races
- **URL dedup**: Partial unique index prevents duplicate active entries
- **Embed dedup**: Partial unique index on `chunks_artifact_id` prevents duplicate active entries
- **Notification dedup**: PgListener + tick can both attempt delivery; atomic `claim()` ensures exactly-once processing
- **Hook dedup**: CAS on `tick_generation` prevents double-firing

### Crash Recovery

All queues handle crashed workers:

| System | Recovery Mechanism | Threshold |
|--------|-------------------|-----------|
| URL ingestion | `recover_stale()` in process_url_queue | 2 minutes |
| Embedding queue | `recover_stale()` in process_embed_queue | 2 minutes |
| Notification queue | `recover_stale()` in process_notification_queue | 2 minutes |
| Job queue | Reaper task (`reclaim_expired`) | Lease expiry (120s) |
| Email scheduling | Re-checked each tick (due items re-listed) | N/A (no stuck state) |

---

## Configuration Reference

### Environment Variables

| Variable | Default | Used By |
|----------|---------|---------|
| `AF_EMBEDDING_ENDPOINT` | (none) | Embed queue — required for processing |
| `AF_EMBEDDING_MODEL` | `snowflake-arctic-embed2` | Embed queue — model name |
| `AF_EMBEDDING_DIMENSIONS` | (auto) | Embed queue — vector dimensions |
| `AF_STORAGE_ROOT` | `/tmp/af/storage` | URL ingestion — blob storage |
| `AF_SCRATCH_ROOT` | `/tmp/af/scratch` | Tick — scratch cleanup |
| `AF_EMAIL_RATE_LIMIT` | `10` | Email scheduling — global rate |
| `AF_EMAIL_PER_USER_RPM` | `5` | Email scheduling — per-user rate |

### Tuning Constants

| Constant | Value | Location | Purpose |
|----------|-------|----------|---------|
| `MAX_HOOKS_PER_EVENT` | 10 | `hooks.rs` | Max hooks per event (prevents storms) |
| `BATCH_SIZE` (URL) | 5 | `url_ingest.rs` | URLs processed per tick |
| `BATCH_SIZE` (embed) | 100 | `embed_queue.rs` | Chunks per embedding batch |
| Embed items per tick | 10 | `embed_queue.rs` | Items processed per tick |
| `MAX_BODY_BYTES` | 5 MB | `url_ingest.rs` | Max URL response size |
| `BATCH_SIZE` (notifications) | 20 | `queue.rs` (af-notify) | Notifications processed per tick |
| `STALE_PROCESSING_MINUTES` | 2 | `url_ingest.rs`, `embed_queue.rs`, `queue.rs` | Stale recovery threshold |
| `LEASE_DURATION_SECS` | 120 | `worker.rs` | Job lease duration |
| `HEARTBEAT_INTERVAL_SECS` | 30 | `worker.rs` | Heartbeat frequency |
| `MAX_ATTEMPTS` (reaper) | 3 | `reaper.rs` | Job retry limit |
| `max_attempts` (queues) | 3-5 | DB default | Queue retry limit (embed: 3, URL: 5, notifications: 5) |
| Scratch cleanup threshold | 2 hours | `tick.rs` | Stale scratch dir age |

---

## Operational Examples

### Setting Up Cron

```bash
# Basic: run tick every minute
* * * * * /usr/local/bin/af tick >> /var/log/af-tick.log 2>&1

# With environment variables
* * * * * AF_DATABASE_URL=postgres://af:pass@db/af \
          AF_STORAGE_ROOT=/data/af/storage \
          AF_EMBEDDING_ENDPOINT=http://localhost:11434/v1 \
          /usr/local/bin/af tick >> /var/log/af-tick.log 2>&1
```

### Monitoring Queue Health

```bash
# Check all queues
af embed-queue list --status pending
af embed-queue list --status failed
af url-ingest list --status pending
af url-ingest list --status failed
af notify queue list --status pending
af notify queue list --status failed

# Retry all failed items in a project
for id in $(af embed-queue list --project $PID --status failed | awk '{print $1}'); do
    af embed-queue retry $id
done
```

### End-to-End URL Import Flow

```bash
# 1. Submit URLs
af url-ingest submit $PROJECT_ID \
    https://blog.example.com/malware-analysis-guide \
    https://docs.example.com/threat-intel-2024

# 2. Check queue status
af url-ingest list --project $PROJECT_ID
# ID                                   | URL                                    | STATUS  | CHUNKS | TITLE
# a1b2c3d4-...                        | https://blog.example.com/malware-a...  | pending |   —    |  —

# 3. Run tick (fetches URLs, converts, chunks, enqueues for embedding)
af tick
# [tick] ingested 2 URL(s)

# 4. Check again
af url-ingest list --project $PROJECT_ID
# ID                                   | URL                                    | STATUS    | CHUNKS | TITLE
# a1b2c3d4-...                        | https://blog.example.com/malware-a...  | completed |   45   | Malware Analysis Guide

# 5. Check embed queue (chunks are now enqueued)
af embed-queue list --project $PROJECT_ID
# ID                                   | STATUS  | CHUNKS | EMBEDDED | TOOL
# e5f6g7h8-...                        | pending |   45   |    0     | url.ingest

# 6. Run tick again (embeds the chunks)
af tick
# [tick] embedded 1 queued chunk set(s)

# 7. Content is now searchable via embed.search
# Agent: embed.search "malware analysis techniques"
# → Returns matching chunks from the ingested blog post
```

### Creating a Full Automation Pipeline

Combine hooks with background processing for a fully automated RE pipeline:

```bash
# 1. Create auto-analyze hook (fires on upload)
af hook create --project $PROJECT_ID \
    --name "auto-analyze" \
    --event artifact_uploaded \
    --workflow full-analysis \
    --template "Analyze {{filename}} (SHA256: {{sha256}}). \
Run surface analysis, decompile key functions, extract IOCs, and write a report."

# 2. Create daily summary hook (fires every 24 hours)
af hook create --project $PROJECT_ID \
    --name "daily-summary" \
    --event tick \
    --agent reporter \
    --interval 1440 \
    --template "Generate a daily summary for project {{project_name}}. \
This is report #{{tick_count}}."

# 3. Now the pipeline is fully automated:
#    - Upload a sample → auto-analyze fires → workflow runs → results stored
#    - Every 24 hours → daily-summary fires → reporter generates summary
#    - All processing happens in af tick, no manual intervention needed
```
