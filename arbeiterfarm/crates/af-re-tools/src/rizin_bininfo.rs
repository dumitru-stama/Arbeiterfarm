use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

pub struct RizinBinInfoExecutor {
    pub rizin_path: PathBuf,
}

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

#[async_trait]
impl ToolExecutor for RizinBinInfoExecutor {
    fn tool_name(&self) -> &str {
        "rizin.bininfo"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        _input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact = ctx
            .artifacts
            .first()
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into()))?;

        // rizin -q -c "iIj;iij;iEj;iSj;iej" <file>
        //   iIj = binary info (JSON)
        //   iij = imports (JSON)
        //   iEj = exports (JSON)
        //   iSj = sections (JSON)
        //   iej = entrypoints (JSON)
        let output = Command::new(&self.rizin_path)
            .arg("-q")
            .arg("-c")
            .arg("iIj;iij;iEj;iSj;iej")
            .arg(&artifact.storage_path)
            .output()
            .map_err(|e| tool_err("exec_failed", format!("failed to run rizin: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(tool_err(
                "rizin_error",
                format!("rizin exited with {}: {stderr}", output.status),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result_json = parse_bininfo_output(&stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: result_json,
            stdout: Some(stdout.into_owned()),
            stderr: if stderr_str.is_empty() {
                None
            } else {
                Some(stderr_str.into_owned())
            },
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}

/// Parse the concatenated JSON output from rizin commands.
/// Each command outputs one line of JSON. We parse them in order:
/// iIj → imports → exports → sections → entrypoints.
///
/// Non-JSON lines (rizin warnings, prompts) are silently skipped.
/// This makes parsing robust against rizin version differences.
pub fn parse_bininfo_output(stdout: &str) -> Value {
    let mut info = Value::Null;
    let mut imports = Value::Null;
    let mut exports = Value::Null;
    let mut sections = Value::Null;
    let mut entrypoints = Value::Null;
    let mut warnings = Vec::new();

    let mut json_idx = 0;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Only attempt JSON parse on lines starting with { or [
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                match json_idx {
                    0 => info = parsed,
                    1 => imports = parsed,
                    2 => exports = parsed,
                    3 => sections = parsed,
                    4 => entrypoints = parsed,
                    _ => {}
                }
                json_idx += 1;
            } else {
                warnings.push(trimmed.to_string());
            }
        }
        // Non-JSON lines (rizin warnings/prompts) are silently skipped
    }

    let mut result = json!({
        "info": info,
        "imports": imports,
        "exports": exports,
        "sections": sections,
        "entrypoints": entrypoints,
    });

    if !warnings.is_empty() {
        result["rizin_warnings"] = json!(warnings);
    }

    result
}

/// Build a compact summary (~500 bytes) from the full parsed bininfo output.
/// Extracts architecture, format, security flags, entry point, counts,
/// and a curated list of the most interesting imports/exports.
pub fn build_summary(full: &Value) -> Value {
    let info = &full["info"];

    // Architecture string: "arch:endian:bits"
    let arch = info["arch"].as_str().unwrap_or("unknown");
    let bits = info["bits"].as_u64().unwrap_or(0);
    let endian = info["endian"].as_str().unwrap_or("?");
    let endian_short = if endian.starts_with("LE") || endian == "little" {
        "LE"
    } else if endian.starts_with("BE") || endian == "big" {
        "BE"
    } else {
        endian
    };
    let architecture = format!("{arch}:{endian_short}:{bits}");

    let format = info["bintype"].as_str()
        .or_else(|| info["fmt"].as_str())
        .or_else(|| info["class"].as_str())
        .unwrap_or("unknown");
    let os = info["os"].as_str().unwrap_or("unknown");
    let pic = info["pic"].as_bool().unwrap_or(false);
    let canary = info["canary"].as_bool().unwrap_or(false);
    let nx = info["nx"].as_bool().unwrap_or(false);
    let relro = info["relro"].as_str().unwrap_or("none");
    let stripped = info["stripped"].as_bool().unwrap_or(false);

    // Entry point from entrypoints array
    let entry_point = full["entrypoints"]
        .as_array()
        .and_then(|arr| {
            arr.iter().find_map(|e| {
                // Program entry type (skip "preinit", "init", "fini")
                let etype = e.get("type").and_then(|t| t.as_str()).unwrap_or("program");
                if etype == "program" || etype == "unknown" || arr.len() == 1 {
                    e.get("vaddr")
                        .and_then(|v| v.as_u64())
                        .map(|addr| format!("0x{:x}", addr))
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "unknown".into());

    // Imports: count + top 20 interesting names
    let imports_arr = full["imports"].as_array();
    let imports_count = imports_arr.map(|a| a.len()).unwrap_or(0);
    let key_imports: Vec<&str> = imports_arr
        .map(|arr| {
            arr.iter()
                .filter_map(|imp| imp["name"].as_str())
                // Filter out __libc_start_main, __cxa_atexit, etc.
                .filter(|name| !name.starts_with("__"))
                .take(20)
                .collect()
        })
        .unwrap_or_default();

    // Exports: count + top 10
    let exports_arr = full["exports"].as_array();
    let exports_count = exports_arr.map(|a| a.len()).unwrap_or(0);
    let key_exports: Vec<&str> = exports_arr
        .map(|arr| {
            arr.iter()
                .filter_map(|exp| exp["name"].as_str())
                .filter(|name| !name.starts_with("__"))
                .take(10)
                .collect()
        })
        .unwrap_or_default();

    // Sections count
    let sections_count = full["sections"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    json!({
        "architecture": architecture,
        "format": format,
        "os": os,
        "pic": pic,
        "canary": canary,
        "nx": nx,
        "relro": relro,
        "stripped": stripped,
        "entry_point": entry_point,
        "imports_count": imports_count,
        "exports_count": exports_count,
        "sections_count": sections_count,
        "key_imports": key_imports,
        "key_exports": key_exports,
        "hint": "Full binary info stored as artifact. Use file.grep or file.read_range to search.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bininfo_five_json_objects() {
        let stdout = r#"{"arch":"x86"}
[{"name":"printf"}]
[{"name":"main"}]
[{"name":".text","size":4096}]
[{"vaddr":4096}]
"#;
        let result = parse_bininfo_output(stdout);
        assert_eq!(result["info"]["arch"], "x86");
        assert!(result["imports"].is_array());
        assert!(result["exports"].is_array());
        assert!(result["sections"].is_array());
        assert!(result["entrypoints"].is_array());
    }

    #[test]
    fn test_parse_bininfo_skips_non_json() {
        let stdout = "WARNING: something\n{\"arch\":\"arm\"}\nsome noise\n[]\n[]\n[]\n[]\n";
        let result = parse_bininfo_output(stdout);
        assert_eq!(result["info"]["arch"], "arm");
    }

    #[test]
    fn test_parse_bininfo_empty_output() {
        let result = parse_bininfo_output("");
        assert!(result["info"].is_null());
        assert!(result["imports"].is_null());
    }

    #[test]
    fn test_parse_bininfo_partial_output() {
        // Only two valid JSON lines
        let stdout = "{\"arch\":\"mips\"}\n[{\"name\":\"puts\"}]\n";
        let result = parse_bininfo_output(stdout);
        assert_eq!(result["info"]["arch"], "mips");
        assert!(result["imports"].is_array());
        assert!(result["exports"].is_null()); // missing
    }

    #[test]
    fn test_build_summary_realistic() {
        let full = json!({
            "info": {
                "arch": "x86",
                "bits": 64,
                "endian": "LE",
                "bintype": "elf",
                "os": "linux",
                "pic": true,
                "canary": true,
                "nx": true,
                "relro": "full",
                "stripped": false,
            },
            "imports": [
                {"name": "krb5_init_context"},
                {"name": "krb5_mk_req_extended"},
                {"name": "printf"},
                {"name": "malloc"},
                {"name": "__libc_start_main"},
                {"name": "__cxa_atexit"},
            ],
            "exports": [
                {"name": "argp_parse"},
                {"name": "argp_help"},
                {"name": "__bss_start"},
            ],
            "sections": [
                {"name": ".text", "size": 4096},
                {"name": ".data", "size": 1024},
                {"name": ".rodata", "size": 512},
            ],
            "entrypoints": [
                {"vaddr": 0x8720, "type": "program"},
            ],
        });

        let summary = build_summary(&full);

        assert_eq!(summary["architecture"], "x86:LE:64");
        assert_eq!(summary["format"], "elf");
        assert_eq!(summary["os"], "linux");
        assert_eq!(summary["pic"], true);
        assert_eq!(summary["canary"], true);
        assert_eq!(summary["nx"], true);
        assert_eq!(summary["relro"], "full");
        assert_eq!(summary["stripped"], false);
        assert_eq!(summary["entry_point"], "0x8720");
        assert_eq!(summary["imports_count"], 6);
        assert_eq!(summary["exports_count"], 3);
        assert_eq!(summary["sections_count"], 3);

        // __libc_start_main and __cxa_atexit should be filtered out
        let key_imports = summary["key_imports"].as_array().unwrap();
        assert_eq!(key_imports.len(), 4);
        assert!(key_imports.iter().all(|v| !v.as_str().unwrap().starts_with("__")));

        // __bss_start should be filtered out
        let key_exports = summary["key_exports"].as_array().unwrap();
        assert_eq!(key_exports.len(), 2);
        assert!(key_exports.iter().all(|v| !v.as_str().unwrap().starts_with("__")));

        assert!(summary["hint"].as_str().unwrap().contains("artifact"));
    }

    #[test]
    fn test_build_summary_empty_input() {
        let full = json!({
            "info": Value::Null,
            "imports": Value::Null,
            "exports": Value::Null,
            "sections": Value::Null,
            "entrypoints": Value::Null,
        });

        let summary = build_summary(&full);
        assert_eq!(summary["architecture"], "unknown:?:0");
        assert_eq!(summary["imports_count"], 0);
        assert_eq!(summary["exports_count"], 0);
        assert_eq!(summary["sections_count"], 0);
        assert_eq!(summary["entry_point"], "unknown");
    }
}
