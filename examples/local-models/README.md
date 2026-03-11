# Local Model Card TOML Reference

Drop `.toml` files into `~/.af/models/` (or set `AF_MODELS_DIR`) to register model metadata without recompiling. Model cards are loaded at startup before the LLM router is built.

## Why model cards?

The static model catalog covers ~20 well-known models (GPT-4o, Claude, Gemini, etc.). When you use a custom or local model (Ollama, vLLM, etc.) or a newly released model not yet in the catalog, `lookup()` returns `None` and the runtime falls back to:

- **Context window**: 32,768 tokens (may be far too small or too large)
- **Max output tokens**: 4,096 tokens

This causes incorrect compaction triggers, wrong context window percentages, and missing cost estimates. A TOML model card fixes all of these.

## Format

```toml
[model]
# Required: model name — must match the model name in your backend configuration.
# For Ollama: matches AF_OPENAI_MODEL or names in AF_OPENAI_MODELS.
# For Anthropic: matches the model ID (e.g. "claude-opus-4-6").
name = "my-model"

# Required: total context window size in tokens (must be > 0)
context_window = 65536

# Required: maximum output tokens per response (must be > 0)
max_output_tokens = 8192

# Optional: pricing in USD per million tokens (default: 0.0)
cost_per_mtok_input = 0.55
cost_per_mtok_output = 2.19

# Optional: cached input token pricing (provider-specific)
# Anthropic: cache hits = 10% of input, cache creation (5min) = 125% of input
# OpenAI: cached tokens are a subset of prompt tokens, discounted rate
# Google: context caching read rate
cost_per_mtok_cached_input = 0.14

# Optional: Anthropic-only cache creation pricing
cost_per_mtok_cache_creation = 0.0

# Optional: whether the model accepts image inputs (default: false)
supports_vision = false

# Optional: training data cutoff date (YYYY-MM format)
knowledge_cutoff = "2025-01"
```

## Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `name` | yes | — | Model name. Must match exactly what your LLM backend uses. |
| `context_window` | yes | — | Total context window in tokens. Must be > 0. |
| `max_output_tokens` | yes | — | Maximum output tokens per response. Must be > 0. |
| `cost_per_mtok_input` | no | `0.0` | USD per million input tokens. |
| `cost_per_mtok_output` | no | `0.0` | USD per million output tokens. |
| `cost_per_mtok_cached_input` | no | — | USD per million cached input tokens. `None` = use standard input rate. |
| `cost_per_mtok_cache_creation` | no | — | USD per million cache creation tokens (Anthropic 5-min writes). `None` = not applicable. |
| `supports_vision` | no | `false` | Whether the model supports image inputs. |
| `knowledge_cutoff` | no | — | Training data cutoff (e.g. `"2025-01"`). Informational only. |

## Lookup behavior

1. Local TOML models are checked **first** — they override the static catalog
2. Exact name match is tried first
3. If no match, date suffixes are stripped (`-YYYY-MM-DD` or `-YYYYMMDD`) and retried
4. Falls back to the static catalog with the same logic

This means a TOML card for `"gpt-4o"` would override the built-in GPT-4o entry.

## Example model cards (in this directory)

### Cloud API models

| File | Model | Context | Max Output | Provider |
|---|---|---|---|---|
| `claude-opus-4-6.toml` | `claude-opus-4-6` | 200K | 128K | Anthropic |
| `claude-sonnet-4-6.toml` | `claude-sonnet-4-6` | 200K | 64K | Anthropic |
| `claude-haiku-4-5.toml` | `claude-haiku-4-5-20251001` | 200K | 64K | Anthropic |
| `gemini-3.1-pro-preview.toml` | `gemini-3.1-pro-preview` | 1M | 64K | Google |
| `gemini-3-flash-preview.toml` | `gemini-3-flash-preview` | 1M | 64K | Google |

### Local Ollama models (47 models, auto-generated from `ollama list`)

