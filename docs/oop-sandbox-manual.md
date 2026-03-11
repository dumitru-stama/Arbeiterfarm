# The OOP Executor, Bubblewrap Sandbox, and Artifact Pipeline

A comprehensive guide to how arbeiterfarm executes tools, isolates them in sandboxes, and manages artifacts on disk.

---

## Table of Contents

1. [The Big Picture](#1-the-big-picture)
2. [What is Bubblewrap?](#2-what-is-bubblewrap)
3. [Sandbox Profiles](#3-sandbox-profiles)
4. [The OOP Executor Mechanism](#4-the-oop-executor-mechanism)
5. [How Rizin Tools Work](#5-how-rizin-tools-work)
6. [How Ghidra Tools Work](#6-how-ghidra-tools-work)
7. [The Ghidra Cache: How Analysis is Shared](#7-the-ghidra-cache-how-analysis-is-shared)
8. [Content-Addressed Storage: How Artifacts Live on Disk](#8-content-addressed-storage-how-artifacts-live-on-disk)
9. [The Full Pipeline: From Tool Call to Artifact](#9-the-full-pipeline-from-tool-call-to-artifact)
10. [Scratch Directories: The Temporary Workshop](#10-scratch-directories-the-temporary-workshop)
11. [Security Model](#11-security-model)
12. [Filesystem Layout](#12-filesystem-layout)
13. [Troubleshooting](#13-troubleshooting)

---

## 1. The Big Picture

Imagine a factory where an AI analyst sits at a desk, asking specialists to examine files. Each specialist works in a sealed clean room (sandbox). They receive the file through a slot, do their work, and pass results back through the slot. They cannot see the rest of the factory, cannot access the network, and cannot touch any other files.

That is exactly how arbeiterfarm runs tools like Ghidra and rizin.

```
                                    ┌──────────────────────────┐
 LLM Agent                         │   Bubblewrap Sandbox     │
 ─────────                         │                          │
 "I need to analyze                 │  ┌───────────────────┐  │
  this binary"                      │  │  af-executor  │  │
       │                            │  │                    │  │
       v                            │  │  runs rizin/ghidra │  │
 ┌─────────────┐    OOP Protocol    │  │  writes results to │  │
 │  Job System  │ ───stdin/stdout──>│  │  scratch directory  │  │
 │  (af-jobs) │                   │  └───────────────────┘  │
 └─────────────┘                    │                          │
       │                            │  Can see:                │
       v                            │  - /usr, /lib (read-only)│
 ┌─────────────┐                    │  - input artifact (r/o)  │
 │   Artifact   │                   │  - scratch dir (r/w)     │
 │   Storage    │                   │  - ghidra cache (r/w)    │
 └─────────────┘                    │                          │
                                    │  Cannot see:             │
                                    │  - other files on disk   │
                                    │  - the network           │
                                    │  - environment variables │
                                    │  - other processes       │
                                    └──────────────────────────┘
```

There are two key concepts:

- **OOP (Out-of-Process) Executor**: Tools run as separate processes, not inside the main server. Communication happens via JSON over stdin/stdout — like two programs talking through a pipe.

- **Bubblewrap (bwrap)**: A lightweight Linux sandboxing tool that creates a minimal, isolated filesystem for each tool execution. Think of it as a shipping container for programs — they can only see what you explicitly put inside.

---

## 2. What is Bubblewrap?

[Bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) is a sandboxing tool that uses Linux namespaces to create lightweight containers. Unlike Docker (which is designed for long-running services), bwrap is designed for single-command isolation — run one program in a restricted environment, then tear it down.

### Real-world analogy

Think of a hospital operating room. Before surgery:
- The room is completely sterile (empty tmpfs root — nothing from the host leaks in)
- Only the specific instruments needed are brought in (bind mounts for artifacts, tools)
- The surgeon can only use what's on the instrument tray (selective read-only mounts)
- Waste goes in a designated bin (writable scratch directory)
- The room is sealed from the rest of the hospital (namespace isolation)
- After the procedure, the room is cleaned (scratch dir deleted)

### How arbeiterfarm builds a bwrap command

When a tool needs to run, the system constructs a bwrap command that looks roughly like this:

```bash
bwrap \
  # 1. Start with an empty filesystem
  --tmpfs /                           # Root is empty (ephemeral RAM disk)

  # 2. Mount just the system libraries the tool needs (read-only)
  --ro-bind /usr /usr                 # System binaries and libraries
  --ro-bind /lib /lib                 # Shared libraries
  --ro-bind /lib64 /lib64             # 64-bit shared libraries
  --ro-bind /etc /etc                 # Config files (SSL certs, linker cache)

  # 3. Virtual filesystems
  --dev /dev                          # Device nodes
  --proc /proc                        # Process information
  --tmpfs /tmp                        # Temporary files (RAM-backed)

  # 4. Mount the specific files this tool needs
  --ro-bind /storage/.../abc123... /storage/.../abc123...   # Input artifact (read-only)
  --bind /scratch/tool-run-id/ /scratch/tool-run-id/        # Scratch dir (read-write)
  --bind /ghidra-cache/ /ghidra-cache/                      # Ghidra cache (read-write, if Ghidra)
  --ro-bind /home/tools/ghidra/ /home/tools/ghidra/         # Ghidra installation (read-only)

  # 5. For ~/.ghidra (Ghidra writes logs and OSGi cache here)
  --bind /home/user/.ghidra /home/user/.ghidra              # Writable!

  # 6. Isolation settings
  --unshare-all                       # New user/network/PID/IPC namespaces
  --die-with-parent                   # Kill sandbox if parent dies
  --cap-drop ALL                      # Drop ALL Linux capabilities

  # 7. Clean environment — NO inherited variables
  --clearenv                          # Wipe all env vars (no API keys leak in)
  --setenv PATH /usr/local/bin:/usr/bin:/bin
  --setenv HOME /home/user

  # 8. The executor binary itself
  --ro-bind /path/to/af-executor /path/to/af-executor
  /path/to/af-executor
```

### What each part does

| bwrap flag | Purpose | Real-world parallel |
|---|---|---|
| `--tmpfs /` | Empty root filesystem | Starting with a blank room |
| `--ro-bind /usr /usr` | System libraries, read-only | Bolting tool cabinets to the wall |
| `--bind <scratch> <scratch>` | Writable output directory | The workbench where results go |
| `--ro-bind <artifact> <artifact>` | Input file, read-only | The specimen slide under the microscope |
| `--unshare-all` | New PID/network/user namespaces | Soundproof walls, no windows |
| `--cap-drop ALL` | Remove all Linux capabilities | No master keys |
| `--clearenv` | Wipe environment variables | No post-it notes with passwords |
| `--die-with-parent` | Kill if parent dies | Fire alarm kills all activity |

**Key insight**: The artifact is mounted at its *exact disk path* inside the sandbox. The executor binary sees the same path it would outside — this means path references in the OOP envelope are valid inside the sandbox without any translation.

---

## 3. Sandbox Profiles

Not all tools need the same level of isolation. A tool that runs rizin (a native binary) needs full namespace isolation. Ghidra (which runs Java) breaks under full isolation because Java's runtime needs loopback networking. The `SandboxProfile` enum captures these differences:

```rust
pub enum SandboxProfile {
    NoNetReadOnly,       // Full isolation — default for most tools
    NoNetReadOnlyTmpfs,  // Full isolation + tmpfs working area
    PrivateLoopback,     // Selective isolation — for Java/Ghidra
    NetEgressAllowlist,  // Network allowed with allowlist
    Trusted,             // No sandbox at all (in-process tools)
}
```

### Profile comparison

| Profile | User namespace | Network | PID isolation | Used by |
|---|---|---|---|---|
| `NoNetReadOnly` | Isolated | None | Isolated | rizin tools, strings.extract |
| `PrivateLoopback` | Host | Host loopback | Host | Ghidra tools (Java needs this) |
| `Trusted` | N/A | N/A | N/A | email tools, in-process tools |

### Why Ghidra needs PrivateLoopback

Java's runtime (JVM) does several things at startup that break under full namespace isolation:

1. **User namespace**: Java checks user identity via `/etc/passwd` and native calls. With `--unshare-user`, the UID mapping is different, causing confusion.
2. **Network namespace**: Java's `InetAddress.getLocalHost()` needs loopback (`127.0.0.1`). With `--unshare-net`, there's no network stack at all — not even loopback. This causes Java to hang.
3. **PID namespace**: Ghidra's OSGi framework (Felix) manages bundles using PID files and process checks. New PID namespaces can confuse these mechanisms.

The fix: `PrivateLoopback` skips `--unshare-all` entirely but keeps everything else — tmpfs root, selective bind mounts, `--cap-drop ALL`, `--clearenv`. The sandbox is still very restrictive, just not namespace-isolated.

---

## 4. The OOP Executor Mechanism

### What "Out-of-Process" means

The main af server never runs rizin or Ghidra directly. Instead, it spawns a separate binary (`af-executor`) as a child process, communicates with it over stdin/stdout using a JSON protocol, and collects the results.

This is like a manager (the job system) handing a work order to a contractor (the executor). The work order is a JSON document, the contractor works independently, and returns a JSON result.

### The OOP Protocol

**Step 1: Handshake** (at startup)

When the system first discovers the executor binary, it runs it with `--handshake` to learn what tools it supports:

```bash
$ af-executor --handshake
```
```json
{
  "protocol_version": 1,
  "supported_tools": [
    { "name": "rizin.bininfo", "version": 1 },
    { "name": "rizin.disasm",  "version": 1 },
    { "name": "rizin.xrefs",   "version": 1 },
    { "name": "strings.extract", "version": 1 },
    { "name": "ghidra.analyze",  "version": 1 },
    { "name": "ghidra.decompile", "version": 1 },
    { "name": "ghidra.rename",    "version": 1 },
    { "name": "vt.file_report",   "version": 1 }
  ]
}
```

**Step 2: Execution** (per tool call)

For each tool invocation, the system spawns a new process (inside bwrap), writes an `OopEnvelope` to stdin, and reads an `OopResponse` from stdout.

**OopEnvelope (stdin → executor)**:
```json
{
  "tool_name": "rizin.bininfo",
  "tool_version": 1,
  "input": {
    "artifact_id": "8c009d2e-169d-4f16-9ac1-eaeb347ce973"
  },
  "context": {
    "project_id": "a1b2c3d4-...",
    "tool_run_id": "e5f6a7b8-...",
    "scratch_dir": "/tmp/af/scratch/e5f6a7b8-...",
    "artifacts": [
      {
        "id": "8c009d2e-...",
        "sha256": "3f4e8a9b2c...",
        "filename": "malware.exe",
        "storage_path": "/tmp/af/storage/data/3f/4e/3f4e8a9b2c...",
        "size_bytes": 245760,
        "mime_type": "application/octet-stream"
      }
    ],
    "actor_user_id": "user-uuid-...",
    "extra": {
      "rizin_path": "/usr/bin/rizin",
      "ghidra_home": "/home/ds/tools/ghidra_11.4.2_PUBLIC",
      "cache_dir": "/tmp/af/ghidra_cache",
      "scripts_dir": "/home/ds/tools/ghidra-scripts"
    }
  }
}
```

Key things to notice:
- The `artifacts` array contains the **full disk path** (`storage_path`) so the executor can open the file directly
- The `scratch_dir` is the writable directory where the tool writes its output files
- The `extra` field carries tool-specific configuration (paths to rizin, Ghidra, etc.)

**OopResponse (executor → stdout)**:
```json
{
  "result": {
    "status": "ok",
    "output": {
      "summary": "ELF x86_64, dynamically linked, 142 imports, 8 exports...",
      "produced_artifact_ids": ["new-artifact-uuid"]
    },
    "produced_files": [
      {
        "filename": "bininfo.json",
        "path": "bininfo.json",
        "mime_type": "application/json",
        "description": "Full rizin binary info: imports, exports, sections..."
      }
    ]
  }
}
```

Or on error:
```json
{
  "result": {
    "status": "error",
    "code": "exec_failed",
    "message": "rizin crashed with exit code 1",
    "retryable": false
  }
}
```

### Artifact-First Output

A critical design pattern: tools that produce large output (thousands of lines of disassembly, full decompilation, hundreds of strings) **never** send it all inline to the LLM. Instead they:

1. Write the full result to a file in the scratch directory (e.g., `bininfo.json`)
2. Declare it as a `produced_file` in the response
3. Return a compact **summary** as the inline `output`

The summary contains just enough context for the LLM to decide what to look at next — counts, top-N items, and a hint to use `file.read_range` or `file.grep` to inspect details.

Why? Local LLMs (gpt-oss, qwen3, llama) get overwhelmed by 24KB+ of inline data. They lose track of their analysis plan and start repeating themselves or hallucinating. With artifact-first output, the LLM sees a 200-byte summary and can surgically inspect the parts it needs.

| Tool | Artifact file | Inline summary contains |
|---|---|---|
| `rizin.bininfo` | `bininfo.json` | Architecture, security flags, import/export counts |
| `rizin.disasm` | `disasm.json` | Address, instruction count, first 10 instructions |
| `rizin.xrefs` | `xrefs.json` | Counts per direction, top 10 xrefs |
| `strings.extract` | `strings.json` | Total count, top 20 strings |
| `ghidra.analyze` | `functions.json` | Function index (name + address), thunk list |
| `ghidra.decompile` | `decompiled.json` | Function names and line counts |

---

## 5. How Rizin Tools Work

[Rizin](https://rizin.re/) is a fast, scriptable binary analysis framework. The executor calls it as a subprocess with specific commands, parses its JSON output, and produces artifacts.

### rizin.bininfo

**What it does**: Extracts comprehensive metadata about a binary — architecture, format, security flags, imported/exported symbols, sections, and entry points. This is usually the first tool called during analysis.

**How it works**:
```bash
rizin -q -c "iIj;iij;iEj;iSj;iej" /path/to/binary
```

Each command is a rizin "info" command that outputs JSON:
- `iIj` — Binary info (arch, bits, endianness, OS, format, security flags like NX/PIE/canary)
- `iij` — Imports (functions the binary calls from shared libraries)
- `iEj` — Exports (symbols the binary exposes)
- `iSj` — Sections (.text, .data, .bss, .rodata — with sizes, permissions)
- `iej` — Entry points (program entry + init/fini functions)

The `-q` flag means quiet mode (suppress the interactive banner), and `-c` runs commands non-interactively.

**Output flow**:
1. Full result → `scratch_dir/bininfo.json` (artifact)
2. Compact summary → inline JSON with key counts and notable imports (filtering out internal libc symbols like `__libc_start_main`)

### rizin.disasm

**What it does**: Disassembles machine code at a specific address into human-readable assembly instructions.

```bash
rizin -q -c "aa;pdj 100 @ 0x00401000" /path/to/binary
```

- `aa` — Auto-analysis (detects functions, cross-references)
- `pdj 100 @ 0x00401000` — Print 100 instructions as JSON starting at address `0x00401000`

The address is validated with `is_valid_hex_address()` to prevent command injection — only hex characters after `0x` are allowed.

### rizin.xrefs

**What it does**: Finds cross-references — who calls this function, or what does this function call.

```bash
rizin -q -c "aa;axtj @ 0x00401000" /path/to/binary    # Who calls this address?
rizin -q -c "aa;axfj @ 0x00401000" /path/to/binary    # What does this address call?
```

- `axtj` — Cross-references TO this address (callers)
- `axfj` — Cross-references FROM this address (callees)

### strings.extract

**What it does**: Extracts embedded strings from the binary, with encoding detection.

```bash
rizin -q -c "izzj" /path/to/binary
```

- `izzj` — All strings (including cross-section), with encoding info, as JSON

Results are filtered by minimum length and encoding type, then capped at `max_strings`.

---

## 6. How Ghidra Tools Work

[Ghidra](https://ghidra-sre.org/) is the NSA's open-source reverse engineering framework. Unlike rizin (which is fast and lightweight), Ghidra performs deep program analysis — it disassembles, decompiles to C pseudocode, recovers types, builds control flow graphs, and much more. The trade-off is that initial analysis is slow (30 seconds to several minutes depending on binary size).

### ghidra.analyze

**What it does**: Runs Ghidra's headless analyzer on a binary, stores the analysis project in a persistent cache, and extracts a function list.

**Phase 1: Analysis** (if not cached)

```bash
analyzeHeadless /cache/project-id/sha256/ analysis \
    -import /storage/.../binary \
    -overwrite \
    -analysisTimeoutPerFile 240
```

This tells Ghidra to:
- Create a project at the given directory
- Import the binary
- Run all analysis passes (disassembly, decompilation, type recovery, etc.)
- Time out after 240 seconds per file

The analysis produces a Ghidra project directory structure:
```
/cache/project-id/sha256/
├── analysis.gpr              # Project marker file (0 bytes in Ghidra 11.4+)
├── analysis.rep/             # Repository directory
│   └── idata/                # Actual analysis database
│       ├── ~index.dat
│       ├── ~index.bak
│       └── 00/
│           └── 00000000.prp  # Program properties
└── .analysis.lock            # File lock for concurrency control
```

**Phase 2: Function extraction** (always runs, uses cached analysis)

```bash
analyzeHeadless /cache/project-id/sha256/ analysis \
    -process binary.exe \
    -noanalysis \
    -scriptPath /path/to/scripts \
    -postScript ListFunctionsJSON.java /scratch/functions.json
```

The `-noanalysis` flag is critical — it tells Ghidra to open the existing project without re-analyzing. The `ListFunctionsJSON.java` script iterates all functions and writes their names, addresses, and sizes to a JSON file.

**Output**: The inline summary is a function index:
```json
{
  "program_info": { "image_base": "0x00400000", "entry_points": ["0x00401060"] },
  "total_functions": 47,
  "code_functions": 23,
  "thunks": 24,
  "fn_index": [
    { "name": "main", "address": "0x00401150" },
    { "name": "parse_header", "address": "0x00401200" },
    ...
  ],
  "imported_symbols": ["printf", "malloc", "free", ...],
  "cached": true
}
```

### ghidra.decompile

**What it does**: Takes a list of function names or addresses and decompiles them to C pseudocode.

```bash
analyzeHeadless /cache/project-id/sha256/ analysis \
    -process binary.exe \
    -noanalysis \
    -scriptPath /path/to/scripts \
    -postScript DecompileFunctionsJSON.java \
    "main,parse_header,0x00401300" \
    /scratch/decompiled.json
```

Functions are passed as a comma-separated list. The script finds each function (by name or address), decompiles it, and writes the C code to JSON.

**Output artifact** (`decompiled.json`):
```json
{
  "functions": [
    {
      "name": "main",
      "address": "0x00401150",
      "decompiled": "int main(int argc, char **argv) {\n    ...\n}",
      "line_count": 42
    },
    ...
  ]
}
```

The description now includes function names: `"Ghidra decompiled: main, parse_header"` (or `"... (23 total)"` if there are many).

### ghidra.rename

**What it does**: Renames functions in the Ghidra project. This is persistent — subsequent `ghidra.decompile` calls will show the new names, even in cross-references.

This is powerful for iterative analysis: the LLM decompiles a function, understands what it does, renames it from `FUN_00401200` to `parse_header`, and all future decompilations will reference `parse_header` instead of the cryptic default name.

```bash
analyzeHeadless /cache/project-id/sha256/ analysis \
    -process binary.exe \
    -noanalysis \
    -scriptPath /path/to/scripts \
    -postScript RenameFunctionJSON.java \
    "FUN_00401200=parse_header,FUN_00401300=decode_payload" \
    /scratch/rename_result.json
```

---

## 7. The Ghidra Cache: How Analysis is Shared

This is one of the most elegant parts of the system. Ghidra analysis is expensive (30 seconds to minutes), but the result is deterministic for a given binary. The cache ensures each binary is analyzed only once.

### Cache structure

```
$AF_GHIDRA_CACHE/
└── {project_id}/
    └── {artifact_sha256}/
        ├── analysis.gpr           # Ghidra project marker
        ├── analysis.rep/
        │   └── idata/             # The actual analysis database
        ├── .analysis.lock         # Concurrency lock file
        └── ...
```

The cache key is `(project_id, sha256)`. This means:

- **Same binary, same project, different conversations** → cache hit. The second conversation gets instant results.
- **Same binary, different projects** → depends on NDA status. Non-NDA projects share a cache at `shared/{sha256}/` for efficiency. NDA-flagged projects use isolated caches at `{project_id}/{sha256}/` to prevent cross-project data leakage. On NDA flag read failure, the system defaults to isolated (fail-safe).

### How the cache is shared across conversations

Here's a concrete scenario:

1. **Monday**: You upload `malware.exe` (SHA256: `abc123...`) to Project Alpha and start a conversation with a thinking agent. The agent calls `ghidra.analyze`. Ghidra runs for 45 seconds. The analysis is cached at `/cache/project-alpha-id/abc123.../`.

2. **Tuesday**: You start a new conversation in Project Alpha to look at the same binary from a different angle. The agent calls `ghidra.analyze`. The cache check finds that `/cache/project-alpha-id/abc123.../analysis.gpr` exists AND `analysis.rep/idata/` is a valid directory. Analysis is **skipped entirely**. The function list is extracted in 2 seconds instead of 45.

3. **Tuesday (continued)**: The agent calls `ghidra.decompile` for `main`. It opens the cached project (no analysis needed), runs the decompile script, and returns C pseudocode. Then it calls `ghidra.rename` to rename `FUN_00401200` to `parse_header`. This rename is saved to the cached project.

4. **Wednesday**: Yet another conversation calls `ghidra.decompile` for a different function. That function's decompiled output now shows `parse_header` instead of `FUN_00401200` in cross-references — because the rename was saved to the shared cache.

5. **Thursday**: You upload the same `malware.exe` to Project Beta. A thinking agent calls `ghidra.analyze`. Even though the binary is identical, Project Beta gets its own analysis because the cache key includes `project_id`. This is a fresh 45-second analysis.

### Concurrency control

What happens if two conversations in the same project try to analyze the same binary simultaneously? File locking prevents corruption:

```rust
fn acquire_cache_lock(project_dir: &Path) -> Option<std::fs::File> {
    let lock_path = project_dir.join(".analysis.lock");
    let file = std::fs::File::create(&lock_path)?;
    libc::flock(file.as_raw_fd(), libc::LOCK_EX);  // Block until lock is acquired
    Some(file)
}
```

- `ghidra.analyze` acquires an **exclusive lock** (`LOCK_EX`). Only one analysis can run at a time for a given SHA256.
- After acquiring the lock, it re-checks the cache (another process may have completed analysis while we were waiting).
- The lock is released automatically when the `File` is dropped (Rust's RAII).

### Cache validity checking

Ghidra 11.4+ changed how project files work — the `.gpr` file is now a 0-byte marker. Checking its file size (the old approach) would always say "empty = invalid". Instead, the system checks for the actual analysis database:

```rust
let gpr_path = project_dir.join("analysis.gpr");
let rep_idata = project_dir.join("analysis.rep/idata");

// Both must exist for the cache to be valid
let valid = gpr_path.exists() && rep_idata.is_dir();
```

If the `.gpr` exists but `analysis.rep/idata/` doesn't (incomplete/corrupted analysis), the entire cache directory is deleted and analysis is re-run.

---

## 8. Content-Addressed Storage: How Artifacts Live on Disk

Every file stored in arbeiterfarm is identified by its SHA-256 hash. This is called **content-addressed storage** — the file's address (path) is derived from its content.

### Real-world analogy

Think of a library where books are shelved by fingerprint instead of title. Two copies of the same book (same content) have the same fingerprint and go to the same shelf. If you try to add a duplicate, the library says "already have it" and doesn't waste shelf space.

### How it works

When a tool produces a file (say `bininfo.json`), the storage system:

1. **Reads the file** into memory
2. **Computes SHA-256**: `sha256(content) → "3f4e8a9b2c1d..."`  (64 hex characters)
3. **Determines the path** using the first 4 hex characters as directory shards:
   ```
   storage_root/data/3f/4e/3f4e8a9b2c1d7890...
                      ^^  ^^  ^^^^^^^^^^^^^^^^^^^^^^
                      │   │   └── Full 64-char hash
                      │   └── Second 2 chars (sub-directory)
                      └── First 2 chars (directory)
   ```
4. **Writes atomically**: First writes to a temp file (`storage_root/tmp/blob-{uuid}`), then atomically renames to the final path. This ensures no reader ever sees a half-written file.
5. **Handles deduplication**: If the rename fails because the file already exists (another process stored the same content), that's fine — the content is identical, so we just clean up the temp file.
6. **Records in database**: Upserts a row in the `blobs` table linking the SHA-256 hash to the disk path and file size.

```
storage_root/
├── data/
│   ├── 3f/
│   │   └── 4e/
│   │       └── 3f4e8a9b2c1d7890abcdef...  ← actual file bytes
│   ├── a1/
│   │   └── b2/
│   │       └── a1b2c3d4e5f67890abcdef...
│   └── ...
└── tmp/
    └── blob-{uuid}                          ← temp files during writes
```

### Why content-addressed?

1. **Automatic deduplication**: If the same binary is uploaded to multiple projects, only one copy is stored on disk. Each project's `artifacts` table row points to the same blob.

2. **Integrity verification**: The filename IS the hash. You can verify any file hasn't been corrupted by re-hashing it and comparing to its filename.

3. **Concurrent safety**: The atomic write + dedup race handling means multiple tool executions can store the same content simultaneously without corruption.

4. **Constant-time writes**: The system always writes to temp then renames, even if the blob already exists. This eliminates a timing oracle that could reveal whether a particular file content already exists in storage (a security consideration for multi-tenant systems).

### The database layer

Two tables track artifacts:

**`blobs` table** — Physical storage (shared, deduplicated):
```
sha256 (PK)     | size_bytes | storage_path
─────────────────────────────────────────────────
3f4e8a9b2c...   | 1024       | /storage/data/3f/4e/3f4e8a...
a1b2c3d4e5...   | 245760     | /storage/data/a1/b2/a1b2c3...
```

**`artifacts` table** — Logical artifacts (per-project, with metadata):
```
id (PK)     | project_id | sha256          | filename      | source_tool_run_id | description
─────────────────────────────────────────────────────────────────────────────────────────────────
uuid-1      | proj-A     | a1b2c3d4e5...  | malware.exe   | NULL               | NULL
uuid-2      | proj-A     | 3f4e8a9b2c...  | bininfo.json  | run-uuid-1         | Full rizin binary info...
uuid-3      | proj-B     | a1b2c3d4e5...  | sample.bin    | NULL               | NULL
```

Notice:
- `uuid-1` and `uuid-3` point to the same blob (`a1b2c3d4e5...`) — same file in two projects, one copy on disk.
- `uuid-2` has `source_tool_run_id` set — it was produced by a tool (not uploaded by a user). This distinguishes uploaded samples from generated analysis artifacts.
- The `description` field is what appears in the LLM's system prompt next to each artifact.

**`tool_run_artifacts` table** — Links artifacts to tool executions:
```
tool_run_id | artifact_id | role
───────────────────────────────────
run-uuid-1  | uuid-1      | input    ← "this tool consumed this artifact"
run-uuid-1  | uuid-2      | output   ← "this tool produced this artifact"
```

---

## 9. The Full Pipeline: From Tool Call to Artifact

Here's the complete journey when an LLM agent calls `rizin.bininfo` with `artifact_id: "#1"`:

### Step 1: Agent Runtime (translate and dispatch)

The agent runtime receives the tool call from the LLM. It translates `#1` to the actual UUID (using the index map built from the system prompt), validates the tool name, and creates a `ToolRequest`.

### Step 2: Enqueue (validate and record)

The job system validates the request:
- Extracts artifact IDs from the input JSON using schema paths (`$ref: "#/$defs/ArtifactId"`)
- Verifies each artifact exists and belongs to the project (prevents cross-tenant access)
- Checks user quota (concurrent run limits)
- Inserts a `tool_runs` row with `status="queued"`
- Links input artifacts via `tool_run_artifacts` with `role="input"`

### Step 3: Worker Claims Job

A background worker claims the job:
- `UPDATE tool_runs SET status='running' WHERE status='queued' ... FOR UPDATE SKIP LOCKED`
- Starts a heartbeat task (extends lease every 30 seconds so the job isn't reaped as stale)

### Step 4: Context Building

The worker builds a `ToolContext`:
- Queries `tool_run_artifacts` for input artifacts
- Resolves each artifact's SHA-256 → blob → disk path
- Creates a scratch directory: `/tmp/af/scratch/{tool_run_id}/`

### Step 5: OOP Execution

The worker spawns the executor inside bwrap:
- Builds the bwrap command with appropriate mounts and isolation
- Serializes the `OopEnvelope` (input + context + artifact paths)
- Pipes it to stdin, collects stdout/stderr with timeout

### Step 6: Executor Does the Work

Inside the sandbox, `af-executor`:
- Parses the envelope
- Dispatches to the right tool function
- Runs rizin/Ghidra as a subprocess
- Writes results to the scratch directory
- Returns `OopResponse` with a summary and list of produced files

### Step 7: Produced File Ingestion

For each produced file:
1. **Security check**: Path is relative, no `..` components, resolves under scratch_dir
2. **Read and hash**: `SHA-256(file_content) → hex_hash`
3. **Store blob**: Write to `storage_root/data/{xx}/{yy}/{hash}` (atomic)
4. **Create artifact**: Insert into `artifacts` table with `source_tool_run_id` set
5. **Link output**: Insert into `tool_run_artifacts` with `role="output"`

### Step 8: Completion

- Update `tool_runs`: `status="completed"`, store output JSON
- Clean up scratch directory
- Return `ToolResult` with inline summary and produced artifact IDs

### Step 9: Back to the Agent

The agent runtime receives the tool result. The inline summary is added to the conversation as a tool response message. The LLM sees something like:

```
rizin.bininfo result:
  Architecture: x86_64, Format: ELF, Endian: little
  Security: NX enabled, PIE enabled, canary found
  Imports: 142 (printf, malloc, free, socket, connect...)
  Exports: 8 (main, init_module, cleanup_module...)
  Sections: 28 (.text: 0x1200 bytes, .rodata: 0x800 bytes...)

  Full output stored as artifact #2. Use file.read_range or file.grep to inspect.
```

The LLM can now call `file.read_range` on artifact `#2` to inspect specific parts of the full bininfo output.

---

## 10. Scratch Directories: The Temporary Workshop

Each tool invocation gets its own scratch directory:

```
/tmp/af/scratch/
├── e5f6a7b8-1234-...../     ← tool run 1 (active)
│   ├── bininfo.json          ← produced by rizin
│   └── (cleaned up after ingestion)
├── f9a0b1c2-5678-...../     ← tool run 2 (active)
│   ├── decompiled.json
│   └── functions.json
└── (empty after cleanup)
```

**Lifecycle**:
1. **Created** before tool execution: `scratch_root/{tool_run_id}/`
2. **Written to** by the executor (inside the sandbox, this is the only writable directory besides Ghidra cache)
3. **Ingested** after execution: files listed in `produced_files` are read, hashed, and stored in content-addressed storage
4. **Deleted** after ingestion: `tokio::fs::remove_dir_all(scratch_dir)`

**Orphan cleanup**: If a worker crashes mid-execution, scratch dirs may be left behind. A periodic cleanup task removes directories older than a configurable `max_age`:

```rust
pub async fn cleanup_stale_dirs(scratch_root: &Path, max_age: Duration) -> Result<u64, ...> {
    // Iterate dirs, check modification time, remove if older than cutoff
}
```

---

## 11. Security Model

### Defense in depth

The system has multiple layers of security, each catching what the previous layer might miss:

**Layer 1: Input validation**
- Tool input is validated against JSON schema before execution
- Artifact IDs are verified to belong to the project (no cross-tenant access)
- Hex addresses are validated (prevents command injection in rizin)
- Input size and depth limits are enforced

**Layer 2: Process isolation (bwrap)**
- Empty root filesystem (no accidental access to host files)
- Network disabled for analysis tools
- All Linux capabilities dropped
- Environment variables cleared (no API key leakage)
- Process dies if parent dies (no orphaned sandboxes)

**Layer 3: Filesystem restrictions**
- Input artifacts: read-only
- System libraries: read-only
- Scratch directory: only writable area (besides Ghidra cache)
- Ghidra cache: writable but scope-limited

**Layer 4: Output validation**
- Produced file paths are validated (no `..`, must be under scratch_dir)
- File sizes are checked against limits
- Artifact counts are limited per tool policy

**Layer 5: Database isolation**
- Per-project artifact scoping
- Row-level security for multi-tenant access
- Tool run artifacts linked with explicit role ("input"/"output")
- NDA projects excluded from cross-project queries (all 9 cross-project queries use `af_shareable_projects()`)
- RLS defense-in-depth on `re.ghidra_function_renames` and `re.yara_rules` (migration 007)
- NDA flag changes audited in immutable `audit_log` (actor, old/new values)

### What happens if a tool is malicious?

Even if a tool tries to escape:

| Attack | Defense |
|---|---|
| Read `/etc/shadow` | Not mounted (tmpfs root) |
| Read other artifacts | Only requested artifacts are bind-mounted |
| Write to host filesystem | Only scratch_dir and Ghidra cache are writable |
| Access network | Network namespace isolated (NoNetReadOnly) |
| Read API keys from environment | `--clearenv` wipes all env vars |
| Spawn persistent processes | `--die-with-parent` kills everything when job finishes |
| Escalate privileges | `--cap-drop ALL` removes all capabilities |
| Path traversal in produced files | Canonicalization + scratch_dir containment check |

### The Trusted exception

Some tools (like email, web gateway) need network access or host integration and run as `Trusted` (no sandbox). These are in-process tools with their own security controls (recipient allowlists, SSRF protection, rate limiting).

---

## 12. Filesystem Layout

Complete layout of all directories used by the system:

```
/tmp/af/                              # Default root (configurable via env vars)
├── storage/                            # AF_STORAGE_ROOT — content-addressed blobs
│   ├── data/                           # Blob data, sharded by SHA-256 prefix
│   │   ├── 3f/                         # First 2 hex chars
│   │   │   └── 4e/                     # Next 2 hex chars
│   │   │       └── 3f4e8a9b2c1d...     # Full SHA-256 hash = filename
│   │   ├── a1/
│   │   │   └── b2/
│   │   │       └── a1b2c3d4e5f6...
│   │   └── ...
│   └── tmp/                            # Temp files during atomic writes
│       ├── blob-{uuid}                 # Temp blob (renamed to final path)
│       └── upload-{uuid}               # Temp upload (streaming uploads)
│
├── scratch/                            # AF_SCRATCH_ROOT — tool work areas
│   ├── {tool_run_id_1}/               # One dir per active tool execution
│   │   ├── bininfo.json               # Produced file (ingested → storage)
│   │   └── ...
│   └── {tool_run_id_2}/
│       └── decompiled.json
│
└── ghidra_cache/                       # AF_GHIDRA_CACHE — persistent analysis
    ├── {project_id_1}/                 # Per-project isolation
    │   ├── {sha256_of_binary_A}/       # Per-binary cache
    │   │   ├── analysis.gpr            # Ghidra project marker (0 bytes in 11.4+)
    │   │   ├── analysis.rep/           # Ghidra repository
    │   │   │   └── idata/              # Analysis database
    │   │   │       ├── ~index.dat
    │   │   │       └── 00/
    │   │   │           └── 00000000.prp
    │   │   └── .analysis.lock          # flock() concurrency control
    │   └── {sha256_of_binary_B}/
    │       └── ...
    └── {project_id_2}/
        └── ...

~/.ghidra/                              # Ghidra user config (writable in sandbox)
└── .ghidra_11.4.2_PUBLIC/
    ├── java_home.save                  # Saved JDK path
    ├── application.log                 # Ghidra runtime log
    ├── script.log                      # Script execution log
    └── osgi/
        └── felixcache/
            └── cache.lock              # OSGi bundle cache lock
```

---

## 13. Troubleshooting

### Ghidra analysis hangs

**Symptom**: `ghidra.analyze` never completes, worker times out after 5 minutes.

**Cause**: Usually Java namespace issues. Check that the tool spec uses `SandboxProfile::PrivateLoopback`, not `NoNetReadOnly`.

**Fix**: Verify in `specs.rs` that Ghidra tools use `PrivateLoopback`.

### Ghidra cache is stale or corrupt

**Symptom**: `ghidra.decompile` fails with "no valid analysis project".

**Cause**: Incomplete analysis left `.gpr` but no `analysis.rep/idata/`.

**Fix**: Delete the cache directory for that SHA256:
```bash
rm -rf /tmp/af/ghidra_cache/{project_id}/{sha256}/
```
The next `ghidra.analyze` call will re-create it.

### "sandbox unavailable" error

**Symptom**: Tool execution fails with "sandbox unavailable".

**Cause**: `bwrap` is not installed or not in PATH.

**Fix**: Install bubblewrap (`apt install bubblewrap`) or set `AF_ALLOW_UNSANDBOXED=1` for development (NOT production).

### Scratch directories accumulating

**Symptom**: `/tmp/af/scratch/` growing large.

**Cause**: Worker crashed before cleanup. Orphaned scratch dirs remain.

**Fix**: Run periodic cleanup or manually remove old directories. The system has a `cleanup_stale_dirs()` function that removes dirs older than a configured age.

### Tool produces too many artifacts

**Symptom**: Error about `max_produced_artifacts` exceeded.

**Cause**: The tool declared more `produced_files` than its policy allows. RE tools default to `max_produced_artifacts: 4`.

**Fix**: Either reduce the number of produced files in the tool implementation or increase the limit in the tool spec.
