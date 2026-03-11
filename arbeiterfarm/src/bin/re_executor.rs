use af_builtin_tools::envelope::{
    HandshakeResponse, OopArtifact, OopEnvelope, OopResponse, OopResult, ProducedFile,
    SupportedTool,
};
use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Acquire an exclusive file lock on the cache directory to prevent
/// concurrent Ghidra analyses of the same SHA256 from corrupting each other.
fn acquire_cache_lock(project_dir: &Path) -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(project_dir);
    let lock_path = project_dir.join(".analysis.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            eprintln!("[ghidra-analyze] WARNING: failed to acquire lock: {}", std::io::Error::last_os_error());
            return None;
        }
    }
    Some(file)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--handshake") {
        let handshake = HandshakeResponse {
            protocol_version: 1,
            supported_tools: vec![
                SupportedTool { name: "rizin.bininfo".into(), version: 1 },
                SupportedTool { name: "rizin.disasm".into(), version: 1 },
                SupportedTool { name: "rizin.xrefs".into(), version: 1 },
                SupportedTool { name: "strings.extract".into(), version: 1 },
                SupportedTool { name: "ghidra.analyze".into(), version: 1 },
                SupportedTool { name: "ghidra.decompile".into(), version: 1 },
                SupportedTool { name: "vt.file_report".into(), version: 1 },
                SupportedTool { name: "yara.scan".into(), version: 1 },
                SupportedTool { name: "yara.generate".into(), version: 1 },
                SupportedTool { name: "transform.decode".into(), version: 1 },
                SupportedTool { name: "transform.unpack".into(), version: 1 },
                SupportedTool { name: "transform.jq".into(), version: 1 },
                SupportedTool { name: "transform.csv".into(), version: 1 },
                SupportedTool { name: "transform.convert".into(), version: 1 },
                SupportedTool { name: "transform.regex".into(), version: 1 },
                SupportedTool { name: "doc.parse".into(), version: 1 },
                SupportedTool { name: "doc.chunk".into(), version: 1 },
                SupportedTool { name: "doc.ingest".into(), version: 1 },
            ],
        };
        let json = serde_json::to_string_pretty(&handshake).unwrap();
        println!("{json}");
        return;
    }

    // Read envelope from stdin
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        write_response(&OopResponse {
            result: OopResult::Error {
                code: "stdin_error".into(),
                message: format!("failed to read stdin: {e}"),
                retryable: false,
            },
        });
        return;
    }

    let envelope: OopEnvelope = match serde_json::from_str(&input) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[re-executor] FAILED to parse envelope: {e}");
            eprintln!("[re-executor] raw stdin (first 2000 chars): {}", input.chars().take(2000).collect::<String>());
            write_response(&OopResponse {
                result: OopResult::Error {
                    code: "parse_error".into(),
                    message: format!("failed to parse envelope: {e}"),
                    retryable: false,
                },
            });
            return;
        }
    };

    eprintln!("[re-executor] tool={} artifacts_count={} input={}",
        envelope.tool_name, envelope.context.artifacts.len(),
        serde_json::to_string(&envelope.input).unwrap_or_else(|_| "<err>".into()));
    for art in &envelope.context.artifacts {
        eprintln!("[re-executor]   artifact: id={} sha256={} file={} path={}",
            art.id, art.sha256, art.filename, art.storage_path.display());
    }

    // Handle tools that don't require artifacts first
    if envelope.tool_name == "yara.generate" {
        let result = execute_yara_generate(&envelope.input, &envelope.context.extra, &envelope.context.scratch_dir);
        write_response(&OopResponse { result });
        return;
    }

    let artifact = match envelope.context.artifacts.first() {
        Some(a) => a,
        None => {
            write_response(&OopResponse {
                result: OopResult::Error {
                    code: "no_artifact".into(),
                    message: "no artifacts provided in context".into(),
                    retryable: false,
                },
            });
            return;
        }
    };

    let result = match envelope.tool_name.as_str() {
        "rizin.bininfo" => execute_rizin_bininfo(artifact, &envelope.context.extra, &envelope.context.scratch_dir),
        "rizin.disasm" => execute_rizin_disasm(artifact, &envelope.input, &envelope.context.extra, &envelope.context.scratch_dir),
        "rizin.xrefs" => execute_rizin_xrefs(artifact, &envelope.input, &envelope.context.extra, &envelope.context.scratch_dir),
        "strings.extract" => {
            execute_strings_extract(artifact, &envelope.input, &envelope.context.extra, &envelope.context.scratch_dir)
        }
        "ghidra.analyze" => execute_ghidra_analyze(
            artifact,
            &envelope.context.extra,
            &envelope.context.scratch_dir,
            &envelope.context.project_id.to_string(),
        ),
        "ghidra.decompile" => execute_ghidra_decompile(
            artifact,
            &envelope.input,
            &envelope.context.extra,
            &envelope.context.scratch_dir,
            &envelope.context.project_id.to_string(),
        ),
        // ghidra.rename is now in-process (DB-only), not dispatched via OOP
        "vt.file_report" => execute_vt_file_report(artifact, &envelope.context.extra, envelope.context.actor_user_id),
        "yara.scan" => execute_yara_scan(artifact, &envelope.input, &envelope.context.extra, &envelope.context.scratch_dir),
        "transform.decode" => af_re_tools::transform_decode::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "transform.unpack" => af_re_tools::transform_unpack::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "transform.jq" => af_re_tools::transform_jq::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "transform.csv" => af_re_tools::transform_csv::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "transform.convert" => af_re_tools::transform_convert::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "transform.regex" => af_re_tools::transform_regex::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "doc.parse" => af_re_tools::doc_parse::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "doc.chunk" => af_re_tools::doc_chunk::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        "doc.ingest" => af_re_tools::doc_ingest::execute(artifact, &envelope.input, &envelope.context.scratch_dir),
        other => OopResult::Error {
            code: "unknown_tool".into(),
            message: format!("unknown tool: {other}"),
            retryable: false,
        },
    };

    write_response(&OopResponse { result });
}

