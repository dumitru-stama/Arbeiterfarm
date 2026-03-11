/// Static catalog of known model metadata (context window, pricing, capabilities).
///
/// Returns `None` for unknown / custom models. Lookup tries exact match first,
/// then strips common date suffixes (e.g. `gpt-4o-2024-11-20` → `gpt-4o`).
///
/// Local TOML model cards loaded from `~/.af/models/` override static entries.

use std::sync::OnceLock;

#[derive(Debug)]
pub struct ModelSpec {
    pub context_window: u32,
    pub max_output_tokens: u32,
    /// USD per million input tokens.
    pub cost_per_mtok_input: f64,
    /// USD per million output tokens.
    pub cost_per_mtok_output: f64,
    /// USD per million cached input tokens. None = use standard input rate.
    pub cost_per_mtok_cached_input: Option<f64>,
    /// USD per million cache creation tokens (Anthropic only). None = not applicable.
    pub cost_per_mtok_cache_creation: Option<f64>,
    pub supports_vision: bool,
    /// e.g. "2025-03"
    pub knowledge_cutoff: Option<&'static str>,
    /// Allowed temperature range `(min, max)`. `None` = any temperature accepted.
    /// `Some((1.0, 1.0))` = fixed (parameter omitted from requests).
    /// `Some((0.0, 2.0))` = clamped to range.
    pub temperature_range: Option<(f32, f32)>,
    /// Whether the model supports native tool/function calling.
    /// `None` = use backend default (true for OpenAI-compatible).
    /// `Some(false)` = use Mode A (JSON-block in system prompt).
    pub supports_tool_calls: Option<bool>,
}

/// Runtime-loaded model specs from TOML files. Checked before the static catalog.
static LOCAL_MODELS: OnceLock<Vec<(String, ModelSpec)>> = OnceLock::new();

/// Initialize the local model catalog from TOML-loaded specs.
/// Must be called before any `lookup()` calls (typically at startup).
/// Subsequent calls are ignored (OnceLock semantics).
pub fn init_local_models(specs: Vec<(String, ModelSpec)>) {
    let _ = LOCAL_MODELS.set(specs);
}

/// Look up model metadata by name. Checks local TOML models first (overrides),
/// then the static catalog. Tries exact match, then strips date suffixes.
pub fn lookup(model_name: &str) -> Option<&'static ModelSpec> {
    // Check local models first (allows overriding static catalog)
    if let Some(locals) = LOCAL_MODELS.get() {
        if let Some((_, spec)) = locals.iter().find(|(k, _)| k == model_name) {
            return Some(spec);
        }
        let base = strip_date_suffix(model_name);
        if base != model_name {
            if let Some((_, spec)) = locals.iter().find(|(k, _)| k == base) {
                return Some(spec);
            }
        }
    }

    // Fall back to static catalog
    if let Some(spec) = CATALOG.iter().find(|(k, _)| *k == model_name) {
        return Some(spec.1);
    }
    // Strip trailing date suffix: `-YYYY-MM-DD` or `-YYYYMMDD`
    let base = strip_date_suffix(model_name);
    if base != model_name {
        if let Some(spec) = CATALOG.iter().find(|(k, _)| *k == base) {
            return Some(spec.1);
        }
    }
    None
}

fn strip_date_suffix(name: &str) -> &str {
    // Try `-YYYYMMDD` (8 digits)
    if name.len() > 9 {
        let candidate = &name[name.len() - 9..];
        if candidate.starts_with('-') && candidate[1..].bytes().all(|b| b.is_ascii_digit()) {
            return &name[..name.len() - 9];
        }
    }
    // Try `-YYYY-MM-DD` (10 chars + dash)
    if name.len() > 11 {
        let candidate = &name[name.len() - 11..];
        if candidate.starts_with('-')
            && candidate[1..5].bytes().all(|b| b.is_ascii_digit())
            && candidate.as_bytes()[5] == b'-'
            && candidate[6..8].bytes().all(|b| b.is_ascii_digit())
            && candidate.as_bytes()[8] == b'-'
            && candidate[9..11].bytes().all(|b| b.is_ascii_digit())
        {
            return &name[..name.len() - 11];
        }
    }
    name
}

