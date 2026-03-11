# Local Workflow TOML Reference

Drop `.toml` files into `~/.af/workflows/` (or set `AF_WORKFLOWS_DIR`) to register workflows without recompiling. Workflows and any inline agents are upserted to DB at startup.

## Format

```toml
[workflow]
# Required: unique workflow name (lowercase, hyphens/underscores, no double hyphens)
name = "my-analysis"

# Optional: human-readable description (shown in UI and CLI)
description = "Custom RE pipeline with reporting"

# Required: at least one step
# Steps with the same group number run in parallel; groups execute sequentially (ascending).

[[workflow.steps]]
agent = "surface"        # Required: agent name (must exist as builtin or local agent)
group = 1                # Required: group number (u32). Same group = parallel execution.
prompt = "Perform quick surface triage."  # Required: task prompt injected into agent's system prompt
can_repivot = true       # Optional (default: true). If true, agent re-runs when a replacement artifact appears.

[[workflow.steps]]
agent = "intel"
group = 1                # Same group as surface — they run in parallel
prompt = "Look up threat intelligence."

[[workflow.steps]]
agent = "decompiler"
group = 2                # Runs after group 1 finishes
prompt = "Analyze key functions found in surface triage."

[[workflow.steps]]
agent = "reporter"
group = 3                # Runs after group 2 finishes
prompt = "Write report synthesizing all findings."
can_repivot = false      # Reporter should not re-run on repivot

# Optional: define agents inline (registered alongside the workflow)
[[workflow.agents]]
name = "my-custom-agent"
route = "auto"
tools = ["ghidra.analyze", "ghidra.decompile"]

[workflow.agents.prompt]
text = "You are a specialized function analyst."
```

## Workflow fields

| Field | Required | Default | Description |
|---|---|---|---|
| `workflow.name` | yes | — | Workflow name. Pattern: `[a-z][a-z0-9_-]*`, no double hyphens, no trailing hyphen/underscore. |
| `workflow.description` | no | `null` | Human-readable description shown in UI dropdown and `af workflow list`. |
| `workflow.steps` | yes | — | At least one step. |
| `workflow.agents` | no | `[]` | Inline agent definitions, registered alongside the workflow. Same format as agent TOML files (see `local-agents/README.md`). |

## Step fields

| Field | Required | Default | Description |
|---|---|---|---|
| `agent` | yes | — | Name of the agent to run. Must be registered (builtin, local TOML, or inline). |
| `group` | yes | — | Group number (u32). Steps in the same group run in parallel. Groups execute in ascending order. |
| `prompt` | yes | — | Task-specific prompt. Appended to the agent's system prompt as "Your specific task: {prompt}". |
| `can_repivot` | no | `true` | If `true` and a tool produces a replacement artifact during the workflow, this agent is re-queued to analyze the new artifact. Set to `false` for agents that should not re-run (e.g. reporter). |

## Execution model

```
Group 1:  [surface] ──┬── parallel
          [intel]   ──┘
                      │
                      ▼ (wait for all group 1 agents)
Group 2:  [decompiler] ── sequential
                      │
                      ▼ (wait)
Group 3:  [reporter]   ── sequential
```

1. All steps in group 1 run concurrently (each agent gets its own tokio task)
2. When all group 1 agents finish, agent outputs are scanned for signals
3. Group 2 starts, and so on
4. After each group, dynamic behaviors may trigger:
   - **Signals**: agents can emit `signal:request:<agent>`, `signal:skip:<agent>`, `signal:priority:<agent>` to modify remaining groups
   - **Repivot**: if a tool produces a replacement artifact, eligible completed agents are re-queued
   - **Fan-out**: if a tool extracts child files (zip, tar), each gets its own conversation running the full workflow

## Dynamic signals

Agents can emit signal markers in their output text to influence the workflow:

| Signal | Effect |
|---|---|
| `signal:request:<agent>:<reason>` | Add agent to the next group (if not already planned) |
| `signal:skip:<agent>:<reason>` | Remove agent from all future groups |
| `signal:priority:<agent>:<reason>` | Move agent from a later group to the next group |

Agents are told about signals in their system prompt automatically.

## Builtin workflow (exported here)

| File | Name | Description |
|---|---|---|
| `full-analysis.toml` | `full-analysis` | surface + intel (parallel) -> decompiler -> reporter |

## Name validation rules

- Must start with `a-z`
- Only `a-z`, `0-9`, `-`, `_` allowed
- No double hyphens (`--`)
- Cannot end with `-` or `_`
- Examples: `full-analysis`, `quick-triage`, `my_custom_pipeline`

## Notes

- Workflows loaded from TOML are registered with `is_builtin = false`
- Inline agents are also registered with `is_builtin = false`
- Restart `af` to reload after adding/modifying TOML files
- Use `af workflow validate <file>` to check a TOML file without registering
- All agents in a workflow write to the same conversation (later agents see earlier agents' output)
