# Arbeiterfarm — Build, Test, and Setup
#
# Usage:
#   make help            Show all targets
#   make setup-postgres  Install PostgreSQL on Linux Mint / Ubuntu
#   make setup-db        Create af user and database
#   make build           Build release binaries
#   make test            Run unit tests
#   make test-e2e        Full end-to-end test (Slices 1-3, needs DB)
#   make test-tools      Test all 5 file tools (needs DB)
#   make test-chat       Launch interactive chat (needs DB + LLM)
#   make clean-db        Drop and recreate af database
#
# Binaries produced:
#   af              RE distribution binary (compiled plugin + optional TOML plugins)
#   af-executor     OOP executor for RE tools (Rizin, Ghidra) in bwrap sandbox
#   af-builtin-executor OOP executor for builtin file tools in bwrap sandbox
#   af                 Generic binary (TOML plugins only, no compiled plugins)
#
# Environment:
#   AF_DATABASE_URL    Postgres connection (default: postgres://af:af@localhost/af)
#   AF_ANTHROPIC_API_KEY / AF_OPENAI_ENDPOINT / AF_VERTEX_ENDPOINT  (for chat)
#   AF_MAX_STREAM_DURATION_SECS  Global agent/stream timeout cap (default: 1800)
#
# Local config:
#   Copy Makefile.local.example → Makefile.local and fill in your API keys.
#   Makefile.local is gitignored and will be auto-generated with instructions
#   if missing.

SHELL := /bin/bash
.ONESHELL:
.PHONY: help build test check clean setup-postgres setup-db setup-bwrap \
        test-e2e test-tools test-slice1 test-slice2 test-slice3 test-chat \
        test-redaction clean-db db-status clean-storage clean-all serve serve-local \
        serve-generic worker tick submodules

# ─── Local config (API keys, tool paths) ─────────────────────────────────────
# Auto-generate Makefile.local with instructions if it doesn't exist.
ifeq ($(wildcard Makefile.local),)
$(info )
$(info  Makefile.local not found -- generating template.)
$(info  Edit Makefile.local with your API keys and tool paths.)
$(info )
$(shell printf '%s\n' \
	'# Makefile.local — Personal configuration (NOT tracked by git)' \
	'#' \
	'# This file is included by the main Makefile. Set your API keys,' \
	'# tool paths, and local preferences here.' \
	'#' \
	'# To regenerate this file with defaults, delete it and run any make target.' \
	'' \
	'# ─── LLM API Keys ────────────────────────────────────────────────────────' \
	'# Uncomment and fill in the backends you use.' \
	'' \
	'# AF_OPENAI_API_KEY    ?= sk-proj-CHANGEME' \
	'# AF_OPENAI_MODEL      ?= gpt-4o' \
	'# AF_ANTHROPIC_API_KEY ?= sk-ant-CHANGEME' \
	'# AF_ANTHROPIC_MODEL   ?= claude-sonnet-4-20250514' \
	'' \
	'# ─── Local LLM (Ollama / vLLM / llama.cpp) ──────────────────────────────' \
	'# AF_LOCAL_ENDPOINT  ?= http://localhost:11434' \
	'# AF_LOCAL_MODEL     ?= gpt-oss' \
	'' \
	'# ─── Tool Paths ─────────────────────────────────────────────────────────' \
	'# GHIDRA_HOME          ?= /opt/ghidra_11.0' \
	'# AF_RIZIN_PATH        ?= /usr/bin/rizin' \
	'' \
	'# ─── Ollama model for serve-local ───────────────────────────────────────' \
	'# OLLAMA_MODEL         ?= gpt-oss' \
	> Makefile.local)
endif
-include Makefile.local

# ─── Defaults (used if Makefile.local doesn't set them) ──────────────────────
GHIDRA_HOME    ?=
AF_RIZIN_PATH  ?= /usr/bin/rizin
OLLAMA_MODEL   ?= gpt-oss

# Binaries
BIN            := target/release/af
BIN_GENERIC    := target/release/af
EXECUTOR       := target/release/af-builtin-executor
RE_EXECUTOR    := target/release/af-executor

# Database
DB_URL    ?= postgres://af:af@localhost/af
export AF_DATABASE_URL := $(DB_URL)

# ─── Submodules ───────────────────────────────────────────────────────────────

