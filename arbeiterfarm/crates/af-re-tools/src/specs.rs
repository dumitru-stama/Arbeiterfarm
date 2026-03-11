use af_plugin_api::{OutputRedirectPolicy, SandboxProfile, ToolPolicy, ToolSpec};
use serde_json::json;
use std::path::Path;

fn re_tool_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::NoNetReadOnly,
        timeout_ms: 60_000,
        max_output_bytes: 64 * 1024 * 1024,
        max_produced_artifacts: 4,
        allow_exec: true,
        ..Default::default()
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

pub fn rizin_bininfo_spec() -> ToolSpec {
    ToolSpec {
        name: "rizin.bininfo".to_string(),
        version: 1,
        deprecated: false,
        description: "Binary metadata via rizin: imports, exports, sections, entry point, architecture".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            },
            "required": ["artifact_id"]
        }),
        policy: re_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn rizin_disasm_spec() -> ToolSpec {
    ToolSpec {
        name: "rizin.disasm".to_string(),
        version: 1,
        deprecated: false,
        description: "Disassemble an address range using rizin".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "address": {
                    "type": "string",
                    "pattern": "^0x[0-9a-fA-F]+$",
                    "description": "Start address in hex (e.g. 0x00401000)"
                },
                "length": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4096,
                    "description": "Number of instructions to disassemble"
                }
            },
            "required": ["artifact_id", "address", "length"]
        }),
        policy: re_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn rizin_xrefs_spec() -> ToolSpec {
    ToolSpec {
        name: "rizin.xrefs".to_string(),
        version: 1,
        deprecated: false,
        description: "Cross-references for a function/address using rizin".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "address": {
                    "type": "string",
                    "pattern": "^0x[0-9a-fA-F]+$",
                    "description": "Address to find cross-references for"
                },
                "direction": {
                    "type": "string",
                    "enum": ["to", "from", "both"],
                    "default": "both",
                    "description": "Direction of cross-references"
                }
            },
            "required": ["artifact_id", "address"]
        }),
        policy: re_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn strings_extract_spec() -> ToolSpec {
    ToolSpec {
        name: "strings.extract".to_string(),
        version: 1,
        deprecated: false,
        description: "Advanced string extraction with encoding detection using rizin".to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "min_length": {
                    "type": "integer",
                    "minimum": 2,
                    "maximum": 256,
                    "default": 4,
                    "description": "Minimum string length to include"
                },
                "encoding": {
                    "type": "string",
                    "enum": ["ascii", "utf8", "utf16le", "utf16be", "all"],
                    "default": "all",
                    "description": "String encoding filter"
                },
                "max_strings": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10000,
                    "default": 1000,
                    "description": "Maximum number of strings to return"
                }
            },
            "required": ["artifact_id"]
        }),
        policy: re_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn ghidra_analyze_spec(ghidra_home: &Path, cache_dir: &Path, scripts_dir: &Path, java_home: Option<&Path>) -> ToolSpec {
    let mut ro_mounts = vec![ghidra_home.to_path_buf(), scripts_dir.to_path_buf()];
    if let Some(jh) = java_home {
        ro_mounts.push(jh.to_path_buf());
    }
    ToolSpec {
        name: "ghidra.analyze".to_string(),
        version: 1,
        deprecated: false,
        description: "Run headless Ghidra analysis on a binary. Returns function list with \
                      addresses, sizes, and calling conventions. Analysis results are cached \
                      per artifact SHA256 for reuse by ghidra.decompile."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::PrivateLoopback,
            timeout_ms: 300_000, // 5 minutes — analysis can be slow
            max_output_bytes: 64 * 1024 * 1024,
            max_produced_artifacts: 2,
            allow_exec: true,
            writable_bind_mounts: vec![cache_dir.to_path_buf()],
            extra_ro_bind_mounts: ro_mounts,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn ghidra_decompile_spec(ghidra_home: &Path, cache_dir: &Path, scripts_dir: &Path, java_home: Option<&Path>) -> ToolSpec {
    let mut ro_mounts = vec![ghidra_home.to_path_buf(), scripts_dir.to_path_buf()];
    if let Some(jh) = java_home {
        ro_mounts.push(jh.to_path_buf());
    }
    ToolSpec {
        name: "ghidra.decompile".to_string(),
        version: 1,
        deprecated: false,
        description: "Decompile one or more functions to C pseudocode using Ghidra. \
                      Requires ghidra.analyze to have been run first. Functions can be \
                      specified by name (e.g. 'main', 'FUN_00401000') or address (e.g. '0x00401000'). \
                      You must specify which functions to decompile."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "functions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": 20,
                    "description": "Function names or addresses (e.g. 'main', 'FUN_00401000', '0x00401000')."
                }
            },
            "required": ["artifact_id", "functions"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::PrivateLoopback,
            timeout_ms: 180_000, // 3 minutes
            max_output_bytes: 32 * 1024 * 1024,
            max_produced_artifacts: 4,
            allow_exec: true,
            writable_bind_mounts: vec![cache_dir.to_path_buf()],
            extra_ro_bind_mounts: ro_mounts,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn all_specs() -> Vec<ToolSpec> {
    vec![
        rizin_bininfo_spec(),
        rizin_disasm_spec(),
        rizin_xrefs_spec(),
        strings_extract_spec(),
    ]
}

/// ghidra.rename spec — DB-only (Trusted, no JVM needed).
pub fn ghidra_rename_spec() -> ToolSpec {
    ToolSpec {
        name: "ghidra.rename".to_string(),
        version: 1,
        deprecated: false,
        description: "Rename functions so subsequent ghidra.decompile calls show meaningful names. \
                      Renames are stored in the database per-project and applied as overlay during \
                      decompilation — the shared Ghidra analysis cache is never modified. Functions \
                      can be specified by name (e.g. 'FUN_00401000') or address (e.g. '0x00401000')."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "renames": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old": {
                                "type": "string",
                                "description": "Current function name or address (e.g. 'FUN_00401000', '0x00401000')"
                            },
                            "new": {
                                "type": "string",
                                "description": "New function name"
                            },
                            "address": {
                                "type": "string",
                                "description": "Optional function address for disambiguation (e.g. '0x00401000')"
                            }
                        },
                        "required": ["old", "new"]
                    },
                    "minItems": 1,
                    "maxItems": 50,
                    "description": "Array of {old, new, address?} rename pairs"
                }
            },
            "required": ["artifact_id", "renames"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            max_output_bytes: 64 * 1024,
            max_produced_artifacts: 0,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

/// ghidra.suggest_renames spec — DB-only (Trusted), surfaces renames from other projects.
pub fn ghidra_suggest_renames_spec() -> ToolSpec {
    ToolSpec {
        name: "ghidra.suggest_renames".to_string(),
        version: 1,
        deprecated: false,
        description: "Suggest function renames from other projects that analyzed the same binary. \
                      Returns rename suggestions discovered from non-NDA projects sharing the same \
                      SHA256. Use ghidra.rename to apply desired suggestions."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            max_output_bytes: 64 * 1024,
            max_produced_artifacts: 0,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn ghidra_specs(ghidra_home: &Path, cache_dir: &Path, scripts_dir: &Path, java_home: Option<&Path>) -> Vec<ToolSpec> {
    vec![
        ghidra_analyze_spec(ghidra_home, cache_dir, scripts_dir, java_home),
        ghidra_decompile_spec(ghidra_home, cache_dir, scripts_dir, java_home),
        ghidra_rename_spec(),
        ghidra_suggest_renames_spec(),
    ]
}

pub fn ioc_list_spec() -> ToolSpec {
    ToolSpec {
        name: "re-ioc.list".to_string(),
        version: 1,
        deprecated: false,
        description: "List all IOCs extracted from a project, optionally filtered by type. \
                      Returns indicators of compromise (IPs, domains, hashes, etc.) found during analysis."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "ioc_type": {
                    "type": "string",
                    "enum": ["ipv4", "ipv6", "domain", "url", "md5", "sha1", "sha256", "email", "mutex", "registry_key", "all"],
                    "default": "all",
                    "description": "IOC type to filter by, or 'all' for all types"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 500,
                    "default": 100,
                    "description": "Maximum number of IOCs to return"
                }
            }
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn ioc_pivot_spec() -> ToolSpec {
    ToolSpec {
        name: "re-ioc.pivot".to_string(),
        version: 1,
        deprecated: false,
        description: "Pivot on an IOC value — find all artifacts and tool runs that share a \
                      given indicator (IP, domain, hash, etc.) across the project."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "value": {
                    "type": "string",
                    "description": "IOC value to pivot on (IP, domain, hash, etc.)"
                },
                "ioc_type": {
                    "type": "string",
                    "enum": ["ipv4", "ipv6", "domain", "url", "md5", "sha1", "sha256", "email", "mutex", "registry_key", "all"],
                    "default": "all",
                    "description": "Restrict pivot to a specific IOC type, or 'all' for any type"
                }
            },
            "required": ["value"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn ioc_search_spec() -> ToolSpec {
    ToolSpec {
        name: "re-ioc.search".to_string(),
        version: 1,
        deprecated: false,
        description: "Search for an IOC value across all accessible projects. Finds matching \
                      indicators in other projects to correlate campaigns. NDA and opted-out \
                      projects are excluded from results."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "value": {
                    "type": "string",
                    "description": "IOC value to search for (IP, domain, hash, etc.)"
                },
                "ioc_type": {
                    "type": "string",
                    "enum": ["ipv4", "ipv6", "domain", "url", "md5", "sha1", "sha256", "email", "mutex", "registry_key", "all"],
                    "default": "all",
                    "description": "Restrict search to a specific IOC type, or 'all' for any type"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "Maximum number of results to return"
                }
            },
            "required": ["value"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 15_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn ioc_specs() -> Vec<ToolSpec> {
    vec![ioc_list_spec(), ioc_pivot_spec(), ioc_search_spec()]
}

pub fn artifact_describe_spec() -> ToolSpec {
    ToolSpec {
        name: "artifact.describe".to_string(),
        version: 1,
        deprecated: false,
        description: "Set a human-readable description on an artifact. Use this to annotate \
                      artifacts with findings (e.g. file type, packer, purpose) so later \
                      analysis can leverage prior context."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "description": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 1000,
                    "description": "Description text for the artifact (max 1000 chars)"
                }
            },
            "required": ["artifact_id", "description"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn artifact_search_spec() -> ToolSpec {
    ToolSpec {
        name: "artifact.search".to_string(),
        version: 1,
        deprecated: false,
        description: "Search for artifacts across all accessible projects by filename, \
                      description, SHA256 hash, or MIME type. Uses substring matching (ILIKE). \
                      NDA and opted-out projects are excluded from results."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 200,
                    "description": "Search term (matched as substring against the selected field)"
                },
                "field": {
                    "type": "string",
                    "enum": ["any", "filename", "description", "sha256", "mime_type"],
                    "default": "any",
                    "description": "Restrict search to a specific field, or 'any' to search all fields"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "Maximum number of results to return"
                }
            },
            "required": ["query"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn artifact_specs() -> Vec<ToolSpec> {
    vec![artifact_describe_spec(), artifact_search_spec()]
}

pub fn family_tag_spec() -> ToolSpec {
    ToolSpec {
        name: "family.tag".to_string(),
        version: 1,
        deprecated: false,
        description: "Tag an artifact with a malware family name (e.g. 'Emotet', 'TrickBot'). \
                      Upserts on conflict — if the artifact already has this family tag, \
                      confidence and notes are updated."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "family_name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 100,
                    "description": "Malware family name (e.g. 'emotet', 'cobalt strike')"
                },
                "confidence": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "confirmed"],
                    "default": "medium",
                    "description": "Confidence level of the attribution"
                },
                "notes": {
                    "type": "string",
                    "maxLength": 1000,
                    "description": "Optional notes about the attribution (max 1000 chars)"
                }
            },
            "required": ["artifact_id", "family_name"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn family_list_spec() -> ToolSpec {
    ToolSpec {
        name: "family.list".to_string(),
        version: 1,
        deprecated: false,
        description: "List malware family tags in the current project. Optionally filter by \
                      family name or artifact ID."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "family_name": {
                    "type": "string",
                    "description": "Filter by family name (exact match, case-insensitive)"
                },
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 500,
                    "default": 100,
                    "description": "Maximum number of results to return"
                }
            }
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn family_search_spec() -> ToolSpec {
    ToolSpec {
        name: "family.search".to_string(),
        version: 1,
        deprecated: false,
        description: "Search for artifacts tagged with a malware family name across all \
                      accessible projects. Returns artifact_id + project_id pairs."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "family_name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 100,
                    "description": "Malware family name to search for"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "Maximum number of results to return"
                }
            },
            "required": ["family_name"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn family_untag_spec() -> ToolSpec {
    ToolSpec {
        name: "family.untag".to_string(),
        version: 1,
        deprecated: false,
        description: "Remove a malware family tag from an artifact. Use to correct \
                      misattributions."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "family_name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 100,
                    "description": "Family name to remove"
                }
            },
            "required": ["artifact_id", "family_name"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn family_specs() -> Vec<ToolSpec> {
    vec![
        family_tag_spec(),
        family_list_spec(),
        family_search_spec(),
        family_untag_spec(),
    ]
}

// --- Dedup tools ---

pub fn dedup_prior_analysis_spec() -> ToolSpec {
    ToolSpec {
        name: "dedup.prior_analysis".to_string(),
        version: 1,
        deprecated: false,
        description: "Look up prior analysis of the same binary (by SHA256) in other accessible \
                      projects. Returns family tags, artifact descriptions, and thread counts \
                      from previous analyses. NDA projects are excluded."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "default": 5,
                    "description": "Max number of prior project analyses to return"
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 15_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn dedup_specs() -> Vec<ToolSpec> {
    vec![dedup_prior_analysis_spec()]
}

// --- YARA tools ---

pub fn yara_scan_spec(yara_rules_dir: Option<&Path>) -> ToolSpec {
    let mut policy = re_tool_policy();
    policy.max_produced_artifacts = 1;
    if let Some(dir) = yara_rules_dir {
        policy.extra_ro_bind_mounts.push(dir.to_path_buf());
    }
    ToolSpec {
        name: "yara.scan".to_string(),
        version: 1,
        deprecated: false,
        description: "Scan an artifact with YARA rules. Returns matching rule names, string \
                      matches with offsets, and match counts. Uses rules from the configured \
                      rules directory."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "rules": {
                    "type": "string",
                    "description": "Rule set name (filename without extension), or 'all' to scan with all rules. Default: 'all'",
                    "default": "all"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 5,
                    "maximum": 120,
                    "default": 30,
                    "description": "YARA scan timeout in seconds"
                }
            },
            "required": ["artifact_id"]
        }),
        policy,
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn yara_generate_spec() -> ToolSpec {
    let mut policy = re_tool_policy();
    policy.max_produced_artifacts = 1;
    ToolSpec {
        name: "yara.generate".to_string(),
        version: 1,
        deprecated: false,
        description: "Validate and store a YARA rule. Compiles the rule to check syntax, then \
                      saves it as a .yar artifact. The rule is also persisted in the DB for \
                      cross-project reuse."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "rule_text": {
                    "type": "string",
                    "minLength": 10,
                    "maxLength": 65536,
                    "description": "YARA rule source code"
                },
                "rule_name": {
                    "type": "string",
                    "maxLength": 200,
                    "description": "Override rule name (auto-extracted from source if omitted)"
                },
                "description": {
                    "type": "string",
                    "maxLength": 1000,
                    "description": "Human-readable description of what the rule detects"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string", "maxLength": 50 },
                    "maxItems": 20,
                    "description": "Tags for categorization (e.g. 'malware', 'packer', 'dropper')"
                }
            },
            "required": ["rule_text"]
        }),
        policy,
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn yara_test_spec() -> ToolSpec {
    ToolSpec {
        name: "yara.test".to_string(),
        version: 1,
        deprecated: false,
        description: "Test a YARA rule against multiple artifacts. Supports three scopes: \
                      'project' (all uploaded samples), 'artifact' (single sample), or \
                      'artifact_type' (filter by MIME type). Returns a match matrix showing \
                      which artifacts matched."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["project", "artifact", "artifact_type"],
                    "description": "Test scope: 'project' = all uploaded samples, 'artifact' = single sample, 'artifact_type' = filter by MIME"
                },
                "rule_text": {
                    "type": "string",
                    "maxLength": 65536,
                    "description": "YARA rule source (provide this OR rule_artifact_id, not both)"
                },
                "rule_artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "artifact_id": {
                    "$ref": "#/$defs/ArtifactId",
                    "description": "Target artifact (required when scope='artifact')"
                },
                "mime_pattern": {
                    "type": "string",
                    "maxLength": 100,
                    "description": "MIME type pattern for filtering (e.g. 'application/x-elf'). Required when scope='artifact_type'"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "default": 50,
                    "description": "Maximum number of artifacts to test against"
                }
            },
            "required": ["scope"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 300_000,
            max_produced_artifacts: 1,
            allow_exec: true,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn yara_list_spec() -> ToolSpec {
    ToolSpec {
        name: "yara.list".to_string(),
        version: 1,
        deprecated: false,
        description: "List available YARA rules from all sources: filesystem rules directory, \
                      DB-stored rules (project + global), and .yar artifacts in the project."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "maxLength": 200,
                    "description": "Substring filter on rule name"
                },
                "source": {
                    "type": "string",
                    "enum": ["all", "filesystem", "db", "artifacts"],
                    "default": "all",
                    "description": "Restrict to a specific source"
                }
            }
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 10_000,
            allow_exec: false,
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn yara_specs(yara_rules_dir: Option<&Path>) -> Vec<ToolSpec> {
    vec![
        yara_scan_spec(yara_rules_dir),
        yara_generate_spec(),
        yara_test_spec(),
        yara_list_spec(),
    ]
}

// --- Transform tools ---

fn transform_tool_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::NoNetReadOnly,
        timeout_ms: 60_000,
        max_output_bytes: 64 * 1024 * 1024,
        max_produced_artifacts: 1,
        allow_exec: false,
        ..Default::default()
    }
}

pub fn transform_decode_spec() -> ToolSpec {
    ToolSpec {
        name: "transform.decode".to_string(),
        version: 1,
        deprecated: false,
        description: "Decode or decompress a data blob. Supports base64, base64url, hex, \
                      URL-encoding, XOR (with key), gzip, zlib, and bzip2. Returns decoded \
                      data as an artifact."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "encoding": {
                    "type": "string",
                    "enum": ["base64", "base64url", "hex", "url", "xor", "gzip", "zlib", "bzip2"],
                    "description": "Encoding/compression to reverse"
                },
                "key": {
                    "type": "string",
                    "description": "Hex-encoded XOR key (required when encoding=xor, max 256 bytes)"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Byte offset to start processing"
                },
                "length": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Max bytes to read from source"
                }
            },
            "required": ["artifact_id", "encoding"]
        }),
        policy: transform_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn transform_unpack_spec() -> ToolSpec {
    ToolSpec {
        name: "transform.unpack".to_string(),
        version: 1,
        deprecated: false,
        description: "Extract archive contents. Supports ZIP, tar, tar.gz, tar.bz2, and 7z. \
                      Returns each extracted file as an artifact."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "password": {
                    "type": "string",
                    "description": "Archive password (for encrypted ZIP archives)"
                },
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 500,
                    "default": 100,
                    "description": "Maximum number of files to extract"
                },
                "max_total_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 268435456,
                    "description": "Maximum total extraction size in bytes (default 256MB)"
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            max_produced_artifacts: 100,
            timeout_ms: 120_000,
            ..transform_tool_policy()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn transform_jq_spec() -> ToolSpec {
    ToolSpec {
        name: "transform.jq".to_string(),
        version: 1,
        deprecated: false,
        description: "Apply a jq expression to a JSON artifact. Useful for filtering, \
                      transforming, or extracting specific fields from JSON data."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "expression": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 4096,
                    "description": "jq filter expression (e.g. '.functions[] | select(.name | contains(\"crypt\"))')"
                },
                "raw_output": {
                    "type": "boolean",
                    "default": false,
                    "description": "Emit raw strings without JSON quoting"
                }
            },
            "required": ["artifact_id", "expression"]
        }),
        policy: transform_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn transform_csv_spec() -> ToolSpec {
    ToolSpec {
        name: "transform.csv".to_string(),
        version: 1,
        deprecated: false,
        description: "Parse, filter, or compute statistics on CSV data. 'parse' converts to \
                      JSON, 'filter' selects rows matching a regex, 'stats' shows column statistics."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "operation": {
                    "type": "string",
                    "enum": ["parse", "filter", "stats"],
                    "description": "Operation: parse (CSV→JSON), filter (rows matching regex), stats (column statistics)"
                },
                "filter_column": {
                    "type": "string",
                    "description": "Column name or 0-based index for filter operation"
                },
                "filter_pattern": {
                    "type": "string",
                    "description": "Regex pattern for filter operation"
                },
                "columns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column names or indices to select (omit for all columns)"
                },
                "delimiter": {
                    "type": "string",
                    "maxLength": 1,
                    "default": ",",
                    "description": "Field delimiter character"
                },
                "has_header": {
                    "type": "boolean",
                    "default": true,
                    "description": "Whether the first row is a header"
                }
            },
            "required": ["artifact_id", "operation"]
        }),
        policy: transform_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn transform_convert_spec() -> ToolSpec {
    ToolSpec {
        name: "transform.convert".to_string(),
        version: 1,
        deprecated: false,
        description: "Convert between structured data formats: JSON, YAML, TOML, XML."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "from_format": {
                    "type": "string",
                    "enum": ["json", "yaml", "toml", "xml"],
                    "description": "Source format"
                },
                "to_format": {
                    "type": "string",
                    "enum": ["json", "yaml", "toml", "xml"],
                    "description": "Target format"
                }
            },
            "required": ["artifact_id", "from_format", "to_format"]
        }),
        policy: transform_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn transform_regex_spec() -> ToolSpec {
    ToolSpec {
        name: "transform.regex".to_string(),
        version: 1,
        deprecated: false,
        description: "Extract patterns from text or binary artifacts using regex with named \
                      capture groups. Returns all matches as structured JSON."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "pattern": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 4096,
                    "description": "Rust regex pattern (supports named groups (?P<name>...))"
                },
                "mode": {
                    "type": "string",
                    "enum": ["all", "first"],
                    "default": "all",
                    "description": "Match mode: 'all' returns all matches, 'first' returns only the first"
                },
                "max_matches": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10000,
                    "default": 1000,
                    "description": "Maximum number of matches to return"
                }
            },
            "required": ["artifact_id", "pattern"]
        }),
        policy: transform_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn transform_specs() -> Vec<ToolSpec> {
    vec![
        transform_decode_spec(),
        transform_unpack_spec(),
        transform_jq_spec(),
        transform_csv_spec(),
        transform_convert_spec(),
        transform_regex_spec(),
    ]
}