fn write_response(resp: &OopResponse) {
    let json = serde_json::to_string(resp).unwrap();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(json.as_bytes());
    let _ = handle.write_all(b"\n");
    let _ = handle.flush();
}

fn oop_err(code: &str, message: String) -> OopResult {
    OopResult::Error {
        code: code.into(),
        message,
        retryable: false,
    }
}

fn rizin_path_from_extra(extra: &Value) -> PathBuf {
    PathBuf::from(
        extra["rizin_path"]
            .as_str()
            .unwrap_or("/usr/bin/rizin"),
    )
}

// --- Rizin tools ---

fn execute_rizin_bininfo(artifact: &OopArtifact, extra: &Value, scratch_dir: &Path) -> OopResult {
    let rizin = rizin_path_from_extra(extra);

    let output = match Command::new(&rizin)
        .arg("-q")
        .arg("-c")
        .arg("iIj;iij;iEj;iSj;iej")
        .arg(&artifact.storage_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run rizin: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return oop_err(
            "rizin_error",
            format!("rizin exited with {}: {stderr}", output.status),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_result = af_re_tools::rizin_bininfo::parse_bininfo_output(&stdout);

    // Write full output as artifact
    let bininfo_path = scratch_dir.join("bininfo.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&bininfo_path, serde_json::to_vec_pretty(&full_result).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "bininfo.json".into(),
                path: PathBuf::from("bininfo.json"),
                mime_type: Some("application/json".into()),
                description: Some("Full rizin binary info: imports, exports, sections, entrypoints, security flags.".into()),
            });
        }
        Err(e) => {
            eprintln!("[rizin-bininfo] WARNING: failed to write bininfo.json: {e}");
        }
    }

    // Return compact summary
    let summary = af_re_tools::rizin_bininfo::build_summary(&full_result);

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

fn execute_rizin_disasm(artifact: &OopArtifact, input: &Value, extra: &Value, scratch_dir: &Path) -> OopResult {
    let rizin = rizin_path_from_extra(extra);

    let address = match input["address"].as_str() {
        Some(a) => a,
        None => return oop_err("invalid_input", "address is required".into()),
    };
    if !af_re_tools::common::is_valid_hex_address(address) {
        return oop_err(
            "invalid_input",
            format!("invalid address format: {address} — expected 0x[0-9a-fA-F]+"),
        );
    }
    let length = match input["length"].as_u64() {
        Some(l) => l,
        None => return oop_err("invalid_input", "length is required".into()),
    };

    let cmd_str = format!("aa;pdj {} @ {}", length, address);

    let output = match Command::new(&rizin)
        .arg("-q")
        .arg("-c")
        .arg(&cmd_str)
        .arg(&artifact.storage_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run rizin: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return oop_err(
            "rizin_error",
            format!("rizin exited with {}: {stderr}", output.status),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_result = af_re_tools::common::parse_last_json(&stdout);

    // Store full disassembly as artifact
    let disasm_path = scratch_dir.join("disasm.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&disasm_path, serde_json::to_vec_pretty(&full_result).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "disasm.json".into(),
                path: PathBuf::from("disasm.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "Disassembly at {} ({} instructions requested)",
                    address, length
                )),
            });
        }
        Err(e) => {
            eprintln!("[rizin-disasm] WARNING: failed to write disasm.json: {e}");
        }
    }

    // Build compact summary with preview of first 10 instructions
    let instruction_count = full_result.as_array().map(|a| a.len()).unwrap_or(0);
    let preview: Vec<Value> = full_result.as_array()
        .map(|a| a.iter().take(10).cloned().collect())
        .unwrap_or_default();

    let summary = json!({
        "address": address,
        "instruction_count": instruction_count,
        "preview": preview,
        "hint": "Full disassembly stored as artifact. Use file.read_range to inspect details.",
    });

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