| File | Model | Params | Context | Vision |
|---|---|---|---|---|
| `qwen3-32b.toml` | `qwen3:32b` | 32.8B | 40K | |
| `qwen3-30b.toml` | `qwen3:30b` | 30.5B MoE | 256K | |
| `qwen3-14b.toml` | `qwen3:14b` | 14.8B | 40K | |
| `qwen3-latest.toml` | `qwen3:latest` | 8.2B | 40K | |
| `qwen3-4b.toml` | `qwen3:4b` | 4.0B | 256K | |
| `qwq-latest.toml` | `qwq:latest` | 32.8B | 128K | |
| `qwen2.5-coder-32b.toml` | `qwen2.5-coder:32b` | 32.8B | 32K | |
| `qwen-110b.toml` | `qwen:110b` | 111.2B | 32K | |
| `qwen-72b.toml` | `qwen:72b` | 72.3B | 32K | |
| `qwen-14b.toml` | `qwen:14b` | 14.2B | 32K | |
| `qwen-latest.toml` | `qwen:latest` | 4.0B | 32K | |
| `gemma3-27b.toml` | `gemma3:27b` | 27.4B | 128K | yes |
| `gemma3-12b.toml` | `gemma3:12b` | 12.2B | 128K | yes |
| `gemma3-4b.toml` | `gemma3:4b` | 4.3B | 128K | yes |
| `gemma3-1b.toml` | `gemma3:1b` | 999M | 32K | yes |
| `gemma3n-latest.toml` | `gemma3n:latest` | 6.9B | 32K | yes |
| `gemma2-27b.toml` | `gemma2:27b` | 27.2B | 8K | |
| `gemma2-9b.toml` | `gemma2:9b` | 9.2B | 8K | |
| `gemma-7b.toml` | `gemma:7b` | 8.5B | 8K | |
| `llama4-maverick.toml` | `llama4:maverick` | 401.6B MoE | 1M | yes |
| `llama4-latest.toml` | `llama4:latest` | 108.6B | 10M | yes |
| `llama3.3-latest.toml` | `llama3.3:latest` | 70.6B | 128K | |
| `llama3.1-70b.toml` | `llama3.1:70b` | 70.6B | 128K | |
| `llama3.1-latest.toml` | `llama3.1:latest` | 8.0B | 128K | |
| `llama3-70b.toml` | `llama3:70b` | 70.6B | 8K | |
| `llama3-latest.toml` | `llama3:latest` | 8.0B | 8K | |
| `llama2-latest.toml` | `llama2:latest` | 6.7B | 4K | |
| `llama2-uncensored-latest.toml` | `llama2-uncensored:latest` | 6.7B | 2K | |
| `deepseek-r1-70b.toml` | `deepseek-r1:70b` | 70.6B | 128K | |
| `deepseek-r1-32b.toml` | `deepseek-r1:32b` | 32.8B | 128K | |
| `mistral-small3.2-latest.toml` | `mistral-small3.2:latest` | 24.0B | 128K | yes |
| `mistral-small3.1-latest.toml` | `mistral-small3.1:latest` | 24.0B | 128K | yes |
| `mistral-large-latest.toml` | `mistral-large:latest` | 122.6B | 32K | |
| `mistral-nemo-latest.toml` | `mistral-nemo:latest` | 12.2B | 1M | |
| `mistral-latest.toml` | `mistral:latest` | 7.2B | 32K | |
| `devstral-latest.toml` | `devstral:latest` | 23.6B | 128K | |
| `magistral-latest.toml` | `magistral:latest` | 23.6B | 40K | |
| `gpt-oss-120b.toml` | `gpt-oss:120b` | 116.8B | 128K | |
| `gpt-oss-latest.toml` | `gpt-oss:latest` | 20.9B | 128K | |
| `phi4-latest.toml` | `phi4:latest` | 14.7B | 16K | |
| `phi3-medium-128k.toml` | `phi3:medium-128k` | 14.0B | 128K | |
| `phi3-latest.toml` | `phi3:latest` | 3.8B | 128K | |
| `mixtral-latest.toml` | `mixtral:latest` | 46.7B MoE | 32K | |
| `dolphin-mixtral-latest.toml` | `dolphin-mixtral:latest` | 46.7B MoE | 32K | |
| `codellama-latest.toml` | `codellama:latest` | 6.7B | 16K | |
| `orca2-latest.toml` | `orca2:latest` | 6.7B | 4K | |
| `wizard-math-latest.toml` | `wizard-math:latest` | 7.2B | 32K | |

## Models already in the static catalog

These models are built-in and do **not** need TOML cards (unless you want to override their specs):

**OpenAI**: `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini`, `gpt-4.1-nano`, `o3`, `o3-mini`, `o4-mini`

**Anthropic**: `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-sonnet-4-20250514`, `claude-opus-4-20250514`, `claude-haiku-4-5-20251001`, `claude-haiku-3-5-20241022`

**Google Gemini**: `gemini-3.1-pro-preview`, `gemini-3-pro-preview`, `gemini-3-flash-preview`, `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.0-flash`

## Notes

- TOML files are loaded once at startup — restart `af` after adding/modifying files
- The `name` field must match your backend's model name exactly (case-sensitive)
- For Ollama models, the name typically matches what you pass to `AF_OPENAI_MODEL`
- Cost fields are only used for cost estimation in logs and reports — they don't affect billing
- Invalid files (parse errors, missing required fields, zero values) are logged as warnings and skipped
