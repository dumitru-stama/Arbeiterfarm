# Local Tool Definitions

These TOML files mirror every OOP (out-of-process) tool that ships compiled into
`af`. They are **not loaded at runtime** — the compiled-in versions take
precedence. They exist as reference material for writing your own tools.

## Quick start: creating a new tool

1. Write your tool binary (any language). It reads JSON from stdin, writes JSON to stdout.
2. Create a `.toml` file in `~/.af/tools/` (or `$AF_TOOLS_DIR`).
3. Restart `af`. Your tool appears in `af tool list` with a `[local]` tag.

## TOML schema reference

```toml
[tool]
name = "custom.hash"                     # REQUIRED — dotted lowercase (e.g. "ns.tool")
version = 1                              # REQUIRED — integer >= 1
binary = "/usr/local/bin/hash-tool"      # REQUIRED — absolute path to executable
protocol = "simple"                      # REQUIRED — "simple" or "oop"
description = "Compute various hashes"   # REQUIRED — shown to LLM in system prompt

# Optional rich docs — merged into description for LLM context
usage = "Pass an artifact_id to hash. Returns md5, sha256, ssdeep."
good_for = ["File identification", "Malware tracking via fuzzy hashes"]

# Output redirect policy (default: "Allowed")
# "Allowed"   — worker may store oversized output as an artifact
# "Forbidden" — output must always be inline JSON
output_redirect = "Allowed"

# JSON Schema for tool input (TOML tables → serde_json::Value)
[tool.input_schema]
type = "object"
required = ["artifact_id"]

[tool.input_schema.properties.artifact_id]
"$ref" = "#/$defs/ArtifactId"            # auto-injected if missing from $defs

[tool.input_schema.properties.my_param]
type = "string"
description = "Some parameter"

# Policy overrides (all optional — defaults shown below)
[tool.policy]
sandbox = "NoNetReadOnly"                # see "Sandbox profiles" below
timeout_ms = 60000                       # max execution time
max_input_bytes = 262144                 # max input JSON size (256 KB)
max_output_bytes = 67108864              # max output size (64 MB)
max_produced_artifacts = 16              # max files the tool can produce
uds_bind_mounts = []                     # Unix domain sockets (read-only in sandbox)
writable_bind_mounts = []                # directories writable inside sandbox
extra_ro_bind_mounts = []                # extra read-only directories in sandbox

# Extra context passed to OOP executor (protocol = "oop" only)
[tool.context_extra]
my_key = "my_value"
```

### Name format

Tool names must be dotted lowercase: `[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+`

Examples: `custom.hash`, `my.deep_scan`, `re.yara_match`

Invalid: `Hash` (uppercase), `nodot` (no dot), `custom..tool` (empty segment)

### Description merging

At load time, `usage` and `good_for` are appended to `description`:

```
Compute various hashes

Usage: Pass an artifact_id to hash. Returns md5, sha256, ssdeep.

Good for: File identification, Malware tracking via fuzzy hashes
```

This merged string is what the LLM sees in its system prompt. Write it as if
you're explaining the tool to an AI assistant.

### Artifact references

If your tool operates on files, use `"$ref" = "#/$defs/ArtifactId"` in the
input schema. The loader auto-injects the `$defs.ArtifactId` definition if
your schema references it but doesn't define it.

For the **simple** protocol, artifact UUID strings in the input are automatically
replaced with file paths before being sent to your tool. Your tool receives an
actual file path it can open and read — no need to understand UUIDs.

For the **oop** protocol, artifacts are passed in `context.artifacts[]` with
full metadata (id, sha256, filename, storage_path, size_bytes, mime_type).

## Protocol modes

### Simple protocol (`protocol = "simple"`)

Easiest option. Your tool:
- Reads flat JSON from **stdin**
- Writes flat JSON to **stdout**
- Exits with code 0 on success, non-zero on failure