submodules: ## Initialize and update git submodules (cwc, oaie)
	@if [ ! -f cwc/cwc-core/Cargo.toml ] || [ ! -f oaie/Cargo.toml ]; then \
		echo -e "\033[0;33mInitializing git submodules...\033[0m"; \
		git submodule update --init --recursive; \
	fi

# ─── Help ──────────────────────────────────────────────────────────────────────

help: ## Show this help
	@echo ""
	@echo "  Arbeiterfarm Makefile targets:"
	@echo ""
	@grep -hE '^[a-zA-Z0-9_-]+:.*##' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*##"}; {printf "  \033[0;36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""

# ─── Setup ─────────────────────────────────────────────────────────────────────

setup-postgres: ## Install PostgreSQL on Linux Mint / Ubuntu
	@set -e
	echo -e "\033[0;33mInstalling PostgreSQL...\033[0m"
	sudo apt-get update
	sudo apt-get install -y postgresql postgresql-client postgresql-contrib postgresql-$$(pg_config --version | grep -oP '\d+' | head -1)-pgvector
	sudo systemctl start postgresql
	sudo systemctl enable postgresql
	echo ""
	echo -e "\033[0;32mPostgreSQL installed and running.\033[0m"
	echo "Next: run 'make setup-db' to create the af database."

setup-db: ## Create af user and database (idempotent)
	@set -e
	echo -e "\033[0;33mSetting up af database...\033[0m"
	sudo -u postgres psql -tc "SELECT 1 FROM pg_roles WHERE rolname='af'" | grep -q 1 || \
		sudo -u postgres psql -c "CREATE USER af WITH PASSWORD 'af';"
	sudo -u postgres psql -tc "SELECT 1 FROM pg_database WHERE datname='af'" | grep -q 1 || \
		sudo -u postgres psql -c "CREATE DATABASE af OWNER af;"
	sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE af TO af;"
	# PG 15+ requires explicit schema grant
	sudo -u postgres psql -d af -c "GRANT ALL ON SCHEMA public TO af;" 2>/dev/null || true
	# pgvector for embeddings (CREATE EXTENSION requires superuser)
	sudo -u postgres psql -d af -c "CREATE EXTENSION IF NOT EXISTS vector;" 2>/dev/null || true
	echo ""
	echo -e "\033[0;32mDatabase ready.\033[0m Verify with:"
	echo "  psql $(DB_URL) -c 'SELECT 1'"

setup-bwrap: ## Install bubblewrap sandbox
	@set -e
	sudo apt-get install -y bubblewrap
	echo -e "\033[0;32mbubblewrap installed:\033[0m $$(bwrap --version)"

# ─── Build ─────────────────────────────────────────────────────────────────────

build: submodules ## Build release binaries (af, af, executors)
	@set -e
	cargo build --release
	cp -r arbeiterfarm/ghidra-scripts target/release/ 2>/dev/null || true
	echo ""
	echo -e "\033[0;32mBuilt:\033[0m"
	ls -lh $(BIN) $(BIN_GENERIC) $(EXECUTOR) $(RE_EXECUTOR) 2>/dev/null || echo "  (some binaries missing)"

check: submodules ## Fast type check (no codegen)
	@cargo check --workspace

# ─── Tests ─────────────────────────────────────────────────────────────────────

test: submodules ## Run all unit tests
	@set -e
	cargo test --workspace
	echo ""
	echo -e "\033[0;32mAll unit tests passed.\033[0m"

test-redaction: submodules ## Run redaction tests only
	@cargo test --package af-llm

# ─── End-to-end (needs DB) ─────────────────────────────────────────────────────

test-e2e: build ## Full end-to-end test: Slices 1 + 2 + 3 (needs DB)
	@set -e
	echo ""
	echo -e "\033[0;36m═══ Slice 1: Core plumbing ═══\033[0m"
	$(MAKE) --no-print-directory test-slice1
	echo ""
	echo -e "\033[0;36m═══ Slice 2: File tools + OOP ═══\033[0m"
	$(MAKE) --no-print-directory test-slice2
	echo ""
	echo -e "\033[0;36m═══ Slice 3: CLI + streaming + validation ═══\033[0m"
	$(MAKE) --no-print-directory test-slice3
	echo ""
	echo -e "\033[0;32m═══ All end-to-end tests passed! ═══\033[0m"

