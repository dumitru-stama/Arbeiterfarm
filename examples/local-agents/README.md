# Local Agent TOML Reference

Drop `.toml` files into `~/.af/agents/` (or set `AF_AGENTS_DIR`) to register agents without recompiling. Agents are upserted to DB at startup.

## Format

```toml
# Required: unique agent name
name = "my-agent"

# Optional: LLM route (default: "auto")
# Values: "auto", "local", or a specific backend like "openai:gpt-4o", "anthropic:claude-sonnet-4-20250514"
route = "auto"

# Optional: tool allowlist (default: empty — no tools)
# Supports glob patterns: "file.*" matches file.info, file.read_range, etc.
tools = ["file.*", "rizin.*", "ghidra.decompile"]

# Optional: arbitrary key-value metadata (stored as JSON)
[metadata]
version = "1.0"
author = "me"

# Required: system prompt
[prompt]
text = """You are a specialized analysis agent.
Use the available tools to investigate binaries."""
```

## Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `name` | yes | — | Agent name. Used in `--agent`, conversation settings, and workflow steps. |
| `route` | no | `"auto"` | LLM backend route. `"auto"` picks the first available backend. `"local"` forces local/Ollama. Specific backends: `"openai:gpt-4o"`, `"anthropic:claude-sonnet-4-20250514"`, etc. |
| `tools` | no | `[]` | Tool name patterns the agent can use. Glob-style: `"file.*"` matches all `file.` tools. Exact names also work: `"ghidra.analyze"`. |
| `metadata` | no | `{}` | Arbitrary TOML table, stored as JSON. No effect on behavior — for your own bookkeeping. |
| `prompt.text` | yes | — | System prompt sent to the LLM. This defines the agent's personality, instructions, and constraints. |

## Route values

| Route | Description |
|---|---|
| `auto` | Use the first available backend (tries OpenAI, then Anthropic, then Vertex) |
| `local` | Force local backend (Ollama or other OpenAI-compatible local server) |
| `openai` | Default OpenAI model (`AF_OPENAI_MODEL`) |
| `openai:gpt-4o-mini` | Specific OpenAI model |
| `anthropic` | Default Anthropic model (`AF_ANTHROPIC_MODEL`) |
| `anthropic:claude-haiku-4-5-20251001` | Specific Anthropic model |
| `vertex` | Vertex AI backend |

## Tool patterns

Patterns match against registered tool names:

| Pattern | Matches |
|---|---|
| `file.*` | `file.info`, `file.read_range`, `file.strings`, `file.hexdump`, `file.grep` |
| `rizin.*` | `rizin.bininfo`, `rizin.disasm`, `rizin.xrefs` |
| `ghidra.*` | `ghidra.analyze`, `ghidra.decompile`, `ghidra.rename` |
| `vt.*` | `vt.file_report` |
| `web.*` | `web.fetch`, `web.search` (requires web gateway + user grant) |
| `re-ioc.*` | `re-ioc.list`, `re-ioc.pivot` |
| `transform.*` | `transform.decode`, `transform.unpack`, `transform.jq`, `transform.csv`, `transform.convert`, `transform.regex` |
| `sandbox.*` | `sandbox.trace`, `sandbox.hook`, `sandbox.screenshot` (requires sandbox gateway) |
| `strings.extract` | `strings.extract` (exact match) |
| `echo.tool` | `echo.tool` (built-in echo, useful for testing) |

## Builtin agents (exported here)

| File | Name | Route | Description |
|---|---|---|---|
| `default.toml` | `default` | auto | General-purpose RE assistant with all tools |
| `surface.toml` | `surface` | auto | Quick triage: file metadata, imports, strings |
| `decompiler.toml` | `decompiler` | auto | Deep code analysis with Ghidra + rizin |
| `asm.toml` | `asm` | local | Assembly-only analysis (rizin disasm + xrefs) |
| `intel.toml` | `intel` | auto | Threat intelligence: VT lookups, IOC correlation |
| `reporter.toml` | `reporter` | auto | Report writer: synthesizes conversation history into markdown |
| `transformer.toml` | `transformer` | auto | Data transform specialist: decode, unpack, jq, csv, convert, regex |
| `yara-writer.toml` | `yara-writer` | auto | YARA rule generation from analysis findings |

## Notes

- Agents loaded from TOML are registered with `is_builtin = false`
- If a TOML agent has the same name as a builtin, the TOML version wins (upsert)
- Restart `af` to reload after adding/modifying TOML files
- The agent name is what you type in the UI's agent dropdown or pass to `--agent`