fn execute_rizin_xrefs(artifact: &OopArtifact, input: &Value, extra: &Value, scratch_dir: &Path) -> OopResult {
    let rizin = rizin_path_from_extra(extra);

    let address = match input["address"].as_str() {
        Some(a) => a,
        None => return oop_err("invalid_input", "address is required".into()),
    };
    if !af_re_tools::common::is_valid_hex_address(address) {
        return oop_err(
            "invalid_input",
            format!("invalid address format: {address} — expected 0x[0-9a-fA-F]+"),
        );
    }
    let direction = input
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("both");

    let xref_cmd = match direction {
        "to" => format!("axtj @ {address}"),
        "from" => format!("axfj @ {address}"),
        _ => format!("axtj @ {address};axfj @ {address}"),
    };
    let full_cmd = format!("aa;{xref_cmd}");

    let output = match Command::new(&rizin)
        .arg("-q")
        .arg("-c")
        .arg(&full_cmd)
        .arg(&artifact.storage_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run rizin: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return oop_err(
            "rizin_error",
            format!("rizin exited with {}: {stderr}", output.status),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_result = af_re_tools::rizin_xrefs::parse_xrefs_output(&stdout, direction);

    // Store full xrefs as artifact
    let xrefs_path = scratch_dir.join("xrefs.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&xrefs_path, serde_json::to_vec_pretty(&full_result).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "xrefs.json".into(),
                path: PathBuf::from("xrefs.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "Cross-references for {} (direction: {})",
                    address, direction
                )),
            });
        }
        Err(e) => {
            eprintln!("[rizin-xrefs] WARNING: failed to write xrefs.json: {e}");
        }
    }

    // Build compact summary with counts and top 10 entries
    let xrefs_to_count = full_result.get("xrefs_to")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let xrefs_from_count = full_result.get("xrefs_from")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let top_xrefs_to: Vec<Value> = full_result.get("xrefs_to")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().take(10).cloned().collect())
        .unwrap_or_default();
    let top_xrefs_from: Vec<Value> = full_result.get("xrefs_from")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().take(10).cloned().collect())
        .unwrap_or_default();

    let summary = json!({
        "address": address,
        "direction": direction,
        "xrefs_to_count": xrefs_to_count,
        "xrefs_from_count": xrefs_from_count,
        "top_xrefs_to": top_xrefs_to,
        "top_xrefs_from": top_xrefs_from,
        "hint": "Full cross-references stored as artifact. Use file.grep to search.",
    });

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

fn execute_strings_extract(artifact: &OopArtifact, input: &Value, extra: &Value, scratch_dir: &Path) -> OopResult {
    let rizin = rizin_path_from_extra(extra);

    let min_length = input
        .get("min_length")
        .and_then(|v| v.as_u64())
        .unwrap_or(4) as usize;
    let max_strings = input
        .get("max_strings")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;
    let encoding = input
        .get("encoding")
        .and_then(|v| v.as_str())
        .unwrap_or("all");

    let output = match Command::new(&rizin)
        .arg("-q")
        .arg("-c")
        .arg("izzj")
        .arg(&artifact.storage_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run rizin: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return oop_err(
            "rizin_error",
            format!("rizin exited with {}: {stderr}", output.status),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_result = af_re_tools::strings_extract::filter_strings(&stdout, min_length, max_strings, encoding);

    // Store full string list as artifact
    let strings_path = scratch_dir.join("strings.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&strings_path, serde_json::to_vec_pretty(&full_result).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "strings.json".into(),
                path: PathBuf::from("strings.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "Extracted strings (min_length={}, encoding={})",
                    min_length, encoding
                )),
            });
        }
        Err(e) => {
            eprintln!("[strings-extract] WARNING: failed to write strings.json: {e}");
        }
    }

    // Build compact summary with top 20 strings
    let total = full_result["total"].as_u64().unwrap_or(0);
    let truncated = full_result["truncated"].as_bool().unwrap_or(false);
    let top_strings: Vec<Value> = full_result.get("strings")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().take(20).cloned().collect())
        .unwrap_or_default();

    let summary = json!({
        "total_strings": total,
        "truncated": truncated,
        "top_strings": top_strings,
        "hint": "Full string list stored as artifact. Use file.grep to search.",
    });

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

// --- Ghidra tools ---