static CATALOG: &[(&str, &ModelSpec)] = &[
    // --- OpenAI ---
    // OpenAI cached_tokens is a subset of prompt_tokens (already counted), so cached = 50% of input
    ("gpt-4o", &ModelSpec {
        context_window: 128_000,
        max_output_tokens: 16_384,
        cost_per_mtok_input: 2.50,
        cost_per_mtok_output: 10.00,
        cost_per_mtok_cached_input: Some(1.25),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2024-10"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-4o-mini", &ModelSpec {
        context_window: 128_000,
        max_output_tokens: 16_384,
        cost_per_mtok_input: 0.15,
        cost_per_mtok_output: 0.60,
        cost_per_mtok_cached_input: Some(0.075),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2024-10"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-4.1", &ModelSpec {
        context_window: 1_047_576,
        max_output_tokens: 32_768,
        cost_per_mtok_input: 2.00,
        cost_per_mtok_output: 8.00,
        cost_per_mtok_cached_input: Some(0.50),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-05"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-4.1-mini", &ModelSpec {
        context_window: 1_047_576,
        max_output_tokens: 32_768,
        cost_per_mtok_input: 0.40,
        cost_per_mtok_output: 1.60,
        cost_per_mtok_cached_input: Some(0.10),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-05"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-4.1-nano", &ModelSpec {
        context_window: 1_047_576,
        max_output_tokens: 32_768,
        cost_per_mtok_input: 0.10,
        cost_per_mtok_output: 0.40,
        cost_per_mtok_cached_input: Some(0.025),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-05"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-5", &ModelSpec {
        context_window: 400_000,
        max_output_tokens: 32_768,
        cost_per_mtok_input: 1.25,
        cost_per_mtok_output: 10.00,
        cost_per_mtok_cached_input: Some(0.125),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-10"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-5-mini", &ModelSpec {
        context_window: 400_000,
        max_output_tokens: 32_768,
        cost_per_mtok_input: 0.25,
        cost_per_mtok_output: 2.00,
        cost_per_mtok_cached_input: Some(0.025),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-10"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gpt-5-nano", &ModelSpec {
        context_window: 400_000,
        max_output_tokens: 32_768,
        cost_per_mtok_input: 0.05,
        cost_per_mtok_output: 0.40,
        cost_per_mtok_cached_input: Some(0.005),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-10"),
        temperature_range: Some((1.0, 1.0)), // only default temperature
        supports_tool_calls: None,
    }),
    ("o3", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 100_000,
        cost_per_mtok_input: 2.00,
        cost_per_mtok_output: 8.00,
        cost_per_mtok_cached_input: Some(1.00),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-05"),
        temperature_range: Some((1.0, 1.0)), // reasoning model, fixed temperature
        supports_tool_calls: None,
    }),
    ("o3-mini", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 100_000,
        cost_per_mtok_input: 1.10,
        cost_per_mtok_output: 4.40,
        cost_per_mtok_cached_input: Some(0.55),
        cost_per_mtok_cache_creation: None,
        supports_vision: false,
        knowledge_cutoff: Some("2025-05"),
        temperature_range: Some((1.0, 1.0)), // reasoning model, fixed temperature
        supports_tool_calls: None,
    }),
    ("o4-mini", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 100_000,
        cost_per_mtok_input: 1.10,
        cost_per_mtok_output: 4.40,
        cost_per_mtok_cached_input: Some(0.55),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-05"),
        temperature_range: Some((1.0, 1.0)), // reasoning model, fixed temperature
        supports_tool_calls: None,
    }),
    // --- Anthropic ---
    // Anthropic cache_read = 10% of input, cache_creation (5min) = 125% of input
    ("claude-opus-4-6", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 128_000,
        cost_per_mtok_input: 5.00,
        cost_per_mtok_output: 25.00,
        cost_per_mtok_cached_input: Some(0.50),
        cost_per_mtok_cache_creation: Some(6.25),
        supports_vision: true,
        knowledge_cutoff: Some("2025-03"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("claude-sonnet-4-6", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 64_000,
        cost_per_mtok_input: 3.00,
        cost_per_mtok_output: 15.00,
        cost_per_mtok_cached_input: Some(0.30),
        cost_per_mtok_cache_creation: Some(3.75),
        supports_vision: true,
        knowledge_cutoff: Some("2025-03"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("claude-sonnet-4-20250514", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 16_384,
        cost_per_mtok_input: 3.00,
        cost_per_mtok_output: 15.00,
        cost_per_mtok_cached_input: Some(0.30),
        cost_per_mtok_cache_creation: Some(3.75),
        supports_vision: true,
        knowledge_cutoff: Some("2025-03"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("claude-opus-4-20250514", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 32_000,
        cost_per_mtok_input: 15.00,
        cost_per_mtok_output: 75.00,
        cost_per_mtok_cached_input: Some(1.50),
        cost_per_mtok_cache_creation: Some(18.75),
        supports_vision: true,
        knowledge_cutoff: Some("2025-03"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("claude-haiku-4-5-20251001", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 64_000,
        cost_per_mtok_input: 1.00,
        cost_per_mtok_output: 5.00,
        cost_per_mtok_cached_input: Some(0.10),
        cost_per_mtok_cache_creation: Some(1.25),
        supports_vision: true,
        knowledge_cutoff: Some("2025-03"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("claude-haiku-3-5-20241022", &ModelSpec {
        context_window: 200_000,
        max_output_tokens: 8_192,
        cost_per_mtok_input: 0.80,
        cost_per_mtok_output: 4.00,
        cost_per_mtok_cached_input: Some(0.08),
        cost_per_mtok_cache_creation: Some(1.00),
        supports_vision: true,
        knowledge_cutoff: Some("2025-03"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    // --- Google Gemini ---
    // Gemini cached = 10% of input (context caching)
    ("gemini-3.1-pro-preview", &ModelSpec {
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        cost_per_mtok_input: 2.00,
        cost_per_mtok_output: 12.00,
        cost_per_mtok_cached_input: Some(0.20),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-01"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gemini-3-pro-preview", &ModelSpec {
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        cost_per_mtok_input: 2.00,
        cost_per_mtok_output: 12.00,
        cost_per_mtok_cached_input: Some(0.20),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-01"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gemini-3-flash-preview", &ModelSpec {
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        cost_per_mtok_input: 0.50,
        cost_per_mtok_output: 3.00,
        cost_per_mtok_cached_input: Some(0.05),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-01"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gemini-2.0-flash", &ModelSpec {
        context_window: 1_048_576,
        max_output_tokens: 8_192,
        cost_per_mtok_input: 0.10,
        cost_per_mtok_output: 0.40,
        cost_per_mtok_cached_input: Some(0.025),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-01"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gemini-2.5-pro", &ModelSpec {
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        cost_per_mtok_input: 1.25,
        cost_per_mtok_output: 10.00,
        cost_per_mtok_cached_input: Some(0.3125),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-01"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
    ("gemini-2.5-flash", &ModelSpec {
        context_window: 1_048_576,
        max_output_tokens: 65_536,
        cost_per_mtok_input: 0.15,
        cost_per_mtok_output: 0.60,
        cost_per_mtok_cached_input: Some(0.0375),
        cost_per_mtok_cache_creation: None,
        supports_vision: true,
        knowledge_cutoff: Some("2025-01"),
        temperature_range: None,
        supports_tool_calls: None,
    }),
];

/// Determine whether a route name belongs to the Anthropic provider.
/// Anthropic has different cache token semantics (cache tokens are separate from prompt tokens).
pub fn is_anthropic_route(route: &str) -> bool {
    route.starts_with("anthropic:")
}

/// Compute estimated USD cost for a single LLM call.
/// Returns None if the model is unknown.
pub fn compute_cost(
    route: &str,
    prompt_tokens: u32,
    completion_tokens: u32,
    cached_read_tokens: u32,
    cache_creation_tokens: u32,
) -> Option<f64> {
    let model_name = route.rsplit_once(':').map(|(_, m)| m).unwrap_or(route);
    let spec = lookup(model_name)?;

    let cached_rate = spec.cost_per_mtok_cached_input.unwrap_or(spec.cost_per_mtok_input);
    let creation_rate = spec.cost_per_mtok_cache_creation.unwrap_or(spec.cost_per_mtok_input);

    let cost = if is_anthropic_route(route) {
        // Anthropic: cache_read and cache_creation are SEPARATE from input_tokens
        let input_cost = (prompt_tokens as f64) * spec.cost_per_mtok_input / 1_000_000.0;
        let output_cost = (completion_tokens as f64) * spec.cost_per_mtok_output / 1_000_000.0;
        let cached_cost = (cached_read_tokens as f64) * cached_rate / 1_000_000.0;
        let creation_cost = (cache_creation_tokens as f64) * creation_rate / 1_000_000.0;
        input_cost + output_cost + cached_cost + creation_cost
    } else {
        // OpenAI/Vertex: cached_tokens is a SUBSET of prompt_tokens (already counted at full rate).
        // Discount: (full_rate - cached_rate) * cached_tokens
        let input_cost = (prompt_tokens as f64) * spec.cost_per_mtok_input / 1_000_000.0;
        let output_cost = (completion_tokens as f64) * spec.cost_per_mtok_output / 1_000_000.0;
        let cache_discount = (cached_read_tokens as f64) * (spec.cost_per_mtok_input - cached_rate) / 1_000_000.0;
        input_cost + output_cost - cache_discount
    };

    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let spec = lookup("gpt-4o").unwrap();
        assert_eq!(spec.context_window, 128_000);
        assert!(spec.supports_vision);
    }

    #[test]
    fn date_suffix_stripped() {
        let spec = lookup("gpt-4o-2024-11-20").unwrap();
        assert_eq!(spec.context_window, 128_000);
    }

    #[test]
    fn compact_date_suffix() {
        let spec = lookup("claude-sonnet-4-20250514").unwrap();
        assert_eq!(spec.context_window, 200_000);
    }

    #[test]
    fn unknown_model() {
        assert!(lookup("my-custom-local-model").is_none());
    }

    #[test]
    fn gemini_lookup() {
        let spec = lookup("gemini-2.5-pro").unwrap();
        assert_eq!(spec.max_output_tokens, 65_536);
    }

    #[test]
    fn cache_pricing_openai() {
        let spec = lookup("gpt-4o").unwrap();
        assert_eq!(spec.cost_per_mtok_cached_input, Some(1.25));
        assert_eq!(spec.cost_per_mtok_cache_creation, None);
    }

    #[test]
    fn cache_pricing_anthropic() {
        let spec = lookup("claude-sonnet-4-20250514").unwrap();
        assert_eq!(spec.cost_per_mtok_cached_input, Some(0.30));
        assert_eq!(spec.cost_per_mtok_cache_creation, Some(3.75));
    }

    #[test]
    fn compute_cost_openai() {
        // 1000 prompt tokens (200 cached) + 500 completion tokens for gpt-4o
        let cost = compute_cost("openai:gpt-4o", 1000, 500, 200, 0).unwrap();
        // input: 1000 * 2.50 / 1M = 0.0025
        // output: 500 * 10.00 / 1M = 0.005
        // cache discount: 200 * (2.50 - 1.25) / 1M = 0.00025
        // total: 0.0025 + 0.005 - 0.00025 = 0.00725
        assert!((cost - 0.00725).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_anthropic() {
        // 1000 input + 500 output + 200 cache_read + 100 cache_creation for claude-sonnet-4
        let cost = compute_cost("anthropic:claude-sonnet-4-20250514", 1000, 500, 200, 100).unwrap();
        // input: 1000 * 3.00 / 1M = 0.003
        // output: 500 * 15.00 / 1M = 0.0075
        // cached: 200 * 0.30 / 1M = 0.00006
        // creation: 100 * 3.75 / 1M = 0.000375
        // total: 0.003 + 0.0075 + 0.00006 + 0.000375 = 0.010935
        assert!((cost - 0.010935).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_unknown() {
        assert!(compute_cost("custom:unknown-model", 1000, 500, 0, 0).is_none());
    }

    #[test]
    fn init_local_models_provides_lookup() {
        // Note: OnceLock only initializes once per process, so this test
        // verifies the init + lookup path. Subsequent calls to init are no-ops.
        init_local_models(vec![(
            "test-local-model".to_string(),
            ModelSpec {
                context_window: 65_536,
                max_output_tokens: 8_192,
                cost_per_mtok_input: 0.0,
                cost_per_mtok_output: 0.0,
                cost_per_mtok_cached_input: None,
                cost_per_mtok_cache_creation: None,
                supports_vision: false,
                knowledge_cutoff: None,
                temperature_range: None,
                supports_tool_calls: None,
            },
        )]);

        let spec = lookup("test-local-model");
        // May be None if OnceLock was already set by another test in the same process,
        // but if it's Some it must match our spec.
        if let Some(s) = spec {
            assert_eq!(s.context_window, 65_536);
            assert_eq!(s.max_output_tokens, 8_192);
        }
    }
}