Artifact UUID fields are replaced with file paths before sending. Example:

**What your tool receives on stdin:**
```json
{"artifact_id": "/tmp/af/storage/ab/abc123def...", "min_length": 4}
```

**What your tool writes to stdout:**
```json
{"result": "ok", "hashes": {"md5": "...", "sha256": "..."}}
```

Limitations: no `produced_files`, no access to scratch_dir or project metadata.

### OOP protocol (`protocol = "oop"`)

Full-featured. Your tool:
- Reads an `OopEnvelope` JSON from **stdin**
- Writes an `OopResponse` JSON to **stdout**

**OopEnvelope (stdin):**
```json
{
  "tool_name": "custom.hash",
  "tool_version": 1,
  "input": {"artifact_id": "uuid-string", "min_length": 4},
  "context": {
    "project_id": "uuid",
    "tool_run_id": "uuid",
    "scratch_dir": "/tmp/af/scratch/run-uuid",
    "artifacts": [
      {
        "id": "uuid",
        "sha256": "abc123...",
        "filename": "malware.exe",
        "storage_path": "/tmp/af/storage/ab/abc123...",
        "size_bytes": 102400,
        "mime_type": "application/x-dosexec"
      }
    ],
    "extra": {"my_key": "my_value"}
  }
}
```

**OopResponse — success (stdout):**
```json
{
  "result": {
    "status": "ok",
    "output": {"hashes": {"md5": "...", "sha256": "..."}},
    "produced_files": [
      {
        "filename": "detailed_report.json",
        "path": "detailed_report.json",
        "mime_type": "application/json"
      }
    ]
  }
}
```

`produced_files[].path` is relative to `scratch_dir`. The worker ingests these
into content-addressed blob storage automatically.

**OopResponse — error (stdout):**
```json
{
  "result": {
    "status": "error",
    "code": "analysis_failed",
    "message": "Could not parse binary: unsupported format",
    "retryable": false
  }
}
```

## Sandbox profiles

All non-Trusted tools run inside a bubblewrap (`bwrap`) sandbox with:
- `tmpfs /` — empty root filesystem
- Selective `--ro-bind` for system dirs (`/usr`, `/bin`, `/lib`, `/etc`)
- `--bind scratch_dir` — writable working directory
- `--ro-bind` per artifact file — only the specific input files
- `--clearenv` — no inherited environment variables
- `--cap-drop ALL` — no Linux capabilities

| Profile              | Network  | Namespaces  | Use case                          |
|----------------------|----------|-------------|-----------------------------------|
| `Trusted`            | Full     | None        | In-process tools, dev/testing     |
| `NoNetReadOnly`      | None     | All unshared| Most tools (default)              |
| `NoNetReadOnlyTmpfs` | None     | All unshared| Same as above, stricter tmpfs     |
| `PrivateLoopback`    | Loopback | None        | Java/Ghidra (hangs with unshare)  |
| `NetEgressAllowlist` | Filtered | All unshared| Tools needing specific endpoints   |

### Bind mount options

- **`uds_bind_mounts`** — Unix domain socket paths, read-only. For gateway-pattern
  tools that talk to a local daemon (e.g. VT gateway).
- **`writable_bind_mounts`** — Directories mounted read-write. For caches that
  persist across runs (e.g. Ghidra analysis cache).
- **`extra_ro_bind_mounts`** — Additional read-only directories. For tool-specific
  configs, SSL certs, or installation directories.

## Example: minimal simple-protocol tool in bash

```bash
#!/bin/bash
# ~/.af/tools/my-hash-tool
# Reads JSON with artifact path, outputs hashes

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.artifact_id')

MD5=$(md5sum "$FILE_PATH" | cut -d' ' -f1)
SHA256=$(sha256sum "$FILE_PATH" | cut -d' ' -f1)
SIZE=$(stat -c%s "$FILE_PATH")

cat <<EOF
{"md5": "$MD5", "sha256": "$SHA256", "size_bytes": $SIZE}
EOF
```

