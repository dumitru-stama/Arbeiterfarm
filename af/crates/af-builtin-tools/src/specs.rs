use af_core::{OutputRedirectPolicy, SandboxProfile, ToolPolicy, ToolSpec};
use serde_json::json;

fn file_tool_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::NoNetReadOnly,
        max_input_bytes: 64 * 1024,
        max_input_depth: 8,
        timeout_ms: 30_000,
        max_stdout_bytes: 1024 * 1024,
        max_stderr_bytes: 256 * 1024,
        max_output_bytes: 16 * 1024 * 1024,
        max_produced_artifacts: 0,
        ..ToolPolicy::default()
    }
}

fn artifact_id_defs() -> serde_json::Value {
    json!({
        "ArtifactId": {
            "type": "string",
            "format": "uuid",
            "description": "UUID of an artifact in the current project"
        }
    })
}

pub fn file_info_spec() -> ToolSpec {
    ToolSpec {
        name: "file.info".to_string(),
        version: 1,
        deprecated: false,
        description: "Get file metadata: size, hashes (MD5/SHA1/SHA256), magic bytes detection"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            },
            "required": ["artifact_id"]
        }),
        policy: file_tool_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn file_read_range_spec() -> ToolSpec {
    ToolSpec {
        name: "file.read_range".to_string(),
        version: 1,
        deprecated: false,
        description: "Read a byte or line range from a file".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "offset": { "type": "integer", "minimum": 0, "description": "Byte offset to start reading from" },
                "length": { "type": "integer", "minimum": 1, "description": "Number of bytes to read" },
                "line_start": { "type": "integer", "minimum": 1, "description": "Start line (1-indexed)" },
                "line_count": { "type": "integer", "minimum": 1, "description": "Number of lines to read" }
            },
            "required": ["artifact_id"]
        }),
        policy: file_tool_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn file_strings_spec() -> ToolSpec {
    ToolSpec {
        name: "file.strings".to_string(),
        version: 1,
        deprecated: false,
        description: "Extract printable strings from a file (like the `strings` command). \
                      Returns a summary with top 20 strings; full list stored as artifact."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "min_length": { "type": "integer", "minimum": 1, "default": 4, "description": "Minimum string length" },
                "max_strings": { "type": "integer", "minimum": 1, "default": 1000, "description": "Maximum number of strings to return" }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            max_produced_artifacts: 1,
            ..file_tool_policy()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn file_hexdump_spec() -> ToolSpec {
    ToolSpec {
        name: "file.hexdump".to_string(),
        version: 1,
        deprecated: false,
        description: "Hex + ASCII dump of a byte range from a file".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Byte offset" },
                "length": { "type": "integer", "minimum": 1, "default": 256, "description": "Number of bytes to dump" }
            },
            "required": ["artifact_id"]
        }),
        policy: file_tool_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn file_grep_spec() -> ToolSpec {
    ToolSpec {
        name: "file.grep".to_string(),
        version: 1,
        deprecated: false,
        description: "Search for a regex pattern in a file with context lines. \
                      Returns a summary with top 5 matches; full results stored as artifact."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "pattern": { "type": "string", "description": "Regex pattern to search for" },
                "context_lines": { "type": "integer", "minimum": 0, "default": 2, "description": "Lines of context around each match" },
                "max_matches": { "type": "integer", "minimum": 1, "default": 100, "description": "Maximum number of matches to return" }
            },
            "required": ["artifact_id", "pattern"]
        }),
        policy: ToolPolicy {
            max_produced_artifacts: 1,
            ..file_tool_policy()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

// ---------------------------------------------------------------------------
// Embedding tools (Trusted, in-process)
// ---------------------------------------------------------------------------

fn embed_tool_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted,
        timeout_ms: 60_000,
        allow_exec: false,
        ..ToolPolicy::default()
    }
}

fn embed_batch_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted,
        timeout_ms: 180_000, // 3 min — batch of 100 texts may need model loading time
        allow_exec: false,
        ..ToolPolicy::default()
    }
}

pub fn embed_text_spec() -> ToolSpec {
    ToolSpec {
        name: "embed.text".to_string(),
        version: 1,
        deprecated: false,
        description: "Generate a vector embedding for text and store it in the project. \
                      Use for indexing function names, descriptions, decompiled code, etc."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "text": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The text to embed"
                },
                "label": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Label identifying this embedding (e.g. function name, address)"
                },
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "metadata": {
                    "type": "object",
                    "description": "Optional metadata to associate with the embedding"
                }
            },
            "required": ["text", "label"]
        }),
        policy: embed_tool_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn embed_search_spec() -> ToolSpec {
    ToolSpec {
        name: "embed.search".to_string(),
        version: 1,
        deprecated: false,
        description: "Search for similar embeddings by text query using cosine similarity. \
                      Find functions, descriptions, or code similar to the query."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Text to search for (will be embedded and compared)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 10,
                    "description": "Maximum number of results to return"
                },
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            },
            "required": ["query"]
        }),
        policy: embed_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn embed_batch_spec() -> ToolSpec {
    ToolSpec {
        name: "embed.batch".to_string(),
        version: 1,
        deprecated: false,
        description: "Batch-embed multiple texts in a single call for efficiency. \
                      Useful for indexing many functions or code snippets at once."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "minLength": 1, "description": "Text to embed" },
                            "label": { "type": "string", "minLength": 1, "description": "Label for this embedding" },
                            "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                            "metadata": { "type": "object" }
                        },
                        "required": ["text", "label"]
                    },
                    "description": "Array of items to embed"
                }
            },
            "required": ["items"]
        }),
        policy: embed_batch_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn embed_list_spec() -> ToolSpec {
    ToolSpec {
        name: "embed.list".to_string(),
        version: 1,
        deprecated: false,
        description: "List stored embeddings for the project, optionally filtered by artifact or model."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "model": {
                    "type": "string",
                    "description": "Filter by embedding model name"
                }
            }
        }),
        policy: embed_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn embed_specs() -> Vec<ToolSpec> {
    vec![
        embed_text_spec(),
        embed_search_spec(),
        embed_batch_spec(),
        embed_list_spec(),
    ]
}

pub fn all_specs() -> Vec<ToolSpec> {
    vec![
        file_info_spec(),
        file_read_range_spec(),
        file_strings_spec(),
        file_hexdump_spec(),
        file_grep_spec(),
    ]
}