fn execute_ghidra_analyze(
    artifact: &OopArtifact,
    extra: &Value,
    scratch_dir: &Path,
    project_id: &str,
) -> OopResult {
    eprintln!("[ghidra-analyze] START artifact={} sha256={} project_id={}",
        artifact.filename, artifact.sha256, project_id);
    let ghidra_home = match extra["ghidra_home"].as_str() {
        Some(p) => PathBuf::from(p),
        None => return oop_err("config_error", "ghidra_home not set in context".into()),
    };
    let cache_dir = match extra["cache_dir"].as_str() {
        Some(p) => PathBuf::from(p),
        None => return oop_err("config_error", "cache_dir not set in context".into()),
    };
    let scripts_dir = match extra["scripts_dir"].as_str() {
        Some(p) => PathBuf::from(p),
        None => return oop_err("config_error", "scripts_dir not set in context".into()),
    };

    let is_nda = extra.get("nda").and_then(|v| v.as_bool()).unwrap_or(true);
    eprintln!("[ghidra-analyze] ghidra_home={} cache_dir={} scripts_dir={} is_nda={}",
        ghidra_home.display(), cache_dir.display(), scripts_dir.display(), is_nda);

    let analyze_headless = ghidra_home.join("support").join("analyzeHeadless");
    let project_dir = af_re_tools::common::ghidra_cache_path(&cache_dir, project_id, &artifact.sha256, is_nda);
    let project_name = "analysis";

    // Acquire exclusive lock to prevent concurrent analyses of the same SHA256
    let _lock = acquire_cache_lock(&project_dir);

    // 1. Check if cached project exists.
    //    Ghidra 11.4+ creates the .gpr as a 0-byte marker file, so we check for the
    //    actual analysis database (analysis.rep/idata/) instead of .gpr file size.
    //    Re-check AFTER acquiring the lock (another process may have completed analysis)
    let gpr_path = project_dir.join(format!("{project_name}.gpr"));
    let rep_idata = project_dir.join(format!("{project_name}.rep/idata"));
    let needs_analysis = !gpr_path.exists() || !rep_idata.is_dir();
    if needs_analysis && gpr_path.exists() {
        eprintln!("[ghidra-analyze] removing incomplete cache at {} (gpr exists but no analysis data)", project_dir.display());
        // Only remove .gpr and .rep — NOT .analysis.lock (we hold flock on it)
        let _ = std::fs::remove_file(&gpr_path);
        let rep_dir = project_dir.join(format!("{project_name}.rep"));
        let _ = std::fs::remove_dir_all(&rep_dir);
    }
    eprintln!("[ghidra-analyze] project_dir={} needs_analysis={}", project_dir.display(), needs_analysis);

    if needs_analysis {
        if let Err(e) = std::fs::create_dir_all(&project_dir) {
            return oop_err("io_error", format!("failed to create project dir: {e}"));
        }

        eprintln!("[ghidra-analyze] running: {} {} {} -import {} -overwrite -analysisTimeoutPerFile 240",
            analyze_headless.display(), project_dir.display(), project_name, artifact.storage_path.display());
        let output = match Command::new(&analyze_headless)
            .arg(&project_dir)
            .arg(project_name)
            .arg("-import")
            .arg(&artifact.storage_path)
            .arg("-overwrite")
            .arg("-analysisTimeoutPerFile")
            .arg("240")
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                eprintln!("[ghidra-analyze] FAILED to spawn analyzeHeadless: {e}");
                return oop_err("exec_failed", format!("failed to run analyzeHeadless: {e}"))
            }
        };

        eprintln!("[ghidra-analyze] analysis exit_code={}", output.status);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("[ghidra-analyze] analysis FAILED stderr: {}", stderr.chars().take(2000).collect::<String>());
            return oop_err(
                "analysis_failed",
                format!(
                    "Ghidra analysis failed (exit {}): {}",
                    output.status,
                    stderr.chars().take(2000).collect::<String>()
                ),
            );
        }
        eprintln!("[ghidra-analyze] analysis completed successfully");
    }

    // 2. Extract function list
    let functions_json_path = scratch_dir.join("functions.json");
    let process_name = artifact.storage_path.file_name().unwrap_or_default();
    eprintln!("[ghidra-analyze] extracting functions: {} {} {} -process {:?} -noanalysis -scriptPath {} -postScript ListFunctionsJSON.java {}",
        analyze_headless.display(), project_dir.display(), project_name,
        process_name, scripts_dir.display(), functions_json_path.display());

    let output = match Command::new(&analyze_headless)
        .arg(&project_dir)
        .arg(project_name)
        .arg("-process")
        .arg(process_name)
        .arg("-noanalysis")
        .arg("-scriptPath")
        .arg(&scripts_dir)
        .arg("-postScript")
        .arg("ListFunctionsJSON.java")
        .arg(&functions_json_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ghidra-analyze] FAILED to spawn function extraction: {e}");
            return oop_err(
                "exec_failed",
                format!("failed to run function list extraction: {e}"),
            )
        }
    };

    eprintln!("[ghidra-analyze] function extraction exit_code={}", output.status);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return oop_err(
            "script_failed",
            format!(
                "Function list extraction failed (exit {}): {}",
                output.status,
                stderr.chars().take(2000).collect::<String>()
            ),
        );
    }

    // 3. Read script output — now a JSON object with program_info + functions
    eprintln!("[ghidra-analyze] reading functions from {}", functions_json_path.display());
    let script_output: Value = if functions_json_path.exists() {
        match std::fs::read_to_string(&functions_json_path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(e) => {
                return oop_err("io_error", format!("failed to read functions.json: {e}"))
            }
        }
    } else {
        json!({})
    };

    let program_info = script_output.get("program_info").cloned().unwrap_or(json!({}));
    let functions: Vec<Value> = script_output
        .get("functions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let total = functions.len();

    // Split into real code functions and thunks (PLT/import stubs)
    let (real_fns, thunks): (Vec<&Value>, Vec<&Value>) = functions.iter().partition(|f| {
        !f.get("is_thunk").and_then(|v| v.as_bool()).unwrap_or(false)
    });
    let thunk_names: Vec<&str> = thunks.iter()
        .filter_map(|f| f.get("name").and_then(|n| n.as_str()))
        .collect();

    eprintln!("[ghidra-analyze] found {} functions ({} code, {} thunks)", total, real_fns.len(), thunks.len());

    // Build compact function index for inline summary (just name + address)
    let fn_index: Vec<Value> = real_fns.iter().map(|f| {
        json!({
            "name": f.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
            "address": f.get("address").and_then(|a| a.as_str()).unwrap_or("?"),
        })
    }).collect();

    // Always store full output (program_info + functions) as artifact
    let full_path = scratch_dir.join("functions.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&full_path, serde_json::to_vec_pretty(&script_output).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "functions.json".into(),
                path: PathBuf::from("functions.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "Ghidra function list: {} code functions, {} imports. Includes program_info with entry points, sections, architecture.",
                    real_fns.len(), thunks.len()
                )),
            });
        }
        Err(e) => {
            eprintln!("[ghidra-analyze] WARNING: failed to write functions.json: {e}");
        }
    }

    let mut summary = json!({
        "program_info": program_info,
        "total_functions": total,
        "code_functions": real_fns.len(),
        "thunk_count": thunks.len(),
        "cached": !needs_analysis,
        "functions": fn_index,
        "imported_symbols": thunk_names,
        "hint": "Full function details stored as artifact. Use file.grep or file.read_range to search.",
    });
    if !produced_files.is_empty() {
        summary["full_list_artifact"] = json!("(see produced artifact)");
    }

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

