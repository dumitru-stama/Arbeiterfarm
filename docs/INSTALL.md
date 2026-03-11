# Installing Reverse-Arbeiterfarm

## Prerequisites

| Dependency | Required | Purpose |
|---|---|---|
| Rust (edition 2021) | Yes | Build toolchain |
| PostgreSQL 13+ | Yes | Persistent storage (threads, artifacts, agents, jobs) |
| bubblewrap (`bwrap`) | Yes | Sandbox for out-of-process tool execution |
| rizin | No | Binary analysis (imports, disassembly, xrefs) |
| Ghidra 11+ | No | Headless decompilation to C pseudocode |
| VirusTotal API key | No | Threat intelligence lookups |

At least one LLM backend must be configured for chat and agent operations. Tool listing and project management work without one.

---

## Step 1: Install System Dependencies

### Ubuntu / Linux Mint

```bash
# PostgreSQL
sudo apt-get install -y postgresql postgresql-client

# bubblewrap (sandbox)
sudo apt-get install -y bubblewrap

# rizin (optional — binary analysis)
sudo apt-get install -y rizin
```

Or use the Makefile helpers:

```bash
make setup-postgres
make setup-bwrap
```

### Ghidra (optional)

Download from https://ghidra-sre.org/ and extract:

```bash
sudo mkdir -p /opt/ghidra_11.0
sudo tar xf ghidra_11.0_PUBLIC.tar.gz -C /opt/ghidra_11.0 --strip-components=1
export AF_GHIDRA_HOME=/opt/ghidra_11.0
```

Ghidra requires a JDK 17+ runtime.

---

## Step 2: Set Up PostgreSQL

Create the `af` user and database:

```bash
make setup-db
```

Or manually:

```sql
sudo -u postgres psql -c "CREATE USER af WITH PASSWORD 'af';"
sudo -u postgres psql -c "CREATE DATABASE af OWNER af;"
sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE af TO af;"
sudo -u postgres psql -c "GRANT ALL ON SCHEMA public TO af;"   -- PostgreSQL 15+
```

Verify the connection:

```bash
psql postgres://af:af@localhost/af -c "SELECT 1;"
```

Database migrations run automatically on first use. No manual migration step is needed.

---

## Step 3: Build

```bash
cargo build --release
```

This produces three binaries in `target/release/`:

| Binary | Purpose |
|---|---|
| `af` | Generic CLI and API server (TOML plugins only) |
| `af-re` | RE distribution binary (compiled RE plugin + TOML plugins) |
| `af-builtin-executor` | Sandboxed executor for file analysis tools |
| `af-re-executor` | Sandboxed executor for RE tools (rizin, Ghidra, VT, IOC) |

Optionally install them into your PATH:

```bash
sudo cp target/release/af target/release/af-re /usr/local/bin/
sudo cp target/release/af-builtin-executor target/release/af-re-executor /usr/local/bin/
```

The main binary discovers executor binaries in this order:
1. `AF_EXECUTOR_PATH` environment variable
2. Same directory as the `af-re` binary
3. `$PATH` lookup

---

## Step 4: Configure an LLM Backend

Configure at least one backend. Multiple can be active simultaneously.

### Anthropic (Claude)

```bash
export AF_ANTHROPIC_API_KEY="sk-ant-..."
export AF_ANTHROPIC_MODEL="claude-sonnet-4-20250514"   # default
```

### Local LLM (Ollama, vLLM, llama.cpp, LM Studio, etc.)

```bash
export AF_LOCAL_ENDPOINT="http://localhost:11434"
export AF_LOCAL_MODEL="llama3"
# Optional: AF_LOCAL_MODELS="model-a,model-b" for extra models
# Optional: AF_LOCAL_API_KEY="..." if the server requires auth
```

### OpenAI

```bash
export AF_OPENAI_API_KEY="sk-..."
export AF_OPENAI_MODEL="gpt-4o"
# Optional: AF_OPENAI_MODELS="gpt-4.1,o4-mini" for extra models
```

### Vertex AI (Google Cloud)

```bash
export AF_VERTEX_ENDPOINT="https://us-central1-aiplatform.googleapis.com/v1/projects/PROJECT/locations/us-central1/publishers/google/models/gemini-pro"
export AF_VERTEX_ACCESS_TOKEN="$(gcloud auth print-access-token)"
```

### Multiple Models Per Provider

Register additional models from the same provider. Each gets its own `provider:model` backend name for routing.

```bash
# Extra OpenAI models (comma-separated)
export AF_OPENAI_MODELS="gpt-4o-mini,gpt-3.5-turbo"

# Extra Anthropic models
export AF_ANTHROPIC_MODELS="claude-haiku-4-5-20251001"
```

The primary model (from `AF_OPENAI_MODEL` / `AF_ANTHROPIC_MODEL`) is registered as the default. Extra models are available via explicit routing:

```bash
# Route an agent to a specific model
af-re agent create --name "fast-triage" \
  --tools "file.*,rizin.bininfo" \
  --route "backend:openai:gpt-4o-mini" \
  --prompt "Quick triage only."

# Old routes still work (alias resolves to primary model)
--route "backend:openai"    # resolves to openai:gpt-4o
--route "backend:anthropic" # resolves to anthropic:claude-sonnet-4-20250514
```

---

## Step 5: Configure Optional Tools

### VirusTotal

```bash
export AF_VT_API_KEY="your-vt-api-key"
export AF_VT_RATE_LIMIT=4        # requests per minute (default)
export AF_VT_CACHE_TTL=86400     # cache TTL in seconds (default 24h)
```

### Rizin

Detected automatically at `/usr/bin/rizin`. Override with:

```bash
export AF_RIZIN_PATH="/path/to/rizin"
```

### Ghidra

```bash
export AF_GHIDRA_HOME="/opt/ghidra_11.0"
export AF_GHIDRA_CACHE="/tmp/af/ghidra_cache"   # default
```

### Local Model Cards (Optional)

When using local LLM backends (Ollama, vLLM), you can define model specifications so the system knows the context window, output limits, and capabilities of your models:

```bash
mkdir -p ~/.af/models
cat > ~/.af/models/gpt-oss-20b.toml <<EOF
[model]
name = "gpt-oss-20b"
context_window = 131072
max_output_tokens = 16384
supports_vision = false
supports_tool_calls = true
EOF
```

Model cards override the built-in catalog (~20 models). See `examples/local-models/README.md` for full format and 40+ example cards.

### Sandbox Gateway (Optional)

The sandbox gateway enables dynamic analysis tools (`sandbox.trace`, `sandbox.hook`, `sandbox.screenshot`) using a QEMU/KVM virtual machine with Frida instrumentation:

```bash
# Enable sandbox tools
export AF_SANDBOX_SOCKET="/run/af/sandbox_gateway.sock"
export AF_SANDBOX_QMP="/run/af/qemu-monitor.sock"
export AF_SANDBOX_AGENT="192.168.122.10:9111"
export AF_SANDBOX_SNAPSHOT="clean"
```

See `docs/sandbox-dynamic-analysis.md` for complete setup (VM creation, guest agent, Frida hooks).

### Web Gateway (Optional)

The web gateway enables `web.fetch` and `web.search` tools, allowing agents to retrieve web content and search the internet. It runs as an embedded daemon accessible via Unix domain socket.

```bash
# Enable web tools by setting the gateway socket path
export AF_WEB_GATEWAY_SOCKET="/run/af/web_gateway.sock"

# Optional: GeoIP country blocking (requires MaxMind GeoLite2-Country database)
export AF_GEOIP_MMDB="/path/to/GeoLite2-Country.mmdb"

# Optional: tune rate limits and caching
export AF_WEB_GATEWAY_RATE_LIMIT=4           # global requests per minute (default: 4)
export AF_WEB_GATEWAY_PER_USER_RPM=4         # per-user rate limit (default: 4)
export AF_WEB_GATEWAY_CACHE_TTL=86400        # cache TTL in seconds (default: 24h)
export AF_WEB_GATEWAY_MAX_RESPONSE_BYTES=524288  # max response size (default: 512KB)
export AF_WEB_GATEWAY_FETCH_TIMEOUT=30       # fetch timeout in seconds (default: 30)
export AF_WEB_GATEWAY_MAX_REDIRECTS=5        # max redirect hops (default: 5)
```

Web tools are **restricted by default** — users need an admin grant before they can use them:

```bash
# Grant a user access to web tools
af-re grant tool <user-id> "web.*"
```

---

## Step 6: Verify Installation

```bash
# List registered tools (works without DB or LLM)
af-re tool list

# Check DB connectivity
af-re project list

# Run tests
cargo test --workspace
```

Expected output from `tool list`: 15+ tools across file analysis, rizin, Ghidra, VirusTotal, IOC, and web categories (some marked `[unavailable]` if their dependencies are missing).

---

## Step 7: Web UI (Optional)

The web UI is included in the `ui/` directory and requires no build step. You need a running database (Step 2) and at least one LLM backend (Step 4) before starting the server.

Start the API server **from the repository root** (so the `ui/` directory is found):

```bash
af-re serve --bind 127.0.0.1:8080
# Open http://localhost:8080
```

Create an admin user and API key to log in:

```bash
af-re user create --name admin --roles admin
# Note the user ID printed

af-re user api-key create --user <user-id> --name "browser"
# Copy the raw key (shown once) and paste it into the login screen
```