// --- Document tools ---

fn doc_tool_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::NoNetReadOnly,
        timeout_ms: 120_000,
        max_output_bytes: 64 * 1024 * 1024,
        max_produced_artifacts: 1,
        allow_exec: false,
        ..Default::default()
    }
}

pub fn doc_parse_spec() -> ToolSpec {
    ToolSpec {
        name: "doc.parse".to_string(),
        version: 1,
        deprecated: false,
        description: "Extract readable text from a document artifact. Supports PDF, HTML, \
                      Markdown, DOCX, XLSX, EPUB, and plain text. Auto-detects format from \
                      magic bytes and extension."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown", "html", "pdf", "docx", "xlsx", "epub"],
                    "description": "Override auto-detection. If omitted, format is detected from magic bytes and extension."
                },
                "pages": {
                    "type": "string",
                    "description": "For PDF: page range like '1-10', '5', '1,3,7-9'. Default: all pages."
                }
            },
            "required": ["artifact_id"]
        }),
        policy: doc_tool_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn doc_chunk_spec() -> ToolSpec {
    ToolSpec {
        name: "doc.chunk".to_string(),
        version: 1,
        deprecated: false,
        description: "Split a text artifact into overlapping chunks suitable for embedding. \
                      Splits at paragraph boundaries, falls back to sentences/words. Output \
                      is a JSON array of chunks with index, offset, length, text, and label. \
                      The resulting chunks.json is auto-enqueued for background embedding."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "chunk_size": {
                    "type": "integer",
                    "minimum": 100,
                    "maximum": 10000,
                    "default": 1000,
                    "description": "Target chunk size in characters"
                },
                "chunk_overlap": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 200,
                    "description": "Overlap between consecutive chunks in characters (max: chunk_size/2)"
                },
                "label_prefix": {
                    "type": "string",
                    "maxLength": 200,
                    "description": "Prefix for chunk labels. Default: artifact filename stem."
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            timeout_ms: 60_000,
            ..doc_tool_policy()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn doc_ingest_spec() -> ToolSpec {
    ToolSpec {
        name: "doc.ingest".to_string(),
        version: 1,
        deprecated: false,
        description: "All-in-one: parse document → chunk → auto-enqueue for embedding. Produces \
                      parsed_text.txt and chunks.json artifacts. Chunks are automatically queued \
                      for background embedding (processed by `af tick`). Use embed.search to \
                      query after the next tick cycle, or call embed.batch directly for immediate embedding."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown", "html", "pdf", "docx", "xlsx", "epub"],
                    "description": "Override auto-detection. If omitted, format is detected from magic bytes and extension."
                },
                "pages": {
                    "type": "string",
                    "description": "For PDF: page range like '1-10', '5', '1,3,7-9'. Default: all pages."
                },
                "chunk_size": {
                    "type": "integer",
                    "minimum": 100,
                    "maximum": 10000,
                    "default": 1000,
                    "description": "Target chunk size in characters"
                },
                "chunk_overlap": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 200,
                    "description": "Overlap between consecutive chunks in characters (max: chunk_size/2)"
                },
                "label_prefix": {
                    "type": "string",
                    "maxLength": 200,
                    "description": "Prefix for chunk labels. Default: artifact filename stem."
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            max_produced_artifacts: 2,
            ..doc_tool_policy()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn doc_specs() -> Vec<ToolSpec> {
    vec![doc_parse_spec(), doc_chunk_spec(), doc_ingest_spec()]
}

#[cfg(test)]
mod yara_spec_tests {
    use super::*;

    #[test]
    fn test_yara_specs_count() {
        let specs = yara_specs(None);
        assert_eq!(specs.len(), 4);
    }

    #[test]
    fn test_yara_specs_names() {
        let specs = yara_specs(None);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"yara.scan"));
        assert!(names.contains(&"yara.generate"));
        assert!(names.contains(&"yara.test"));
        assert!(names.contains(&"yara.list"));
    }

    #[test]
    fn test_yara_scan_spec_has_artifact_id() {
        let spec = yara_scan_spec(None);
        let schema_str = serde_json::to_string(&spec.input_schema).unwrap();
        assert!(schema_str.contains("ArtifactId"));
    }
}