fn execute_ghidra_decompile(
    artifact: &OopArtifact,
    input: &Value,
    extra: &Value,
    scratch_dir: &Path,
    project_id: &str,
) -> OopResult {
    eprintln!("[ghidra-decompile] START artifact={} sha256={} input={}",
        artifact.filename, artifact.sha256,
        serde_json::to_string(input).unwrap_or_else(|_| "<err>".into()));
    let ghidra_home = match extra["ghidra_home"].as_str() {
        Some(p) => PathBuf::from(p),
        None => return oop_err("config_error", "ghidra_home not set in context".into()),
    };
    let cache_dir = match extra["cache_dir"].as_str() {
        Some(p) => PathBuf::from(p),
        None => return oop_err("config_error", "cache_dir not set in context".into()),
    };
    let scripts_dir = match extra["scripts_dir"].as_str() {
        Some(p) => PathBuf::from(p),
        None => return oop_err("config_error", "scripts_dir not set in context".into()),
    };

    let functions: Vec<&str> = match input["functions"].as_array() {
        Some(arr) => arr.iter().filter_map(|v| v.as_str()).collect(),
        None => return oop_err("invalid_input", "functions array is required".into()),
    };
    if functions.is_empty() {
        return oop_err("invalid_input", "functions array must not be empty".into());
    }

    // 1. Ensure analysis project exists.
    //    Ghidra 11.4+ creates .gpr as a 0-byte marker — check for analysis.rep/idata/ instead.
    let is_nda = extra.get("nda").and_then(|v| v.as_bool()).unwrap_or(true);
    let project_dir = af_re_tools::common::ghidra_cache_path(&cache_dir, project_id, &artifact.sha256, is_nda);
    eprintln!("[ghidra-decompile] project_dir={} functions={:?} is_nda={}", project_dir.display(), functions, is_nda);
    let gpr_path = project_dir.join("analysis.gpr");
    let rep_idata = project_dir.join("analysis.rep/idata");
    let project_valid = gpr_path.exists() && rep_idata.is_dir();
    if !project_valid {
        eprintln!("[ghidra-decompile] ERROR: no valid analysis project at {} (gpr_exists={}, rep_idata_exists={})",
            project_dir.display(), gpr_path.exists(), rep_idata.is_dir());
        return oop_err(
            "no_analysis",
            "Run ghidra.analyze first to create the analysis project".into(),
        );
    }

    // 2. Run decompile script
    let decompile_output_path = scratch_dir.join("decompiled.json");
    let func_args = functions.join(",");
    let analyze_headless = ghidra_home.join("support").join("analyzeHeadless");
    eprintln!("[ghidra-decompile] running: {} {} analysis -process {:?} -noanalysis -scriptPath {} -postScript DecompileFunctionsJSON.java {} {}",
        analyze_headless.display(), project_dir.display(),
        artifact.storage_path.file_name().unwrap_or_default(),
        scripts_dir.display(), func_args, decompile_output_path.display());

    let output = match Command::new(&analyze_headless)
        .arg(&project_dir)
        .arg("analysis")
        .arg("-process")
        .arg(artifact.storage_path.file_name().unwrap_or_default())
        .arg("-noanalysis")
        .arg("-scriptPath")
        .arg(&scripts_dir)
        .arg("-postScript")
        .arg("DecompileFunctionsJSON.java")
        .arg(&func_args)
        .arg(&decompile_output_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run decompilation: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return oop_err(
            "ghidra_error",
            format!(
                "Ghidra decompilation exited with {}: {}",
                output.status, stderr
            ),
        );
    }

    // 3. Read output
    let mut decompiled: Value = if decompile_output_path.exists() {
        match std::fs::read_to_string(&decompile_output_path) {
            Ok(data) => serde_json::from_str(&data)
                .unwrap_or(json!({"error": "failed to parse decompilation output"})),
            Err(e) => {
                return oop_err("io_error", format!("failed to read decompiled.json: {e}"))
            }
        }
    } else {
        json!({"error": "decompilation produced no output"})
    };

    // 3b. Apply renames overlay from database (injected via ToolConfigHook → extra)
    if let Some(renames_obj) = extra.get("ghidra_renames").and_then(|v| v.as_object()) {
        if !renames_obj.is_empty() {
            let renames: std::collections::HashMap<&str, &str> = renames_obj
                .iter()
                .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
                .collect();
            eprintln!("[ghidra-decompile] applying {} renames overlay", renames.len());
            af_re_tools::common::apply_renames_overlay(&mut decompiled, &renames);
        }
    }

    // 4. Always store full decompiled output as artifact and return compact summary
    let mut produced_files = Vec::new();
    match std::fs::write(&decompile_output_path, serde_json::to_vec_pretty(&decompiled).unwrap_or_default()) {
        Ok(()) => {
            // Build a description that includes function names (up to 5)
            let decompile_desc = {
                let max_names = 5;
                if functions.len() <= max_names {
                    format!("Ghidra decompiled: {}", functions.join(", "))
                } else {
                    let preview: Vec<&str> = functions.iter().take(max_names).copied().collect();
                    format!("Ghidra decompiled: {}, ... ({} total)", preview.join(", "), functions.len())
                }
            };
            produced_files.push(ProducedFile {
                filename: "decompiled.json".into(),
                path: PathBuf::from("decompiled.json"),
                mime_type: Some("application/json".into()),
                description: Some(decompile_desc),
            });
        }
        Err(e) => {
            eprintln!("[ghidra-decompile] WARNING: failed to write decompiled.json: {e}");
        }
    }

    // Build compact summary: function names, line counts, hint
    let fn_summaries: Vec<Value> = if let Some(arr) = decompiled.as_array() {
        arr.iter().map(|f| {
            let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let addr = f.get("address").and_then(|a| a.as_str()).unwrap_or("?");
            let line_count = f.get("code").and_then(|c| c.as_str())
                .map(|c| c.lines().count())
                .unwrap_or(0);
            json!({ "name": name, "address": addr, "line_count": line_count })
        }).collect()
    } else if let Some(obj) = decompiled.as_object() {
        // Single function or keyed by name
        obj.iter().map(|(key, f)| {
            let line_count = f.get("code").and_then(|c| c.as_str())
                .map(|c| c.lines().count())
                .unwrap_or(0);
            json!({ "name": key, "line_count": line_count })
        }).collect()
    } else {
        vec![]
    };

    let summary = json!({
        "functions_decompiled": functions.len(),
        "functions": fn_summaries,
        "hint": "Full decompiled C pseudocode stored as artifact. Use file.read_range or file.grep to inspect.",
    });

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

// --- VT tools ---

fn execute_vt_file_report(artifact: &OopArtifact, extra: &Value, actor_user_id: Option<uuid::Uuid>) -> OopResult {
    let gateway_socket = match extra["gateway_socket"].as_str() {
        Some(p) => p,
        None => return oop_err("config_error", "gateway_socket not set in context".into()),
    };

    // Connect to gateway via UDS (blocking I/O — OOP executor is synchronous)
    let stream = match std::os::unix::net::UnixStream::connect(gateway_socket) {
        Ok(s) => s,
        Err(e) => {
            return OopResult::Error {
                code: "gateway_unavailable".into(),
                message: format!("VT gateway not running at {gateway_socket}: {e}"),
                retryable: true,
            }
        }
    };

    // Set a read/write timeout
    let timeout = std::time::Duration::from_secs(30);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut writer = std::io::BufWriter::new(&stream);
    let mut reader = std::io::BufReader::new(&stream);

    // Send request (include user_id for per-user rate limiting in the gateway)
    let request = json!({
        "action": "file_report",
        "sha256": &artifact.sha256,
        "user_id": actor_user_id.map(|u| u.to_string()),
    });
    let mut req_bytes = match serde_json::to_vec(&request) {
        Ok(b) => b,
        Err(e) => return oop_err("serialize_error", format!("failed to serialize request: {e}")),
    };
    req_bytes.push(b'\n');

    use std::io::Write as _;
    if let Err(e) = writer.write_all(&req_bytes) {
        return OopResult::Error {
            code: "io_error".into(),
            message: format!("failed to write to gateway: {e}"),
            retryable: true,
        };
    }
    if let Err(e) = writer.flush() {
        return OopResult::Error {
            code: "io_error".into(),
            message: format!("failed to flush to gateway: {e}"),
            retryable: true,
        };
    }
    // Shutdown write half to signal end of request
    let _ = stream.shutdown(std::net::Shutdown::Write);

    // Read response
    use std::io::BufRead as _;
    let mut response_line = String::new();
    if let Err(e) = reader.read_line(&mut response_line) {
        return OopResult::Error {
            code: "io_error".into(),
            message: format!("failed to read gateway response: {e}"),
            retryable: true,
        };
    }

    let resp: Value = match serde_json::from_str(&response_line) {
        Ok(v) => v,
        Err(e) => return oop_err("parse_error", format!("failed to parse gateway response: {e}")),
    };

    // Check response
    let ok = resp["ok"].as_bool().unwrap_or(false);
    if !ok {
        let error = resp["error"].as_str().unwrap_or("unknown");
        let message = resp["message"].as_str().unwrap_or("gateway error");
        let retryable = error == "rate_limited" || error == "upstream_error";
        return OopResult::Error {
            code: error.into(),
            message: message.into(),
            retryable,
        };
    }

    // Build result
    let data = resp.get("data").cloned().unwrap_or(Value::Null);
    let cached = resp["cached"].as_bool().unwrap_or(false);

    let output = if data.is_null() {
        json!({
            "found": false,
            "message": resp["message"].as_str().unwrap_or("Hash not found in VirusTotal database"),
            "sha256": &artifact.sha256,
        })
    } else {
        json!({
            "found": true,
            "cached": cached,
            "report": data,
        })
    };

    OopResult::Ok {
        output,
        produced_files: vec![],
    }
}

// --- YARA tools ---

fn yara_path_from_extra(extra: &Value) -> PathBuf {
    PathBuf::from(extra["yara_path"].as_str().unwrap_or("/usr/bin/yara"))
}

fn execute_yara_scan(artifact: &OopArtifact, input: &Value, extra: &Value, scratch_dir: &Path) -> OopResult {
    let yara = yara_path_from_extra(extra);
    let timeout = input
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(30);
    let rules_filter = input
        .get("rules")
        .and_then(|v| v.as_str())
        .unwrap_or("all");

    // Collect rule files
    let rules_dir = extra["yara_rules_dir"].as_str().map(std::path::Path::new);
    let rule_files = af_re_tools::yara_scan::collect_rule_files(rules_dir);

    if rule_files.is_empty() {
        return oop_err("no_rules", "no YARA rule files found in rules directory".into());
    }

    // Filter to specific rule set if requested
    let rule_files: Vec<PathBuf> = if rules_filter == "all" {
        rule_files
    } else {
        rule_files
            .into_iter()
            .filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map_or(false, |s| s == rules_filter)
            })
            .collect()
    };

    if rule_files.is_empty() {
        return oop_err(
            "no_rules",
            format!("no YARA rule file matching '{rules_filter}' found"),
        );
    }

    // Build yara command: yara -s --timeout=N <rule_files...> <artifact>
    let mut cmd = Command::new(&yara);
    cmd.arg("-s")
        .arg(format!("--timeout={timeout}"));

    for rule_file in &rule_files {
        cmd.arg(rule_file);
    }
    cmd.arg(&artifact.storage_path);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run yara: {e}")),
    };

    // YARA exit 0 = matches found or no matches, exit 1 = error
    // (yara returns exit 0 even with no matches when -s is used)
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && !stderr.is_empty() {
        eprintln!("[yara-scan] WARNING: yara stderr: {}", stderr.chars().take(1000).collect::<String>());
    }

    let full_result = af_re_tools::yara_scan::parse_yara_output(&stdout);

    // Add artifact_id to output for the post-tool hook
    let mut full_with_id = full_result.clone();
    if let Some(obj) = full_with_id.as_object_mut() {
        obj.insert("artifact_id".to_string(), json!(artifact.id.to_string()));
    }

    // Store full results as artifact
    let scan_path = scratch_dir.join("yara_matches.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&scan_path, serde_json::to_vec_pretty(&full_with_id).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "yara_matches.json".into(),
                path: PathBuf::from("yara_matches.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "YARA scan results: {} rules matched",
                    full_result["total_rules_matched"].as_u64().unwrap_or(0),
                )),
            });
        }
        Err(e) => {
            eprintln!("[yara-scan] WARNING: failed to write yara_matches.json: {e}");
        }
    }

    let mut summary = af_re_tools::yara_scan::build_scan_summary(&full_result);
    // Include artifact_id and full rules array for the post-tool hook to persist scan results.
    // The summary alone only has rule_names (strings) — the hook needs the structured rules array.
    if let Some(obj) = summary.as_object_mut() {
        obj.insert("artifact_id".to_string(), json!(artifact.id.to_string()));
        if let Some(rules) = full_result.get("rules") {
            obj.insert("rules".to_string(), rules.clone());
        }
    }

    OopResult::Ok {
        output: summary,
        produced_files,
    }
}