```toml
# ~/.af/tools/custom.hash.toml
[tool]
name = "custom.hash"
version = 1
binary = "/home/user/.af/tools/my-hash-tool"
protocol = "simple"
description = "Compute MD5 and SHA256 hashes of a file"
usage = "Pass an artifact_id. Returns md5, sha256, and file size."
good_for = ["File identification", "Hash verification"]

[tool.input_schema]
type = "object"
required = ["artifact_id"]

[tool.input_schema.properties.artifact_id]
"$ref" = "#/$defs/ArtifactId"

[tool.policy]
sandbox = "NoNetReadOnly"
timeout_ms = 30000
```

## Example: simple-protocol tool in Python

```python
#!/usr/bin/env python3
"""Custom YARA scanner tool for Arbeiterfarm."""
import json, sys, yara

input_data = json.load(sys.stdin)
file_path = input_data["artifact_id"]  # replaced with actual path
rules_path = input_data.get("rules", "/etc/yara/default.yar")

rules = yara.compile(filepath=rules_path)
matches = rules.match(file_path)

output = {
    "matches": [{"rule": m.rule, "tags": m.tags} for m in matches],
    "total": len(matches),
}
json.dump(output, sys.stdout)
```

```toml
# ~/.af/tools/custom.yara.toml
[tool]
name = "custom.yara"
version = 1
binary = "/home/user/.af/tools/yara-scanner.py"
protocol = "simple"
description = "Scan a file with YARA rules"
usage = "Pass an artifact_id and optional rules path. Returns matching rule names and tags."
good_for = ["Malware classification", "Pattern matching", "IOC detection"]

[tool.input_schema]
type = "object"
required = ["artifact_id"]

[tool.input_schema.properties.artifact_id]
"$ref" = "#/$defs/ArtifactId"

[tool.input_schema.properties.rules]
type = "string"
default = "/etc/yara/default.yar"
description = "Path to YARA rules file"

[tool.policy]
sandbox = "NoNetReadOnly"
timeout_ms = 60000
extra_ro_bind_mounts = ["/etc/yara"]
```

## What can't be externalized

Tools that run **in-process** and access the database directly (via `ScopedPluginDb`)
cannot be defined as TOML tools:

- `re-ioc.list` — queries IOC table in Postgres
- `re-ioc.pivot` — queries IOC table in Postgres
- `echo.tool` — test/demo in-process executor

## Reference: existing tool files

| File | Tool | Protocol | Sandbox |
|------|------|----------|---------|
| `file.info.toml` | `file.info` | oop | NoNetReadOnly |
| `file.read_range.toml` | `file.read_range` | oop | NoNetReadOnly |
| `file.strings.toml` | `file.strings` | oop | NoNetReadOnly |
| `file.hexdump.toml` | `file.hexdump` | oop | NoNetReadOnly |
| `file.grep.toml` | `file.grep` | oop | NoNetReadOnly |
| `rizin.bininfo.toml` | `rizin.bininfo` | oop | NoNetReadOnly |
| `rizin.disasm.toml` | `rizin.disasm` | oop | NoNetReadOnly |
| `rizin.xrefs.toml` | `rizin.xrefs` | oop | NoNetReadOnly |
| `strings.extract.toml` | `strings.extract` | oop | NoNetReadOnly |
| `ghidra.analyze.toml` | `ghidra.analyze` | oop | PrivateLoopback |
| `ghidra.decompile.toml` | `ghidra.decompile` | oop | PrivateLoopback |
| `ghidra.rename.toml` | `ghidra.rename` | oop | NoNetReadOnly |
| `vt.file_report.toml` | `vt.file_report` | oop | NoNetReadOnly |

Binary paths in these files are placeholders. Replace with actual locations
of `af-builtin-executor` and `af-executor`.