test-slice1: build ## Slice 1: project, artifact, echo.tool
	@set -e
	echo -e "\033[0;33m[1/4] Tool list (no DB)...\033[0m"
	$(BIN) tool list | grep -q "echo.tool"
	echo "  OK: echo.tool found"
	echo -e "\033[0;33m[2/4] Create project...\033[0m"
	PROJECT=$$($(BIN) project create _test_s1_$$$$ 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "  Project: $$PROJECT"
	echo -e "\033[0;33m[3/4] Add artifact...\033[0m"
	echo "hello from slice 1 make test" > /tmp/_af_s1_test.txt
	ARTIFACT=$$($(BIN) artifact add /tmp/_af_s1_test.txt --project $$PROJECT 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "  Artifact: $$ARTIFACT"
	echo -e "\033[0;33m[4/4] Run echo.tool...\033[0m"
	$(BIN) tool run echo.tool --project $$PROJECT --input "{\"artifact_id\":\"$$ARTIFACT\"}" | grep -q "completed"
	echo -e "  \033[0;32mOK: echo.tool completed\033[0m"
	rm -f /tmp/_af_s1_test.txt

test-slice2: build ## Slice 2: all 5 file tools via OOP executor
	@set -e
	echo -e "\033[0;33m[1/7] Executor handshake...\033[0m"
	$(EXECUTOR) --handshake | grep -q "protocol_version"
	echo "  OK: handshake valid"
	echo -e "\033[0;33m[2/7] 6 tools registered...\033[0m"
	TOOL_COUNT=$$($(BIN) tool list | wc -l)
	if [ "$$TOOL_COUNT" -ge 6 ]; then
		echo "  OK: $$TOOL_COUNT tools"
	else
		echo -e "  \033[0;31mFAIL: only $$TOOL_COUNT tools\033[0m" && exit 1
	fi
	echo -e "\033[0;33m[3/7] Create project + artifact...\033[0m"
	PROJECT=$$($(BIN) project create _test_s2_$$$$ 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "Slice 2 file tool testing data with some binary: ABCDEF" > /tmp/_af_s2_test.txt
	ARTIFACT=$$($(BIN) artifact add /tmp/_af_s2_test.txt --project $$PROJECT 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "  Project: $$PROJECT  Artifact: $$ARTIFACT"
	echo -e "\033[0;33m[4/7] file.info...\033[0m"
	$(BIN) tool run file.info --project $$PROJECT --input "{\"artifact_id\":\"$$ARTIFACT\"}" | grep -q "completed"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[5/7] file.read_range...\033[0m"
	$(BIN) tool run file.read_range --project $$PROJECT --input "{\"artifact_id\":\"$$ARTIFACT\",\"line_start\":1,\"line_count\":5}" | grep -q "completed"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[6/7] file.hexdump...\033[0m"
	$(BIN) tool run file.hexdump --project $$PROJECT --input "{\"artifact_id\":\"$$ARTIFACT\",\"offset\":0,\"length\":64}" | grep -q "completed"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[7/7] file.grep...\033[0m"
	$(BIN) tool run file.grep --project $$PROJECT --input "{\"artifact_id\":\"$$ARTIFACT\",\"pattern\":\"Slice\",\"context_lines\":1}" | grep -q "completed"
	echo -e "  \033[0;32mOK\033[0m"
	rm -f /tmp/_af_s2_test.txt

test-slice3: build ## Slice 3: CLI commands, redaction, thread ops
	@set -e
	echo -e "\033[0;33m[1/8] Redaction unit tests...\033[0m"
	cargo test --package af-llm -- --quiet 2>&1 | tail -1
	echo -e "\033[0;33m[2/8] Create project + artifact...\033[0m"
	PROJECT=$$($(BIN) project create _test_s3_$$$$ 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "Slice 3 test data" > /tmp/_af_s3_test.txt
	ARTIFACT=$$($(BIN) artifact add /tmp/_af_s3_test.txt --project $$PROJECT 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "  Project: $$PROJECT  Artifact: $$ARTIFACT"
	echo -e "\033[0;33m[3/8] artifact info...\033[0m"
	$(BIN) artifact info $$ARTIFACT | grep -q "Filename:"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[4/8] artifact list...\033[0m"
	$(BIN) artifact list --project $$PROJECT | grep -q "$$ARTIFACT"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[5/8] project list...\033[0m"
	$(BIN) project list | grep -q "$$PROJECT"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[6/8] thread list (empty)...\033[0m"
	$(BIN) thread list --project $$PROJECT | grep -q "No threads"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[7/8] chat --help...\033[0m"
	$(BIN) chat --help | grep -q "project"
	echo -e "  \033[0;32mOK\033[0m"
	echo -e "\033[0;33m[8/8] thread --help...\033[0m"
	$(BIN) thread --help | grep -q "list"
	echo -e "  \033[0;32mOK\033[0m"
	rm -f /tmp/_af_s3_test.txt

test-tools: build ## Run all 5 file tools against a test file (needs DB)
	@set -e
	echo -e "\033[0;33mSetting up test data...\033[0m"
	PROJECT=$$($(BIN) project create _test_tools_$$$$ 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "ELF binary header simulation + some text for grep" > /tmp/_af_tools_test.bin
	printf '\x7fELF\x02\x01\x01' >> /tmp/_af_tools_test.bin
	ARTIFACT=$$($(BIN) artifact add /tmp/_af_tools_test.bin --project $$PROJECT 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "  Project: $$PROJECT  Artifact: $$ARTIFACT"
	echo ""
	PASS=0; FAIL=0
	for TOOL in file.info file.read_range file.hexdump file.strings file.grep; do
		echo -n "  $$TOOL... "
		case $$TOOL in
			file.info)       INPUT="{\"artifact_id\":\"$$ARTIFACT\"}" ;;
			file.read_range) INPUT="{\"artifact_id\":\"$$ARTIFACT\",\"offset\":0,\"length\":32}" ;;
			file.hexdump)    INPUT="{\"artifact_id\":\"$$ARTIFACT\",\"offset\":0,\"length\":64}" ;;
			file.strings)    INPUT="{\"artifact_id\":\"$$ARTIFACT\",\"min_length\":4}" ;;
			file.grep)       INPUT="{\"artifact_id\":\"$$ARTIFACT\",\"pattern\":\"ELF\",\"context_lines\":0}" ;;
		esac
		if $(BIN) tool run $$TOOL --project $$PROJECT --input "$$INPUT" 2>&1 | grep -q "completed"; then
			echo -e "\033[0;32mOK\033[0m"
			PASS=$$((PASS+1))
		else
			echo -e "\033[0;31mFAIL\033[0m"
			FAIL=$$((FAIL+1))
		fi
	done
	rm -f /tmp/_af_tools_test.bin
	echo ""
	echo "  $$PASS passed, $$FAIL failed"
	[ "$$FAIL" -eq 0 ] || exit 1

# ─── Interactive ───────────────────────────────────────────────────────────────

test-chat: build ## Launch interactive chat (needs DB + LLM backend)
	@set -e
	if [ -z "$$AF_ANTHROPIC_API_KEY" ] && [ -z "$$AF_OPENAI_ENDPOINT" ] && [ -z "$$AF_OPENAI_API_KEY" ] && [ -z "$$AF_VERTEX_ENDPOINT" ]; then
		echo -e "\033[0;31mNo LLM backend configured.\033[0m Set one of:"
		echo "  export AF_ANTHROPIC_API_KEY=..."
		echo "  export AF_OPENAI_API_KEY=..."
		echo "  export AF_OPENAI_ENDPOINT=http://localhost:11434"
		echo "  export AF_VERTEX_ENDPOINT=https://..."
		exit 1
	fi
	echo -e "\033[0;33mCreating test project...\033[0m"
	PROJECT=$$($(BIN) project create _test_chat_$$$$ 2>&1 | grep -oP '[0-9a-f-]{36}')
	echo "Test file for chat validation" > /tmp/_af_chat_test.txt
	$(BIN) artifact add /tmp/_af_chat_test.txt --project $$PROJECT > /dev/null
	echo -e "\033[0;32mStarting chat\033[0m (project: $$PROJECT)"
	echo "  Try: 'What can you tell me about the uploaded file?'"
	echo "  Slash commands: /tools /history /thread /help /quit"
	echo ""
	$(BIN) chat --agent default --project $$PROJECT
	rm -f /tmp/_af_chat_test.txt

# ─── Server (API + UI) ────────────────────────────────────────────────────────

serve: build ## Start API server with UI (needs DB + at least one LLM backend)
	@set -e
	# Keys and model names come from Makefile.local
	$(if $(AF_OPENAI_API_KEY),export AF_OPENAI_API_KEY="$(AF_OPENAI_API_KEY)";,)
	$(if $(AF_OPENAI_MODEL),export AF_OPENAI_MODEL="$(AF_OPENAI_MODEL)";,)
	$(if $(AF_ANTHROPIC_API_KEY),export AF_ANTHROPIC_API_KEY="$(AF_ANTHROPIC_API_KEY)";,)
	$(if $(AF_ANTHROPIC_MODEL),export AF_ANTHROPIC_MODEL="$(AF_ANTHROPIC_MODEL)";,)
	$(if $(AF_LOCAL_ENDPOINT),export AF_LOCAL_ENDPOINT="$(AF_LOCAL_ENDPOINT)";,)
	$(if $(AF_LOCAL_MODEL),export AF_LOCAL_MODEL="$(AF_LOCAL_MODEL)";,)
	$(if $(GHIDRA_HOME),export AF_GHIDRA_HOME="$(GHIDRA_HOME)";,)
	export AF_RIZIN_PATH="$(AF_RIZIN_PATH)"
	export AF_ALLOW_UNSANDBOXED=1
	export AF_CORS_ORIGIN="*"
	if [ -z "$$AF_ANTHROPIC_API_KEY" ] && [ -z "$$AF_OPENAI_ENDPOINT" ] && [ -z "$$AF_OPENAI_API_KEY" ] && [ -z "$$AF_VERTEX_ENDPOINT" ] && [ -z "$$AF_LOCAL_ENDPOINT" ]; then
		echo -e "\033[0;31mNo LLM backend configured.\033[0m"
		echo "  Edit Makefile.local with your API keys, or set env vars:"
		echo "  export AF_LOCAL_ENDPOINT=http://localhost:11434 (Ollama/vLLM/llama.cpp)"
		echo "  export AF_ANTHROPIC_API_KEY=..."
		echo "  export AF_OPENAI_API_KEY=..."
		echo "  export AF_VERTEX_ENDPOINT=https://..."
		exit 1
	fi
	BIND=$${AF_BIND_ADDR:-127.0.0.1:8080}
	echo -e "\033[0;36mStarting af API server...\033[0m"
	echo "  DB:      $${AF_DATABASE_URL}"
	$(if $(GHIDRA_HOME),echo "  Ghidra:  $(GHIDRA_HOME)";,)
	echo "  Rizin:   $(AF_RIZIN_PATH)"
	echo "  Bind:    $$BIND"
	echo "  UI:      http://$$BIND/ui/"
	echo ""
	$(BIN) serve --bind "$$BIND" --allow-insecure

serve-local: build ## Start API server with Ollama (localhost:11434)
	@set -e
	export AF_LOCAL_ENDPOINT=http://localhost:11434
	export AF_LOCAL_MODEL=$(OLLAMA_MODEL)
	$(if $(AF_OPENAI_API_KEY),export AF_OPENAI_API_KEY="$(AF_OPENAI_API_KEY)";,)
	$(if $(AF_OPENAI_MODEL),export AF_OPENAI_MODEL="$(AF_OPENAI_MODEL)";,)
	$(if $(AF_ANTHROPIC_API_KEY),export AF_ANTHROPIC_API_KEY="$(AF_ANTHROPIC_API_KEY)";,)
	$(if $(AF_ANTHROPIC_MODEL),export AF_ANTHROPIC_MODEL="$(AF_ANTHROPIC_MODEL)";,)
	$(if $(GHIDRA_HOME),export AF_GHIDRA_HOME="$(GHIDRA_HOME)";,)
	export AF_RIZIN_PATH="$(AF_RIZIN_PATH)"
	export AF_ALLOW_UNSANDBOXED=1
	export AF_CORS_ORIGIN="*"
	BIND=$${AF_BIND_ADDR:-127.0.0.1:8080}
	echo -e "\033[0;36mStarting af with Ollama...\033[0m"
	echo "  DB:      $${AF_DATABASE_URL}"
	echo "  Ollama:  http://localhost:11434 (model: $(OLLAMA_MODEL))"
	$(if $(GHIDRA_HOME),echo "  Ghidra:  $(GHIDRA_HOME)";,)
	echo "  Rizin:   $(AF_RIZIN_PATH)"
	echo "  Bind:    $$BIND"
	echo "  UI:      http://$$BIND/ui/"
	echo ""
	echo "  Override model: make serve-local OLLAMA_MODEL=llama3:8b"
	echo ""
	$(BIN) serve --bind "$$BIND"

serve-generic: build ## Start generic af (TOML plugins only, no compiled RE plugin)
	@set -e
	if [ -z "$$AF_ANTHROPIC_API_KEY" ] && [ -z "$$AF_OPENAI_ENDPOINT" ] && [ -z "$$AF_OPENAI_API_KEY" ] && [ -z "$$AF_VERTEX_ENDPOINT" ] && [ -z "$$AF_LOCAL_ENDPOINT" ]; then
		echo -e "\033[0;31mNo LLM backend configured.\033[0m Set at least one of:"
		echo "  export AF_LOCAL_ENDPOINT=http://localhost:11434 (Ollama/vLLM/llama.cpp)"
		echo "  export AF_ANTHROPIC_API_KEY=..."
		echo "  export AF_OPENAI_API_KEY=..."
		echo "  export AF_OPENAI_ENDPOINT=https://custom-openai-endpoint"
		echo "  export AF_VERTEX_ENDPOINT=https://..."
		exit 1
	fi
	export AF_ALLOW_UNSANDBOXED=1
	export AF_CORS_ORIGIN="*"
	BIND=$${AF_BIND_ADDR:-127.0.0.1:8080}
	echo -e "\033[0;36mStarting generic af API server (TOML plugins only)...\033[0m"
	echo "  DB:      $${AF_DATABASE_URL}"
	echo "  Bind:    $$BIND"
	echo "  UI:      http://$$BIND/ui/"
	echo ""
	echo "  To load specific TOML plugins: $(BIN_GENERIC) --plugin <name> serve"
	echo ""
	$(BIN_GENERIC) serve --bind "$$BIND"

# ─── Worker / Tick ────────────────────────────────────────────────────────────

worker: build ## Start background job worker daemon (needs DB)
	@set -e
	export AF_ALLOW_UNSANDBOXED=1
	$(if $(GHIDRA_HOME),export AF_GHIDRA_HOME="$(GHIDRA_HOME)";,)
	echo -e "\033[0;36mStarting worker daemon...\033[0m"
	echo "  DB:     $${AF_DATABASE_URL}"
	$(if $(GHIDRA_HOME),echo "  Ghidra: $(GHIDRA_HOME)";,)
	echo ""
	$(BIN) worker start --concurrency 4

tick: build ## Fire all due tick hooks once and exit (cron-friendly)
	@set -e
	echo -e "\033[0;36mFiring due tick hooks...\033[0m"
	$(BIN) tick

# ─── Database ──────────────────────────────────────────────────────────────────

db-status: ## Show database tables and row counts
	@set -e
	echo -e "\033[0;36mTables:\033[0m"
	psql $(DB_URL) -c "\dt" 2>/dev/null || echo "  Cannot connect. Is PostgreSQL running?"
	echo ""
	echo -e "\033[0;36mRow counts:\033[0m"
	for TABLE in projects artifacts tool_runs threads messages message_evidence users api_keys agents workflows audit_log project_members user_quotas project_hooks; do
		COUNT=$$(psql $(DB_URL) -t -c "SELECT count(*) FROM $$TABLE" 2>/dev/null | tr -d ' ')
		printf "  %-22s %s\n" "$$TABLE" "$${COUNT:-N/A}"
	done

clean-db: ## Drop and recreate the af database (DESTRUCTIVE)
	@set -e
	echo -e "\033[0;31mThis will DROP the af database and recreate it.\033[0m"
	read -p "Are you sure? [y/N] " confirm
	[ "$$confirm" = "y" ] || exit 1
	sudo -u postgres psql -c "DROP DATABASE IF EXISTS af;"
	sudo -u postgres psql -c "CREATE DATABASE af OWNER af;"
	sudo -u postgres psql -d af -c "GRANT ALL ON SCHEMA public TO af;" 2>/dev/null || true
	echo -e "\033[0;32mDatabase recreated.\033[0m Migrations will run on next command."

# ─── Clean ─────────────────────────────────────────────────────────────────────

clean: ## Remove build artifacts
	@cargo clean

clean-storage: ## Remove blob and scratch storage (DESTRUCTIVE)
	@set -e
	echo -e "\033[0;31mThis will delete /tmp/af/ (blobs + scratch).\033[0m"
	read -p "Are you sure? [y/N] " confirm
	[ "$$confirm" = "y" ] || exit 1
	rm -rf /tmp/af/
	echo -e "\033[0;32mStorage cleaned.\033[0m"

clean-all: clean clean-storage clean-db ## Remove everything: build + storage + database
