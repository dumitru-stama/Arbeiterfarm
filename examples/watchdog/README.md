# Watchdog: Auto-Triggered Analysis on Upload

Arbeiterfarm's hooks system supports automatic analysis when artifacts are uploaded.
No separate watchdog daemon is needed — the existing hooks infrastructure handles it.

## Setup

### 1. Create a workflow (optional)

Register the `auto-triage` workflow from the example TOML:

```bash
# Copy to workflows dir (loaded at startup)
cp auto-triage-hook.toml ~/.af/workflows/

# Or validate first:
af workflow validate auto-triage-hook.toml
```

### 2. Create a hook

```bash
# Hook that fires a workflow on upload:
af hook create \
  --project <PROJECT_ID> \
  --name "auto-triage-on-upload" \
  --event artifact_uploaded \
  --workflow auto-triage \
  --prompt "Analyze artifact {{artifact_id}} ({{filename}}, sha256:{{sha256}})"

# Or hook that fires a single agent:
af hook create \
  --project <PROJECT_ID> \
  --name "quick-surface-on-upload" \
  --event artifact_uploaded \
  --agent surface \
  --prompt "Perform quick triage on artifact {{artifact_id}} ({{filename}})"
```

### 3. Upload an artifact

```bash
af artifact add sample.exe --project <PROJECT_ID>
# The hook fires automatically, creating a new conversation
# with the workflow or agent running against the uploaded artifact.
```

## Tick Hooks (Periodic Tasks)

For periodic analysis (e.g., re-check VT results daily):

```bash
af hook create \
  --project <PROJECT_ID> \
  --name "daily-vt-recheck" \
  --event tick \
  --agent intel \
  --prompt "Re-check VirusTotal for any artifacts that haven't been scanned in 24 hours." \
  --interval 1440

# Fire all due tick hooks (run from cron):
af tick
```

## API Equivalent

```bash
# Create a hook via API:
curl -X POST https://af.example.com/api/v1/projects/<ID>/hooks \
  -H "Authorization: Bearer <API_KEY>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "auto-triage-on-upload",
    "event_type": "artifact_uploaded",
    "workflow_name": "auto-triage",
    "prompt_template": "Analyze artifact {{artifact_id}} ({{filename}}, sha256:{{sha256}})"
  }'
```

## Template Variables

Hook prompt templates support these placeholders for `artifact_uploaded` events:

| Variable | Description |
|---|---|
| `{{artifact_id}}` | UUID of the uploaded artifact |
| `{{filename}}` | Original filename |
| `{{sha256}}` | SHA256 hash of the file |
| `{{project_id}}` | Project UUID |
