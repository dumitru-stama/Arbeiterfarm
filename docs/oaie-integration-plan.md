# OAIE Integration Plan — Replacing bubblewrap in Arbeiterfarm

## Executive Summary

Replace AF's bwrap-based sandbox (`spawn_with_bwrap()` in `af-jobs/src/oop_executor.rs`) with OAIE's namespace sandbox. OAIE provides deeper isolation (seccomp + Landlock + cgroups on top of namespaces), observation (ptrace/eBPF tracing), and content-addressed provenance — all without root.

Three phases:
1. **Drop-in sandbox replacement** — OAIE runs AF's executor binaries instead of bwrap
2. **Observation & provenance** — trace tool execution, store manifests
3. **Agent containment** — run agents themselves inside OAIE sessions

---

## Current Architecture (bwrap)

```
Worker::try_execute_one()
  → oop_executor::execute_oop()
    → build OopEnvelope JSON
    → spawn_with_bwrap(config, mounts, sandbox_profile, stdin, timeout)
        → bwrap --tmpfs / --unshare-all --die-with-parent --cap-drop ALL \
              --ro-bind /usr /usr --ro-bind <artifacts>... \
              --bind <scratch> <scratch> \
              /path/to/executor
    → read stdout → parse OopResponse
    → ingest_produced_file() → blob storage + DB
```

**SandboxProfile enum:**
| Profile | Behavior |
|---------|----------|
| `NoNetReadOnly` | `--unshare-all --cap-drop ALL`, RO root, RW scratch |
| `PrivateLoopback` | Same but keeps host network (for Java/Ghidra) |
| `Trusted` | No sandbox — `spawn_direct()` |
| `NetEgressAllowlist` | Planned, not implemented |

**What bwrap provides:** user/mount/net/pid namespace isolation, capability dropping, tmpfs root.

**What bwrap does NOT provide:** seccomp filtering, cgroup resource limits, Landlock LSM, execution tracing, provenance records.

---

## OAIE Capabilities

OAIE provides 8 defense layers vs bwrap's 3:

| Layer | bwrap | OAIE |
|-------|-------|------|
| User namespace | Yes | Yes |
| Mount namespace (pivot_root) | Yes | Yes + credential path denial (24 paths) |
| PID namespace | Yes | Yes |
| Network namespace | Yes | Yes + allowlist mode (veth + nftables + DNS proxy) |
| IPC/UTS namespace | No | Yes |
| Seccomp BPF | No | Yes (69-71 blocked syscalls) |
| Landlock LSM | No | Yes (survives namespace escape) |
| Cgroup v2 limits | No | Yes (memory, PIDs, CPU) |

Plus: ptrace/eBPF observation, content-addressed storage, Ed25519 signed manifests, hash-chained trace logs.

---

## Phase 1: Drop-In Sandbox Replacement

**Goal:** Replace `spawn_with_bwrap()` with OAIE's `OaieClient` API. Same OopEnvelope protocol, same produced_files ingestion — just a different sandbox underneath.

### 1.1 Add `oaie-agent` as a dependency

In `af/crates/af-jobs/Cargo.toml`:
```toml
oaie-agent = { path = "../../oaie/crates/oaie-agent" }
oaie-core = { path = "../../oaie/crates/oaie-core" }
```

### 1.2 Map SandboxProfile → OAIE Policy

| AF Profile | OAIE Policy | Notes |
|------------|-------------|-------|
| `NoNetReadOnly` | `agent-safe` (256MB, 2min, no net) | Closest match; adjust timeout per AF's per-tool timeout |
| `NoNetReadOnlyTmpfs` | `agent-safe` | Same — OAIE always uses tmpfs root |
| `PrivateLoopback` | `agent-analyze` + network=on | For Java/Ghidra needing loopback |
| `NetEgressAllowlist` | `agent-net` + allowlist rules | OAIE has native allowlist support |
| `Trusted` | Keep `spawn_direct()` | In-process tools skip sandbox |

### 1.3 Replace `spawn_with_bwrap()` with `spawn_with_oaie()`

New function in `oop_executor.rs`:

```rust
async fn spawn_with_oaie(
    config: &SpawnConfig,
    mounts: &OaieMounts,
    sandbox: &SandboxProfile,
    stdin_json: &str,
    timeout: Duration,
    stderr_tx: Option<mpsc::Sender<String>>,
) -> Result<ProcessOutput, ToolError> {
    let store_path = std::env::var("AF_OAIE_STORE")
        .unwrap_or_else(|_| "/tmp/af/oaie".to_string());

    // Build OAIE job spec
    let policy = match sandbox {
        SandboxProfile::NoNetReadOnly | SandboxProfile::NoNetReadOnlyTmpfs
            => "agent-safe",
        SandboxProfile::PrivateLoopback
            => "agent-analyze",
        SandboxProfile::NetEgressAllowlist
            => "agent-net",
        _ => unreachable!(),
    };

    let result = oaie_agent::OaieClient::new(&store_path)
        .policy(policy)
        .timeout(timeout)
        .stdin_bytes(stdin_json.as_bytes())
        // Map AF artifact paths as OAIE read-only inputs
        .inputs(mounts.artifact_inputs())
        // Scratch dir as writable output
        .output_dir(&mounts.scratch_dir)
        // Extra RO mounts (system libs, etc)
        .extra_ro_mounts(mounts.extra_ro())
        .run(&[&config.binary_path.to_string_lossy()])
        .map_err(|e| ToolError {
            code: "sandbox_error".into(),
            message: format!("OAIE sandbox failed: {e}"),
            retryable: false,
            details: serde_json::Value::Null,
        })?;

    Ok(ProcessOutput {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
    })
}
```

### 1.4 Update `execute_oop()` dispatch

```rust
// Before:
if bwrap_ok {
    spawn_with_bwrap(...)
}

// After:
if oaie_available() {
    spawn_with_oaie(...)
} else if bwrap_ok {
    spawn_with_bwrap(...)   // keep as fallback during transition
} else if std::env::var("AF_ALLOW_UNSANDBOXED").is_ok() {
    spawn_direct(...)
}
```

### 1.5 UDS Gateway Mounts

OAIE supports extra RO mounts. Map AF's `uds_bind_mounts` (for VT gateway, web gateway sockets) to OAIE's mount specification:

```rust
// AF current: --ro-bind /run/af/vt_gateway.sock /run/af/vt_gateway.sock
// OAIE: extra_ro_mount("/run/af/vt_gateway.sock", "/run/af/vt_gateway.sock")
```

### 1.6 Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `AF_OAIE_STORE` | `/tmp/af/oaie` | OAIE content-addressed store path |
| `AF_OAIE_POLICY` | (per-profile) | Override OAIE policy |
| `AF_SANDBOX_BACKEND` | `oaie` | `oaie` or `bwrap` (fallback) |

### 1.7 What Changes, What Stays

| Component | Changes? | Notes |
|-----------|----------|-------|
| `OopEnvelope` protocol | No | Executor binaries unchanged |
| `OopResponse` parsing | No | Same stdout JSON |
| `ProducedFile` ingestion | No | Same scratch_dir flow |
| `spawn_direct()` (Trusted) | No | In-process tools unchanged |
| `spawn_with_bwrap()` | Kept as fallback | Remove after validation |
| `execute_oop()` dispatch | Yes | Add OAIE path |
| `SandboxProfile` enum | No | Maps to OAIE policies |
| `bwrap_available()` | Augmented | Add `oaie_available()` |
| Executor binaries | No | `af-builtin-executor`, `af-executor` unchanged |

### 1.8 Gains from Phase 1

- Seccomp BPF: blocks 69+ dangerous syscalls (kexec, bpf, ptrace, mount, etc.)
- Landlock LSM: filesystem restrictions survive namespace escape
- Cgroup v2: memory/PID/CPU hard limits per tool invocation
- Credential denial: 24 sensitive paths auto-blocked
- Environment sanitization: blocks `LD_*`, `PYTHONPATH`, `NODE_OPTIONS`, etc.
- No root required (same as bwrap)

---

## Phase 2: Observation & Provenance

**Goal:** Trace tool execution and store provenance records.

### 2.1 Enable Tracing

OAIE supports two trace backends:
- **ptrace** (~20-40% overhead) — full syscall interception, argv capture
- **eBPF** (<5% overhead) — lightweight, needs `CAP_BPF`

For RE tool analysis (Ghidra, rizin, YARA), ptrace tracing provides valuable forensic data. For lighter tools (file.info, transform.*), eBPF or no tracing.

```rust
// Per-tool trace decision
let trace_mode = match tool_name {
    "ghidra.analyze" | "ghidra.decompile" | "sandbox.trace" => TraceMode::Ptrace,
    "rizin.bininfo" | "rizin.disasm" | "yara.scan" => TraceMode::Ebpf,
    _ => TraceMode::Off,
};
```

### 2.2 Store Provenance

Each tool execution produces:
- `manifest.toml` — run_id, command, exit_code, duration, artifacts
- `trace.ndjson` — hash-chained syscall trace (if enabled)
- `signature.toml` — Ed25519 signed manifest

Store manifests as AF artifacts (role='provenance') linked to the tool_run. This creates an auditable chain: who ran what tool, on which sample, what syscalls occurred, what files were produced.

### 2.3 Verification

OAIE's `oaie verify` validates:
- Manifest signature (Ed25519)
- Trace hash chain integrity
- CAS blob integrity