See [USAGE.md](USAGE.md#web-ui) for full UI documentation.

---

## Environment Variable Reference

| Variable | Default | Purpose |
|---|---|---|
| `AF_DATABASE_URL` | `postgres://af:af@localhost/af` | PostgreSQL connection |
| `AF_STORAGE_ROOT` | `/tmp/af/storage` | Content-addressed blob storage |
| `AF_SCRATCH_ROOT` | `/tmp/af/scratch` | Per-tool-run temporary files |
| `AF_DEFAULT_ROUTE` | (first registered) | Default backend for auto route (e.g. `anthropic`, `openai:gpt-4.1`) |
| `AF_LOCAL_ENDPOINT` | (none) | Local LLM server (Ollama, vLLM, llama.cpp) |
| `AF_LOCAL_MODEL` | `gpt-oss` | Local model name |
| `AF_LOCAL_API_KEY` | (none) | Local server API key (if needed) |
| `AF_LOCAL_MODELS` | (none) | Extra local models (comma-separated) |
| `AF_OPENAI_ENDPOINT` | (none) | Custom OpenAI-compatible cloud endpoint |
| `AF_OPENAI_API_KEY` | (none) | OpenAI API key |
| `AF_OPENAI_MODEL` | `gpt-4o` | OpenAI model name |
| `AF_OPENAI_MODELS` | (none) | Extra OpenAI models (comma-separated) |
| `AF_ANTHROPIC_API_KEY` | (none) | Anthropic API key |
| `AF_ANTHROPIC_MODEL` | `claude-sonnet-4-20250514` | Anthropic model |
| `AF_ANTHROPIC_MODELS` | (none) | Extra Anthropic models (comma-separated) |
| `AF_VERTEX_ENDPOINT` | (none) | Vertex AI endpoint |
| `AF_VERTEX_ACCESS_TOKEN` | (none) | Vertex AI OAuth2 token |
| `AF_RIZIN_PATH` | `/usr/bin/rizin` | Path to rizin binary |
| `AF_VT_API_KEY` | (none) | VirusTotal API key |
| `AF_VT_SOCKET` | `/run/af/vt_gateway.sock` | VT gateway UDS path |
| `AF_VT_RATE_LIMIT` | `4` | VT requests per minute |
| `AF_VT_CACHE_TTL` | `86400` | VT cache TTL (seconds) |
| `AF_GHIDRA_HOME` | (none) | Ghidra installation directory |
| `AF_GHIDRA_CACHE` | `/tmp/af/ghidra_cache` | Ghidra project cache |
| `AF_EXECUTOR_PATH` | (none) | Explicit path to executor binary |
| `AF_ALLOW_UNSANDBOXED` | (none) | Allow tools without bwrap (dev only) |
| `AF_WEB_GATEWAY_SOCKET` | (none) | UDS path for web gateway (enables web.fetch/web.search) |
| `AF_GEOIP_MMDB` | (none) | MaxMind GeoLite2-Country database for GeoIP blocking |
| `AF_WEB_GATEWAY_RATE_LIMIT` | `4` | Web gateway global rate limit (req/min) |
| `AF_WEB_GATEWAY_PER_USER_RPM` | `4` | Web gateway per-user rate limit (req/min) |
| `AF_WEB_GATEWAY_CACHE_TTL` | `86400` | Web response cache TTL (seconds) |
| `AF_WEB_GATEWAY_MAX_RESPONSE_BYTES` | `524288` | Max web response body (bytes) |
| `AF_WEB_GATEWAY_FETCH_TIMEOUT` | `30` | Web fetch timeout (seconds) |
| `AF_WEB_GATEWAY_MAX_REDIRECTS` | `5` | Max HTTP redirect hops |
| `AF_DB_POOL_SIZE` | `10` | DB connection pool size (use >= 20 for thinking) |
| `AF_UPLOAD_MAX_BYTES` | `104857600` | Max upload size (API server) |
| `AF_API_RATE_LIMIT` | `60` | API rate limit (req/min) |
| `AF_MODELS_DIR` | `~/.af/models` | TOML model card directory |
| `AF_SANDBOX_SOCKET` | (none) | UDS path for sandbox gateway |
| `AF_SANDBOX_QMP` | (none) | QMP Unix socket for QEMU VM |
| `AF_SANDBOX_AGENT` | `192.168.122.10:9111` | Guest agent TCP address |
| `AF_SANDBOX_SNAPSHOT` | `clean` | Snapshot name for savevm/loadvm |

---

## Troubleshooting

**"bwrap not found" / tools fail to execute**

Install bubblewrap (`sudo apt-get install bubblewrap`) or set `AF_ALLOW_UNSANDBOXED=1` for development. The sandbox is fail-closed by default: tools that require isolation will not run without bwrap.

**"no LLM backend configured"**

Set at least one of `AF_ANTHROPIC_API_KEY`, `AF_OPENAI_ENDPOINT`, or `AF_VERTEX_ENDPOINT`. Tool listing and project management work without an LLM, but chat and agent operations require one.

**Database connection refused**

Check that PostgreSQL is running (`sudo systemctl status postgresql`) and that the connection string in `AF_DATABASE_URL` is correct. Run `make setup-db` to create the user and database if they don't exist.

**rizin/Ghidra tools show as unavailable**

These are auto-detected on startup. Verify the binaries exist at their configured paths (`which rizin`, `ls $AF_GHIDRA_HOME/support/analyzeHeadless`).

**Database reset**

```bash
make clean-db        # drops and recreates (prompts for confirmation)
make clean-storage   # deletes blob and scratch directories
```