fn execute_yara_generate(input: &Value, extra: &Value, scratch_dir: &Path) -> OopResult {
    let yara = yara_path_from_extra(extra);

    let rule_text = match input.get("rule_text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return oop_err("invalid_input", "'rule_text' is required".into()),
    };

    // Basic structural validation
    if let Err(e) = af_re_tools::yara_generate::validate_rule_text(rule_text) {
        return OopResult::Ok {
            output: json!({
                "valid": false,
                "error": e,
            }),
            produced_files: vec![],
        };
    }

    // Extract or override rule name
    let rule_name = input
        .get("rule_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| af_re_tools::yara_generate::extract_rule_name(rule_text))
        .unwrap_or_else(|| "unnamed_rule".to_string());

    // Sanitize
    if let Err(e) = af_re_tools::yara_scan::sanitize_rule_name(&rule_name) {
        return OopResult::Ok {
            output: json!({
                "valid": false,
                "error": e,
            }),
            produced_files: vec![],
        };
    }

    // Write rule to temp file for compilation check
    let temp_rule = scratch_dir.join("check.yar");
    if let Err(e) = std::fs::write(&temp_rule, rule_text) {
        return oop_err("io_error", format!("failed to write temp rule: {e}"));
    }

    // Compile check: yara <temp_rule> /dev/null
    let output = match Command::new(&yara)
        .arg(&temp_rule)
        .arg("/dev/null")
        .output()
    {
        Ok(o) => o,
        Err(e) => return oop_err("exec_failed", format!("failed to run yara: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return OopResult::Ok {
            output: json!({
                "valid": false,
                "error": format!("YARA compilation failed: {}", stderr.trim()),
                "rule_name": rule_name,
            }),
            produced_files: vec![],
        };
    }

    // Valid rule — save as artifact
    let filename = format!("{rule_name}.yar");
    let rule_path = scratch_dir.join(&filename);
    if let Err(e) = std::fs::write(&rule_path, rule_text) {
        return oop_err("io_error", format!("failed to write rule file: {e}"));
    }

    let description = input
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tags: Vec<String> = input
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    OopResult::Ok {
        output: json!({
            "valid": true,
            "rule_name": rule_name,
            "rule_text": rule_text,
            "filename": filename,
            "description": description,
            "tags": tags,
        }),
        produced_files: vec![ProducedFile {
            filename,
            path: PathBuf::from(format!("{rule_name}.yar")),
            mime_type: Some("text/x-yara".into()),
            description: Some(format!("YARA rule: {rule_name}")),
        }],
    }
}
