# CWC Integration Plan — Comprehensive Implementation Guide

## Table of Contents

1. [Git Setup](#1-git-setup)
2. [Phase 1: Session Management Integration](#2-phase-1-session-management)
3. [Phase 2: RAG Pipeline Integration](#3-phase-2-rag-pipeline)
4. [Phase 3: Verification Layer](#4-phase-3-verification)
5. [Migration & Rollout](#5-migration--rollout)
6. [File Change Index](#6-file-change-index)

---

## 1. Git Setup

### 1.1 Add CWC as a Git Submodule

```bash
cd /home/ds/work/fpga_local_git/arbeiterfarm
git submodule add https://github.com/YOUR_USER/context_window_compiler.git cwc
# This creates:
#   .gitmodules          — tracks the submodule URL + path
#   cwc/                 — checkout of CWC repo at a pinned commit
```

The submodule pins a specific commit. To update:
```bash
cd cwc && git pull origin main && cd ..
git add cwc
git commit -m "bump CWC submodule"
```

Anyone cloning the repo:
```bash
git clone --recurse-submodules <url>
# or after clone:
git submodule update --init
```

### 1.2 Add CWC Crates to Workspace

**File: `Cargo.toml` (workspace root)**

Add workspace members (only the crates we import — not cwc-cli, cwc-server, cwc-eval):

```toml
[workspace]
members = [
    # ... existing members ...
]

# CWC crates are NOT workspace members (they're in a submodule with their own workspace).
# We reference them via path dependencies in [workspace.dependencies].

[workspace.dependencies]
# ... existing deps ...

# CWC crates (via submodule)
cwc-core = { path = "cwc/week_22/cwc-core" }
cwc-session = { path = "cwc/week_22/cwc-session" }
cwc-ingest = { path = "cwc/week_22/cwc-ingest" }
cwc-index = { path = "cwc/week_22/cwc-index" }
cwc-retrieve = { path = "cwc/week_22/cwc-retrieve" }
cwc-compile = { path = "cwc/week_22/cwc-compile" }
cwc-embed = { path = "cwc/week_22/cwc-embed" }
cwc-verify = { path = "cwc/week_22/cwc-verify" }
cwc-memory = { path = "cwc/week_22/cwc-memory" }
```

**Important**: CWC crates are NOT added to `[workspace].members` — they live in
the submodule's own workspace. They're referenced as path dependencies only.
Cargo resolves this correctly: each CWC crate's own `Cargo.toml` specifies its
deps, and Cargo deduplicates shared deps (serde, tokio, etc.) at build time.

### 1.3 Dependency Version Alignment

Both projects share these deps at compatible versions:
- `serde 1`, `serde_json 1`, `tokio 1`, `async-trait 0.1`, `thiserror 2`
- `uuid 1` (CWC uses `v4,v5,serde`; Arbeiterfarm uses `v4,serde` — compatible)
- `tracing 0.1`, `reqwest 0.12`, `sqlx 0.8`, `dashmap 6`
- `sha2 0.10`, `regex 1`, `chrono 0.4`

CWC-only deps that get pulled in:
- `tiktoken-rs 0.6` — GPT tokenizer (used by cwc-core, ~2MB)
- `tantivy 0.22` — BM25 index (used by cwc-index, ~10MB)
- `ort 2.0.0-rc.12` — ONNX Runtime (used by cwc-embed, large)
- `tokenizers 0.22` — HuggingFace tokenizer (used by cwc-retrieve reranker)
- `bincode 1` — serialization for in-memory vector index
- `ndarray 0.17` — ONNX tensor ops
- `glob 0.3` — file pattern matching

These are all additive — no conflicts with Arbeiterfarm's existing deps.

**Build impact**: `tantivy` adds ~30s to clean build. `ort` adds ~15s but is
only needed for Phase 2 (reranking). Phase 1 (session only) pulls in only
`cwc-core` + `cwc-session` which add `tiktoken-rs` — minimal impact.

---

## 2. Phase 1: Session Management

**Goal**: Replace Arbeiterfarm's hand-rolled compaction + thread memory with CWC's
`SessionManager`, keeping Arbeiterfarm's DB persistence and streaming architecture.

### 2.1 Type Mapping Layer

The two systems use near-identical but distinct message types. We need
bidirectional conversion functions.

**New file: `af/crates/af-agents/src/cwc_bridge.rs`**

```
Arbeiterfarm ChatMessage          ←→    CWC SessionMessage
├── role: ChatRole              ├── role: SessionRole
├── content: String             ├── content: String
├── tool_call_id: Option        ├── tool_result: Option<ToolResult>
├── name: Option                │       (combines tool_call_id + name + content)
├── tool_calls: Vec<ToolCall>   ├── tool_calls: Vec<ToolCall>
├── content_parts: Option       │       (CWC has no VLM equivalent — ignored)
│                               ├── token_count: u32
│                               └── flags: MessageFlags
```

**Conversion: `ChatMessage → SessionMessage`**

```rust
fn chat_to_session(msg: &ChatMessage) -> SessionMessage {
    let role = match msg.role {
        ChatRole::System => SessionRole::System,
        ChatRole::User => SessionRole::User,
        ChatRole::Assistant => SessionRole::Assistant,
        ChatRole::Tool => SessionRole::Tool,
    };
    let mut sm = SessionMessage::text(role, &msg.content);

    // System → PRESERVE
    if role == SessionRole::System {
        sm.flags.insert(MessageFlags::PRESERVE);
    }

    // Map tool_calls
    for tc in &msg.tool_calls {
        sm.tool_calls.push(cwc_session::ToolCall {
            call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        });
    }

    // Map tool result (role=Tool)
    if role == SessionRole::Tool {
        sm.tool_result = Some(cwc_session::ToolResult {
            call_id: msg.tool_call_id.clone().unwrap_or_default(),
            tool_name: msg.name.clone().unwrap_or_default(),
            output: msg.content.clone(),
            is_error: false,
        });
    }

    // Detect nudge/memory/compaction markers from content
    if is_nudge_content(&msg.content) {
        sm.flags.insert(MessageFlags::IS_NUDGE);
    }
    if is_memory_content(&msg.content) {
        sm.flags.insert(MessageFlags::IS_MEMORY);
    }

    sm
}
```

**Conversion: `SessionMessage → ChatMessage`**

```rust
fn session_to_chat(sm: &SessionMessage) -> ChatMessage {
    ChatMessage {
        role: match sm.role {
            SessionRole::System => ChatRole::System,
            SessionRole::User => ChatRole::User,
            SessionRole::Assistant => ChatRole::Assistant,
            SessionRole::Tool => ChatRole::Tool,
        },
        content: sm.content.clone(),
        tool_call_id: sm.tool_result.as_ref().map(|tr| tr.call_id.clone()),
        name: sm.tool_result.as_ref().map(|tr| tr.tool_name.clone()),
        tool_calls: sm.tool_calls.iter().map(|tc| ToolCallInfo {
            id: tc.call_id.clone(),
            name: tc.tool_name.clone(),
            arguments: tc.arguments.clone(),
        }).collect(),
        content_parts: None,
    }
}
```

**Batch converters:**

```rust
fn chat_messages_to_session(
    msgs: &[ChatMessage],
    tokenizer: Arc<dyn TokenCounter>,
) -> Session {
    let mut session = Session::new(tokenizer);
    for msg in msgs {
        session.push(chat_to_session(msg));
    }
    session
}

fn session_to_chat_messages(session: Session) -> Vec<ChatMessage> {
    session.into_messages().iter().map(session_to_chat).collect()
}
```

### 2.2 TokenCounter Adapter

Arbeiterfarm uses `estimate_content_tokens()` (~4 chars/token heuristic).
CWC uses `trait TokenCounter { fn count_tokens(&self, text: &str) -> u32; }`.

We need an adapter that implements CWC's trait using Arbeiterfarm's heuristic:

```rust
/// Adapter: implements cwc_core::TokenCounter using Arbeiterfarm's heuristic.
struct AfTokenCounter;

impl cwc_core::traits::TokenCounter for AfTokenCounter {
    fn count_tokens(&self, text: &str) -> u32 {
        (text.len() as u32) / 4
    }

    fn truncate_to_tokens(&self, text: &str, max_tokens: u32) -> String {
        let max_bytes = (max_tokens * 4) as usize;
        if text.len() <= max_bytes {
            return text.to_string();
        }
        // Find valid UTF-8 boundary
        let mut end = max_bytes;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_string()
    }
}
```

Or, for better accuracy with cloud models, use CWC's built-in tiktoken:

```rust
use cwc_core::tokenizer::TiktokenCounter;
let tokenizer: Arc<dyn TokenCounter> = Arc::new(TiktokenCounter::new("cl100k_base"));
```

**Decision**: Use `AfTokenCounter` for local models (matches existing behavior),
`TiktokenCounter` for cloud models (more accurate). Select based on
`BackendCapabilities::is_local`.

### 2.3 SessionManager Lifecycle

CWC's `SessionManager` is stateful — it holds the memory store and compaction
engine. We need to decide where it lives in Arbeiterfarm's architecture.

**Option A: Per-thread SessionManager (recommended)**

Create a `SessionManager` at the start of each `send_message()` /
`streaming_loop()` call, seeded with the thread's existing memory from DB.

```rust
// In runtime.rs, at the top of send_message():
let cwc_config = build_cwc_config(&caps);
let tokenizer: Arc<dyn cwc_core::traits::TokenCounter> = if caps.is_local {
    Arc::new(AfTokenCounter)
} else {
    Arc::new(AfTokenCounter) // or TiktokenCounter for cloud
};
let mut session_mgr = SessionManager::new(cwc_config, tokenizer.clone())?;

// Seed memory from DB
let db_memory = af_db::thread_memory::list_memory(pool, thread_id).await?;
for (key, value) in &db_memory {
    session_mgr.memory_mut().upsert(MemoryFact {
        key: key.clone(),
        value: value.clone(),
        source: MemorySource::UserMessage { message_index: 0 },
        created_at: 0,
        priority: if key == "goal" { FactPriority::Goal }
                  else if key.starts_with("finding:") { FactPriority::Finding }
                  else { FactPriority::Status },
    });
}
```

**Option B: Cached SessionManager in AgentRuntime**

Store `Option<SessionManager>` in `AgentRuntime`, lazily initialized per thread.
More efficient (no re-creation per call) but requires thread_id tracking in the
runtime struct.

**Recommendation**: Option A. The SessionManager is lightweight (~1KB), and
per-call creation avoids stale state bugs. The DB is the source of truth anyway.

### 2.4 Integration Points in runtime.rs

There are **4 places** where CWC hooks into the existing runtime:

#### 2.4.1 Pre-LLM Optimization (before first LLM call)

**Current code** (runtime.rs ~line 540):
```rust
// Estimate tokens
let estimated = af_llm::estimate_tokens(&messages, &tools);
if compaction_ctx.should_compact(estimated) {
    // ... sliding_window_trim or try_compact ...
}
```

**New code**:
```rust
// Convert Arbeiterfarm messages → CWC Session
let mut session = cwc_bridge::chat_messages_to_session(&messages, tokenizer.clone());

// Run CWC optimization (compaction + trim + preflight + nudge)
let report = session_mgr.optimize(&mut session)?;

if report.trim != TrimAction::None {
    tracing::info!(
        "CWC optimization: trim={:?}, tokens_saved={}, memory_facts={}",
        report.trim, report.tokens_saved, report.memory_facts
    );
    // Sync memory back to DB
    sync_memory_to_db(pool, thread_id, session_mgr.memory()).await?;
}

// Convert back to Arbeiterfarm messages
messages = cwc_bridge::session_to_chat_messages(session);
```

#### 2.4.2 After Each Tool Result (reactive compaction)

**Current code** (runtime.rs ~line 780):
```rust
// After tool invocation:
let mem_entries = thread_memory::extract_from_tool_result(&name, &tool_result_str);
for entry in &mem_entries {
    af_db::thread_memory::upsert_memory(pool, thread_id, &entry.key, &entry.value).await;
}
// ... sliding_window_trim if local ...
```

**New code**:
```rust
// Append tool result to CWC session
session.push(cwc_bridge::chat_to_session(&tool_result_msg));

// Run incremental optimization
let report = session_mgr.process_message(&mut session, cwc_bridge::chat_to_session(&tool_result_msg))?;

// Sync any new memory facts to DB
if report.memory_facts > 0 {
    sync_memory_to_db(pool, thread_id, session_mgr.memory()).await?;
}

// Convert back
messages = cwc_bridge::session_to_chat_messages(session);
```

#### 2.4.3 Memory Injection (replaces build_memory_message)

**Current code** (runtime.rs ~line 500):
```rust
if let Some(mem_msg) = thread_memory::build_memory_message(&memory_pairs) {
    messages.insert(1, mem_msg);
}
```

**New code**: CWC's `optimize()` already handles memory injection at
`messages[1]`. The `preflight_check` inside `optimize()` ensures memory
is fresh. **Remove** the manual injection — CWC handles it.

#### 2.4.4 Reinforcement Nudges (replaces manual nudge injection)

**Current code** (runtime.rs ~line 800):
```rust
// After tool result for local models:
messages.push(ChatMessage {
    role: ChatRole::User,
    content: format!("{}\nYour goal: {}", TOOL_RESULT_NUDGE, goal),
    ..
});
```

**New code**: CWC's `optimize()` handles nudge injection via
`ReinforcementConfig`. Configure nudge frequency and goal inclusion
in `SessionManagerConfig`. **Remove** manual nudge code.

### 2.5 CWC Config Builder

Map Arbeiterfarm's `BackendCapabilities` to CWC's `SessionManagerConfig`:

```rust
fn build_cwc_config(caps: &BackendCapabilities) -> SessionManagerConfig {
    let context_window = caps.context_window.unwrap_or(32_000);
    let max_output = caps.max_output_tokens.unwrap_or(4_096);

    let model_config = if caps.is_local {
        // Local: aggressive compaction, tight budget
        ModelProfileConfig::Custom {
            context_window,
            max_output_tokens: max_output,
            effective_fraction: 0.60,
        }
    } else {
        // Cloud: relaxed thresholds
        ModelProfileConfig::Custom {
            context_window,
            max_output_tokens: max_output,
            effective_fraction: 0.85,
        }
    };

    SessionManagerConfig {
        session: SessionConfig {
            model: model_config,
            sliding_window_fraction: 0.50,
            hard_reset_fraction: 0.60,
            tail_tokens: if caps.is_local { 6000 } else { 12000 },
            min_tail_turns: 2,
            ..Default::default()
        },
        reinforcement: ReinforcementConfig {
            enabled: true,
            nudge_every_n_tool_results: 1,
            include_goal: true,
            max_nudge_tokens: 80,
        },
        preflight: PreflightConfig {
            max_consecutive_same_tool: 3,
            max_tool_calls_without_user: 15,
            default_system_prompt: String::new(), // Arbeiterfarm provides its own
        },
        memory_max_bytes: 2048,
        memory_max_entries: 30,
        artifact_dir: PathBuf::from("/tmp/af/cwc_artifacts"),
        consolidation: ConsolidationConfig::default(),
        llm_consolidation: LlmConsolidationConfig::default(),
    }
}
```

### 2.6 Memory DB Sync

CWC's `SessionMemoryStore` is in-memory. Arbeiterfarm persists to `thread_memory` table.
We need bidirectional sync:

```rust
/// Sync CWC memory store → Arbeiterfarm DB (after optimization)
async fn sync_memory_to_db(
    pool: &PgPool,
    thread_id: Uuid,
    store: &SessionMemoryStore,
) -> Result<()> {
    for fact in store.all() {
        af_db::thread_memory::upsert_memory(
            pool, thread_id, &fact.key, &fact.value,
        ).await?;
    }
    Ok(())
}

/// Seed CWC memory store ← Arbeiterfarm DB (at session start)
fn seed_memory_from_db(
    store: &mut SessionMemoryStore,
    db_entries: &[(String, String)],
) {
    for (key, value) in db_entries {
        let priority = match key.as_str() {
            "goal" => FactPriority::Goal,
            "latest_request" => FactPriority::Goal,
            k if k.starts_with("finding:") => FactPriority::Finding,
            _ => FactPriority::Status,
        };
        store.upsert(MemoryFact {
            key: key.clone(),
            value: value.clone(),
            source: MemorySource::UserMessage { message_index: 0 },
            created_at: 0,
            priority,
        });
    }
}
```

### 2.7 Compaction Rule Configuration

CWC's `CompactionEngine` uses rules per tool. Map Arbeiterfarm's artifact-first tools
to CWC compaction rules:

```rust
fn af_compaction_rules() -> Vec<CompactionRule> {
    vec![
        // RE tools with large output → HeadTruncate
        CompactionRule {
            tool_name: "ghidra.analyze".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 30 },
            inline_token_limit: 200,
        },
        CompactionRule {
            tool_name: "ghidra.decompile".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 50 },
            inline_token_limit: 300,
        },
        CompactionRule {
            tool_name: "rizin.disasm".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 30 },
            inline_token_limit: 200,
        },
        CompactionRule {
            tool_name: "file.grep".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 25 },
            inline_token_limit: 150,
        },
        CompactionRule {
            tool_name: "file.strings".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 20 },
            inline_token_limit: 150,
        },
        CompactionRule {
            tool_name: "sandbox.trace".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 40 },
            inline_token_limit: 250,
        },
        CompactionRule {
            tool_name: "transform.jq".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 30 },
            inline_token_limit: 200,
        },
        CompactionRule {
            tool_name: "transform.regex".into(),
            strategy: CompactionStrategy::HeadTruncate { max_lines: 20 },
            inline_token_limit: 150,
        },
        // Small tools: no compaction (already artifact-first with compact summaries)
        CompactionRule {
            tool_name: "file.info".into(),
            strategy: CompactionStrategy::None,
            inline_token_limit: 500,
        },
        CompactionRule {
            tool_name: "file.read_range".into(),
            strategy: CompactionStrategy::None,
            inline_token_limit: 500,
        },
    ]
}
```

Note: Arbeiterfarm already produces artifact-first output (compact inline summaries).
CWC's compaction is a **second defense** — it catches cases where inline
summaries are still too large for the token budget, or when non-artifact-first
tools produce verbose output.

### 2.8 What Gets Removed from Arbeiterfarm

After CWC integration, these become dead code in af-agents:

| File/Function | Replaced By | Action |
|---|---|---|
| `compaction.rs::sliding_window_trim()` | `cwc_session::sliding_window_trim()` | Remove |
| `compaction.rs::try_compact()` | `SessionManager::optimize()` | Remove |
| `compaction.rs::local_context_reset()` | `cwc_session::hard_reset()` | Remove |
| `compaction.rs::select_messages_for_compaction()` | Internal to CWC | Remove |
| `compaction.rs::preflight_check()` | `cwc_session::preflight_check()` | Remove |
| `thread_memory.rs::extract_from_tool_result()` | `cwc_session::extract_from_turns()` | Remove |
| `thread_memory.rs::build_memory_message()` | `cwc_session::create_memory_message()` | Remove |
| `thread_memory.rs::extract_goal()` | `cwc_session::extract_goal()` | Remove |
| `thread_memory.rs::is_nudge_or_reinforcement()` | `MessageFlags::IS_NUDGE` | Remove |
| Manual nudge injection in runtime.rs | `ReinforcementConfig` | Remove |

**Keep**: `af-db/src/thread_memory.rs` (DB layer) — still needed for persistence.

### 2.9 Testing Strategy

1. **Unit tests for cwc_bridge.rs** — round-trip conversion fidelity
   - ChatMessage → SessionMessage → ChatMessage preserves all fields
   - Tool calls, tool results, VLM content_parts (dropped gracefully)
   - MessageFlags detection from content markers

2. **Integration tests** — run existing test suite, verify no regressions
   - All 303 existing tests must pass
   - Compaction behavior should be equivalent (may differ in exact token counts)

3. **New tests for CWC-specific behavior**
   - Preflight auto-repair (missing system prompt, orphaned tool results)
   - Tool loop detection (same tool called 4+ times consecutively)
   - Memory eviction (> 30 facts → oldest Navigation facts evicted)
   - Hard reset recovery (60% threshold → instant rebuild)

### 2.10 Files Changed (Phase 1)

```
NEW:  af/crates/af-agents/src/cwc_bridge.rs     (~200 lines)
EDIT: af/crates/af-agents/Cargo.toml             (add cwc-core, cwc-session deps)
EDIT: af/crates/af-agents/src/runtime.rs          (4 integration points, ~150 lines changed)
EDIT: af/crates/af-agents/src/mod.rs              (add mod cwc_bridge)
EDIT: Cargo.toml                                      (workspace deps for cwc-core, cwc-session)
DEL:  af/crates/af-agents/src/compaction.rs       (entire file, ~400 lines) — after validation
DEL:  af/crates/af-agents/src/thread_memory.rs    (extraction logic, ~200 lines) — after validation
```

Estimated: ~350 lines new, ~600 lines removed, ~150 lines modified.

---

## 3. Phase 2: RAG Pipeline

**Goal**: Add hybrid BM25+vector retrieval to Arbeiterfarm via CWC's retrieval pipeline,
exposed as `rag.*` tools alongside existing `embed.*` tools.

### 3.1 Architecture Decision: New Crate vs In-Place

**Option A: New `af-rag` crate** (recommended)

```
af/crates/af-rag/
├── Cargo.toml
├── src/
│   ├── lib.rs          — pub mod specs, executors, index_manager
│   ├── specs.rs        — ToolSpec declarations for rag.* tools
│   ├── executors.rs    — Tool executor implementations
│   ├── index_manager.rs — BM25 + vector index lifecycle
│   └── budget.rs       — Token budget allocation for RAG context
```

**Why separate crate**: CWC's retrieval crates pull in `tantivy` (BM25) and
optionally `ort` (ONNX reranking). These are heavy deps. Keeping them in a
separate crate means `af-builtin-tools` doesn't bloat, and the RAG feature
can be conditionally compiled via Cargo features.

### 3.2 Tool Specifications

#### 3.2.1 `rag.ingest` — Ingest Documents into RAG Index

```json
{
    "name": "rag.ingest",
    "description": "Ingest documents into the project RAG index (BM25 + vector). Supports text, markdown, JSON, code files.",
    "input_schema": {
        "type": "object",
        "required": ["artifact_id"],
        "properties": {
            "artifact_id": { "$ref": "#/$defs/ArtifactId" },
            "chunk_size": { "type": "integer", "default": 1000, "minimum": 100, "maximum": 10000 },
            "chunk_overlap": { "type": "integer", "default": 200 }
        }
    },
    "sandbox": "NoNetReadOnly",
    "timeout_ms": 60000,
    "max_produced_artifacts": 1
}
```

**Flow**:
1. Read artifact from storage → bytes → text (using CWC loader)
2. Chunk text (CWC `RecursiveChunker` with section detection)
3. Index chunks in BM25 (CWC `SparseIndex`, stored at `{AF_STORAGE_ROOT}/rag/{project_id}/bm25/`)
4. Embed chunks → store in pgvector (reuse Arbeiterfarm's existing `embed.batch` pathway)
5. Produce `chunks.json` artifact (same format as `doc.chunk`)
6. Return inline summary: chunk count, avg size, sections detected

#### 3.2.2 `rag.query` — Full RAG Pipeline Query

```json
{
    "name": "rag.query",
    "description": "Query the project knowledge base. Retrieves relevant chunks via hybrid BM25+vector search, allocates token budget, returns grounded context.",
    "input_schema": {
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": { "type": "string", "minLength": 1 },
            "top_k": { "type": "integer", "default": 10, "minimum": 1, "maximum": 50 },
            "mmr_lambda": { "type": "number", "default": 0.7, "minimum": 0.0, "maximum": 1.0 },
            "token_budget": { "type": "integer", "default": 4000, "minimum": 500, "maximum": 32000 }
        }
    },
    "sandbox": "Trusted",
    "timeout_ms": 30000
}
```

**Flow**:
1. BM25 search (CWC `SparseIndex::search`, top_k=50)
2. Vector search (Arbeiterfarm's existing `embed.search` via pgvector, top_k=50)
3. Normalize scores to [0,1]
4. Reciprocal Rank Fusion (CWC `rrf_fuse`, k=60)
5. MMR diversity filter (CWC `mmr_select`, lambda configurable)
6. Token budget allocation (CWC `BudgetAllocator`, select chunks that fit)
7. Edge placement reorder (strongest at positions 1 and N)
8. Return: ranked chunks with scores, source paths, section paths, within budget

**Return format** (inline, not artifact-first — results are already budgeted):
```json
{
    "chunks": [
        {
            "rank": 1,
            "source": "docs/auth.md",
            "section": "OAuth2 Flow",
            "score": 0.87,
            "text": "The OAuth2 flow begins with..."
        }
    ],
    "total_chunks_searched": 1234,
    "budget_used": 3800,
    "budget_total": 4000
}
```

#### 3.2.3 `rag.search` — Retrieval-Only (No Budget Allocation)

```json
{
    "name": "rag.search",
    "description": "Search the project knowledge base without budget allocation. Returns raw ranked results.",
    "input_schema": {
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": { "type": "string" },
            "mode": { "type": "string", "enum": ["hybrid", "bm25", "vector"], "default": "hybrid" },
            "top_k": { "type": "integer", "default": 10, "minimum": 1, "maximum": 100 }
        }
    },
    "sandbox": "Trusted",
    "timeout_ms": 30000
}
```

**Use case**: When the agent wants raw search results without budget constraints.

#### 3.2.4 `rag.status` — Index Status

```json
{
    "name": "rag.status",
    "description": "Show RAG index status: document count, chunk count, index sizes.",
    "input_schema": {
        "type": "object",
        "properties": {}
    },
    "sandbox": "Trusted",
    "timeout_ms": 10000
}
```

### 3.3 BM25 Index Storage

CWC's `SparseIndex` uses Tantivy, which stores index files on disk.

**Storage location**: `{AF_STORAGE_ROOT}/rag/{project_id}/bm25/`

**Lifecycle**:
- Created on first `rag.ingest` for a project
- Updated incrementally (Tantivy supports upsert via delete-then-add)
- Deleted when project is deleted (add to project cleanup cascade)
- No DB migration needed — pure filesystem

**Locking**: Tantivy handles concurrent reads/writes internally. Multiple
agents can query the same index concurrently. Writes are serialized by Tantivy's
`IndexWriter` (single-writer model — acquire in `rag.ingest`, release after commit).

### 3.4 Vector Search Integration

CWC has its own vector index (`InMemoryVectorIndex`, `ChunkDb`).
Arbeiterfarm already has pgvector with HNSW indexes.

**Decision**: Reuse Arbeiterfarm's existing pgvector infrastructure for dense retrieval.
Don't use CWC's `DenseRetriever` — instead, implement CWC's `Retriever` trait
as an adapter over Arbeiterfarm's `af_db::embeddings::search_similar()`:

```rust
struct ArbeiterfarmDenseRetriever {
    pool: PgPool,
    project_id: Uuid,
    embedding_backend: Arc<dyn EmbeddingBackend>,
}

impl cwc_core::traits::Retriever for ArbeiterfarmDenseRetriever {
    fn retrieve(&self, query: &str, top_k: usize) -> Vec<RetrievalHit> {
        // 1. Embed query via Arbeiterfarm's embedding backend
        // 2. Call af_db::embeddings::search_similar()
        // 3. Convert results to CWC RetrievalHit format
    }
}
```

This gives us hybrid retrieval (CWC BM25 + Arbeiterfarm pgvector) without duplicating
vector storage.

### 3.5 Index Manager

Manages BM25 index lifecycle per project:

```rust
pub struct RagIndexManager {
    storage_root: PathBuf,
    indexes: DashMap<Uuid, Arc<SparseIndex>>,  // project_id → BM25 index
}

impl RagIndexManager {
    pub fn new(storage_root: PathBuf) -> Self { ... }

    /// Get or open BM25 index for a project.
    pub fn get_index(&self, project_id: Uuid) -> Result<Arc<SparseIndex>> {
        self.indexes.entry(project_id)
            .or_try_insert_with(|| {
                let path = self.storage_root.join(project_id.to_string()).join("bm25");
                std::fs::create_dir_all(&path)?;
                Ok(Arc::new(SparseIndex::open_or_create(&path)?))
            })
            .map(|entry| entry.clone())
    }

    /// Delete index for a project (cleanup).
    pub fn delete_index(&self, project_id: Uuid) -> Result<()> {
        self.indexes.remove(&project_id);
        let path = self.storage_root.join(project_id.to_string());
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        Ok(())
    }
}
```

### 3.6 Tool Registration

RAG tools register conditionally, similar to `embed.*` tools:

```rust
// In af-rag/src/lib.rs
pub fn register_tools(
    registry: &mut ToolRegistry,
    pool: PgPool,
    embedding_backend: Option<Arc<dyn EmbeddingBackend>>,
    storage_root: PathBuf,
) {
    // Always register rag.search (BM25-only works without embeddings)
    registry.register(rag_search_spec(), rag_search_executor(...));
    registry.register(rag_status_spec(), rag_status_executor(...));

    // Only register full RAG if embedding backend available
    if let Some(backend) = embedding_backend {
        registry.register(rag_ingest_spec(), rag_ingest_executor(...));
        registry.register(rag_query_spec(), rag_query_executor(...));
    }
}
```

### 3.7 Auto-Indexing via Tick

When `doc.ingest` or `doc.chunk` produces a `chunks.json` artifact, also
trigger BM25 indexing alongside the existing embedding queue:

```rust
// In oop_executor.rs ingest_produced_file(), after embed_queue enqueue:
if pf.filename == "chunks.json" {
    if let Some(rag_mgr) = &ctx.rag_index_manager {
        match rag_mgr.index_chunks(ctx.project_id, &chunks).await {
            Ok(count) => tracing::info!("BM25-indexed {count} chunks"),
            Err(e) => tracing::warn!("BM25 indexing failed: {e}"),
        }
    }
}
```

### 3.8 Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `AF_RAG_STORAGE` | `{AF_STORAGE_ROOT}/rag` | BM25 index storage root |
| `AF_RAG_CHUNK_SIZE` | `1000` | Default chunk size for rag.ingest |
| `AF_RAG_MMR_LAMBDA` | `0.7` | Default MMR diversity parameter |
| `AF_RAG_SPARSE_TOP_K` | `50` | BM25 candidates before fusion |
| `AF_RAG_DENSE_TOP_K` | `50` | Vector candidates before fusion |

### 3.9 RAG Agent Preset

New built-in agent for knowledge base Q&A:

```toml
# ~/.af/agents/knowledge.toml (or built-in)
name = "knowledge"
route = "auto"
tools = ["rag.*", "embed.search", "file.read_range"]
timeout_secs = 300

[prompt]
text = """You are a knowledge base assistant. Use rag.query to find relevant
information, then answer the user's question with citations. If the knowledge
base doesn't contain the answer, say so clearly."""
```

### 3.10 Files Changed (Phase 2)

```
NEW:  af/crates/af-rag/Cargo.toml                (~30 lines)
NEW:  af/crates/af-rag/src/lib.rs                 (~80 lines)
NEW:  af/crates/af-rag/src/specs.rs               (~120 lines)
NEW:  af/crates/af-rag/src/executors.rs           (~300 lines)
NEW:  af/crates/af-rag/src/index_manager.rs       (~150 lines)
NEW:  af/crates/af-rag/src/budget.rs              (~100 lines)
EDIT: Cargo.toml                                      (add af-rag to workspace)
EDIT: af/crates/af-jobs/Cargo.toml                (add af-rag dep)
EDIT: af/crates/af-jobs/src/oop_executor.rs       (BM25 auto-index hook, ~15 lines)
EDIT: af/crates/af-cli/Cargo.toml                 (add af-rag dep)
EDIT: af/crates/af-cli/src/commands/serve.rs      (register RAG tools)
EDIT: arbeiterfarm/src/main.rs                        (register RAG tools)
```

Estimated: ~780 lines new, ~50 lines modified.

---

## 4. Phase 3: Verification

**Goal**: Add output verification to catch hallucinated citations and
unsupported claims in agent responses.

### 4.1 Integration Point

After the agent's final text response (no more tool calls), run verification:

```rust
// In runtime.rs, after tool-call loop exits:
if !final_text.is_empty() {
    let verifier = cwc_verify::HeuristicVerifier::new(cwc_verify::VerifierConfig {
        check_citations: true,
        check_abstention: true,
        check_schema: false,  // Arbeiterfarm doesn't use schema-constrained output
    });

    let verdict = verifier.verify(&final_text, &sources_context);

    match &verdict {
        Verdict::Pass => { /* normal */ }
        Verdict::Fail { issues } => {
            tracing::warn!("verification failed: {issues:?}");
            // Optionally: inject a system message asking agent to revise
            // Or: add verification metadata to the response
        }
        Verdict::Abstain { reason } => {
            tracing::info!("agent abstained: {reason}");
        }
    }
}
```

### 4.2 Verification Metadata

Store verdict in the thread's message metadata (JSONB `content_json`):

```rust
// When persisting final assistant message:
let mut content_json = serde_json::json!({});
if let Some(verdict) = &verification_result {
    content_json["verification"] = serde_json::json!({
        "verdict": match verdict {
            Verdict::Pass => "pass",
            Verdict::Fail { .. } => "fail",
            Verdict::Abstain { .. } => "abstain",
        },
        "issues": verdict.issues().map(|i| i.description.clone()).collect::<Vec<_>>(),
    });
}
```

### 4.3 UI Indicator

Add a verification badge to the thread detail view:

```javascript
// In app.js renderThreadDetail():
if (msg.content_json?.verification) {
    const v = msg.content_json.verification;
    const badge = v.verdict === 'pass' ? '&#x2713; Verified'
                : v.verdict === 'abstain' ? '&#x26A0; Insufficient evidence'
                : '&#x2717; Unverified';
    msgEl.innerHTML += `<span class="badge-verify badge-${v.verdict}">${badge}</span>`;
}
```

### 4.4 Files Changed (Phase 3)

```
EDIT: af/crates/af-agents/Cargo.toml              (add cwc-verify dep)
EDIT: af/crates/af-agents/src/runtime.rs           (~30 lines, post-loop verification)
EDIT: af/crates/af-db/src/messages.rs              (verification metadata in content_json)
EDIT: ui/app.js                                        (~10 lines, badge rendering)
EDIT: ui/styles.css                                    (~15 lines, badge styles)
```

Estimated: ~55 lines new, ~20 lines modified.

---

## 5. Migration & Rollout

### 5.1 Phase Ordering

```
Phase 1 (Session)    ████████████████░░░░  ~3-4 hours
Phase 2 (RAG)        ████████████████████  ~4-5 hours
Phase 3 (Verify)     ████░░░░░░░░░░░░░░░░  ~1 hour
                     ─────────────────────
Total                                      ~8-10 hours
```

Phase 1 is the foundation — must be done first since it touches runtime.rs.
Phase 2 is independent and additive.
Phase 3 is a lightweight layer on top of Phase 1.

### 5.2 Feature Flag

Use an environment variable to toggle CWC integration during rollout:

```rust
let use_cwc = std::env::var("AF_USE_CWC").map(|v| v == "1").unwrap_or(false);
```

When `AF_USE_CWC=0` (default initially): old code path runs.
When `AF_USE_CWC=1`: CWC code path runs.

Remove the flag once validated.

### 5.3 Backwards Compatibility

- **DB schema**: No migrations needed for Phase 1. `thread_memory` table stays.
  CWC reads/writes through existing DB layer.
- **API**: No API changes. Internal optimization only.
- **Config**: New env vars are optional with sane defaults.
- **Existing tests**: All 303 tests must continue passing.

### 5.4 Validation Checklist

- [ ] `cargo build --release --workspace` succeeds
- [ ] `cargo test --workspace` — all tests pass
- [ ] Single-agent chat with local model: compaction fires correctly
- [ ] Single-agent chat with cloud model: compaction fires correctly
- [ ] Thread memory persists across compaction events
- [ ] Tool loop detection works (call same tool 4x → corrective nudge)
- [ ] Goal anchoring: goal visible in nudges after sliding window trim
- [ ] Hard reset: after reset, memory + system prompt + last 2 turns preserved
- [ ] RAG ingest: chunks indexed in BM25 + pgvector
- [ ] RAG query: hybrid results with scores, within budget
- [ ] RAG search: BM25-only mode works without embedding backend
- [ ] Verification: pass/fail/abstain verdicts stored in message metadata

---

## 6. File Change Index

### Phase 1: Session Management

| Action | File | Lines |
|--------|------|-------|
| NEW | `af/crates/af-agents/src/cwc_bridge.rs` | ~200 |
| EDIT | `af/crates/af-agents/Cargo.toml` | +2 deps |
| EDIT | `af/crates/af-agents/src/runtime.rs` | ~150 changed |
| EDIT | `af/crates/af-agents/src/mod.rs` | +1 line |
| EDIT | `Cargo.toml` (workspace) | +2 deps |
| REMOVE | `af/crates/af-agents/src/compaction.rs` | -400 |
| REMOVE | `af/crates/af-agents/src/thread_memory.rs` | -200 (extraction) |

### Phase 2: RAG Pipeline

| Action | File | Lines |
|--------|------|-------|
| NEW | `af/crates/af-rag/Cargo.toml` | ~30 |
| NEW | `af/crates/af-rag/src/lib.rs` | ~80 |
| NEW | `af/crates/af-rag/src/specs.rs` | ~120 |
| NEW | `af/crates/af-rag/src/executors.rs` | ~300 |
| NEW | `af/crates/af-rag/src/index_manager.rs` | ~150 |
| NEW | `af/crates/af-rag/src/budget.rs` | ~100 |
| EDIT | `Cargo.toml` (workspace) | +1 member, +5 deps |
| EDIT | `af/crates/af-jobs/src/oop_executor.rs` | ~15 |
| EDIT | `af/crates/af-cli/src/commands/serve.rs` | ~10 |
| EDIT | `arbeiterfarm/src/main.rs` | ~10 |

### Phase 3: Verification

| Action | File | Lines |
|--------|------|-------|
| EDIT | `af/crates/af-agents/Cargo.toml` | +1 dep |
| EDIT | `af/crates/af-agents/src/runtime.rs` | ~30 |
| EDIT | `ui/app.js` | ~10 |
| EDIT | `ui/styles.css` | ~15 |

### Totals

| Phase | New Lines | Modified | Removed | Net |
|-------|-----------|----------|---------|-----|
| Phase 1 | ~200 | ~155 | ~600 | -245 |
| Phase 2 | ~780 | ~35 | 0 | +815 |
| Phase 3 | 0 | ~55 | 0 | +55 |
| **Total** | **~980** | **~245** | **~600** | **+625** |

---

## Appendix A: CWC Crate Dependency Map

```
cwc-core          ← cwc-session (Phase 1)
                  ← cwc-ingest  (Phase 2)
                  ← cwc-index   (Phase 2)
                  ← cwc-retrieve(Phase 2)
                  ← cwc-compile (Phase 2)
                  ← cwc-verify  (Phase 3)

cwc-session       ← af-agents (Phase 1)
cwc-ingest        ← af-rag    (Phase 2)
cwc-index         ← af-rag    (Phase 2)
cwc-retrieve      ← af-rag    (Phase 2)
cwc-compile       ← af-rag    (Phase 2)
cwc-verify        ← af-agents (Phase 3)
```

## Appendix B: Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Cargo version conflicts (tantivy, ort) | Low | Medium | Submodule pins exact versions |
| Token count divergence (Arbeiterfarm heuristic vs CWC) | Medium | Low | Use same AfTokenCounter adapter |
| Memory store drift (CWC in-memory vs DB) | Medium | Medium | Always sync to DB after optimize() |
| Tantivy index corruption on crash | Low | Medium | Tantivy has WAL; re-index on error |
| ONNX model download requirement | Medium | Low | Phase 2 reranking is optional; BM25+vector works without it |
| Large binary size increase | Medium | Low | tantivy ~10MB, tiktoken ~2MB; acceptable |
| Compaction behavior change | Medium | Medium | Feature flag for gradual rollout |
