use af_core::{OutputRedirectPolicy, SandboxProfile, ToolPolicy, ToolSpec};
use serde_json::json;

fn web_tool_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted, // In-process, talks to gateway via UDS
        timeout_ms: 45_000,               // 45s (gateway has 30s fetch + overhead)
        allow_exec: false,
        max_calls_per_run: 15,
        ..ToolPolicy::default()
    }
}

pub fn web_fetch_spec() -> ToolSpec {
    ToolSpec {
        name: "web.fetch".to_string(),
        version: 1,
        deprecated: false,
        description: "Fetch a web page or resource by URL. Returns the page content as plain text \
                      (HTML is automatically converted). Respects URL allowlist/blocklist rules \
                      and country-based IP restrictions. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "format": "uri",
                    "description": "The URL to fetch (must be http:// or https://)"
                },
                "extract_links": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, also extract and return all hyperlinks from the page"
                }
            },
            "required": ["url"]
        }),
        policy: web_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn web_search_spec() -> ToolSpec {
    ToolSpec {
        name: "web.search".to_string(),
        version: 1,
        deprecated: false,
        description: "Search the web using DuckDuckGo. Returns a list of results with titles, URLs, \
                      and snippets. Use web.fetch to retrieve full page content from promising results. \
                      Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (max 200 characters)"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "default": 10,
                    "description": "Maximum number of results to return"
                }
            },
            "required": ["query"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 30_000,
            allow_exec: false,
            max_calls_per_run: 5,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn all_specs() -> Vec<ToolSpec> {
    vec![web_fetch_spec(), web_search_spec()]
}