Expose as AF API endpoint: `GET /tool-runs/{id}/verify`.

---

## Phase 3: Agent Containment

**Goal:** Run agents themselves inside OAIE sessions, not just their tools.

### 3.1 Current Agent Architecture

```
AgentRuntime::run_loop()
  → LLM request (cloud/local)
  → parse tool calls
  → invoke tools (via Worker → OOP executor)
  → loop until done
```

Agents run on the host with full access. Only individual tool invocations are sandboxed.

### 3.2 OAIE Session Mode

OAIE sessions provide long-lived sandboxed processes with tool dispatch:

```
OAIE Session
  ├── Agent process (sandboxed, with budget)
  ├── Dispatch socket (tool calls → individual sandbox runs)
  ├── Artifacts directory (shared across tool calls)
  └── Budget (max_tool_calls, max_wall_time, max_output_bytes)
```

### 3.3 Integration Architecture

```
AF Worker
  → create OAIE session (policy=contained-local, budget from agent config)
  → start agent process inside session
  → agent uses dispatch socket for tool calls
  → each tool call = new OAIE sandbox run
  → session budget enforced (prevents runaway agents)
  → session ends → collect results
```

### 3.4 Agent ↔ OAIE Dispatch Bridge

The agent needs LLM access (network) but tools need isolation (no network). OAIE sessions handle this with split network: agent gets allowlist network (to LLM provider), tool calls get isolated network.

```
Agent (in session, network=allowlist[anthropic,openai])
  ↓ dispatch socket
Tool Call (in sub-sandbox, network=off)
  ↓
Result back to agent
```

### 3.5 Benefits

- **Agent crash isolation**: agent OOM/panic doesn't affect host
- **Budget enforcement**: wall clock, tool calls, output bytes — kernel-enforced
- **Credential isolation**: agent can't read host credentials
- **Audit trail**: every tool call traced and signed
- **Multi-agent safety**: thinking thread children can't interfere with each other

### 3.6 Migration Path

1. First: run agents on host, tools via OAIE (Phase 1 — low risk)
2. Then: opt-in agent containment for new/experimental agents
3. Finally: default all agents to OAIE sessions (Phase 3 complete)

---

## Implementation Order

| Step | Scope | Effort | Risk |
|------|-------|--------|------|
| 1.1 Add oaie dependency | Cargo.toml | Small | None |
| 1.2 Map profiles → policies | oop_executor.rs | Small | None |
| 1.3 `spawn_with_oaie()` | oop_executor.rs | Medium | Low — bwrap fallback |
| 1.4 Dispatch logic | oop_executor.rs | Small | Low |
| 1.5 UDS mounts | oop_executor.rs | Small | Medium — test with VT/web |
| 1.6 Env vars & config | config.rs | Small | None |
| 2.1 Trace mode per tool | oop_executor.rs | Small | None |
| 2.2 Provenance storage | worker.rs, DB | Medium | Low |
| 2.3 Verify endpoint | af-api routes | Small | None |
| 3.1-3.6 Agent containment | af-agents, af-jobs | Large | Medium |

---

## Key Files to Modify

| File | Phase | Changes |
|------|-------|---------|
| `af/crates/af-jobs/Cargo.toml` | 1 | Add oaie-agent, oaie-core deps |
| `af/crates/af-jobs/src/oop_executor.rs` | 1 | `spawn_with_oaie()`, dispatch logic |
| `af/crates/af-core/src/types.rs` | 1 | Optional: add OAIE-specific policy fields |
| `af/crates/af-cli/src/config.rs` | 1 | `AF_OAIE_STORE`, `AF_SANDBOX_BACKEND` |
| `af/crates/af-jobs/src/worker.rs` | 2 | Store provenance artifacts |
| `af/crates/af-api/src/routes/tools.rs` | 2 | Verify endpoint |
| `af/crates/af-agents/src/runtime.rs` | 3 | Session-based agent execution |
| `Cargo.toml` (workspace) | 1 | Add oaie crates to workspace deps |

---

## Compatibility Notes

- **Executor binaries unchanged**: `af-builtin-executor` and `af-executor` still read OopEnvelope from stdin, write OopResponse to stdout. OAIE just wraps their execution.
- **bwrap kept as fallback**: `AF_SANDBOX_BACKEND=bwrap` for systems where OAIE isn't available.
- **Trusted profile unchanged**: in-process tools (`email.*`, `notify.*`, `re-ioc.*`) skip sandbox entirely.
- **OAIE store is separate**: OAIE's CAS at `AF_OAIE_STORE` is independent from AF's blob storage. Provenance artifacts get ingested into AF's storage in Phase 2.
- **Linux 5.10+ required**: OAIE needs user namespaces + Landlock. Same as bwrap's requirements in practice.
