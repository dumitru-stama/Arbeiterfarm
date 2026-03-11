use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Acquire an exclusive file lock on the cache directory to prevent
/// concurrent Ghidra analyses of the same SHA256 from corrupting each other.
/// The lock is released when the returned `File` is dropped.
fn acquire_cache_lock(project_dir: &Path) -> Result<std::fs::File, std::io::Error> {
    std::fs::create_dir_all(project_dir)?;
    let lock_path = project_dir.join(".analysis.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(file)
}

pub struct GhidraAnalyzeExecutor {
    pub ghidra_home: PathBuf,
    pub cache_dir: PathBuf,
    pub scripts_dir: PathBuf,
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
impl ToolExecutor for GhidraAnalyzeExecutor {
    fn tool_name(&self) -> &str {
        "ghidra.analyze"
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

        let analyze_headless = self.ghidra_home.join("support").join("analyzeHeadless");
        let is_nda = ctx.tool_config.get("nda")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let project_dir = crate::common::ghidra_cache_path(
            &self.cache_dir,
            &ctx.project_id.to_string(),
            &artifact.sha256,
            is_nda,
        );
        let project_name = "analysis";

        // Acquire exclusive lock to prevent concurrent analyses of the same SHA256
        let _lock = acquire_cache_lock(&project_dir).map_err(|e| {
            tool_err("lock_error", format!("failed to acquire analysis lock: {e}"))
        })?;

        // 1. Check if cached Ghidra project exists for this SHA256.
        //    Ghidra 11.4+ creates .gpr as a 0-byte marker file, so we check for the
        //    actual analysis database (analysis.rep/idata/) instead of .gpr file size.
        //    Re-check AFTER acquiring the lock (another process may have completed analysis)
        let gpr_path = project_dir.join(format!("{project_name}.gpr"));
        let rep_idata = project_dir.join(format!("{project_name}.rep/idata"));
        let needs_analysis = !gpr_path.exists() || !rep_idata.is_dir();
        if needs_analysis && gpr_path.exists() {
            eprintln!("[ghidra-analyze] removing incomplete cache at {} (gpr exists but no analysis data)", project_dir.display());
            // Remove .gpr and .rep but NOT the .analysis.lock (we hold the flock on it)
            let _ = std::fs::remove_file(&gpr_path);
            let rep_dir = project_dir.join(format!("{project_name}.rep"));
            let _ = std::fs::remove_dir_all(&rep_dir);
        }

        if needs_analysis {
            // Create project directory
            std::fs::create_dir_all(&project_dir).map_err(|e| {
                tool_err(
                    "io_error",
                    format!("failed to create project dir: {e}"),
                )
            })?;

            // Run headless analysis:
            //   analyzeHeadless <project_dir> <project_name>
            //     -import <artifact_path>
            //     -overwrite
            //     -analysisTimeoutPerFile 240
            let output = Command::new(&analyze_headless)
                .arg(&project_dir)
                .arg(project_name)
                .arg("-import")
                .arg(&artifact.storage_path)
                .arg("-overwrite")
                .arg("-analysisTimeoutPerFile")
                .arg("240")
                .output()
                .map_err(|e| {
                    tool_err("exec_failed", format!("failed to run analyzeHeadless: {e}"))
                })?;

            if !output.status.success() {
                // Ghidra logs to stdout, not stderr — include both
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let combined = if stderr.is_empty() {
                    stdout.chars().take(2000).collect::<String>()
                } else {
                    format!(
                        "stderr: {} | stdout: {}",
                        stderr.chars().take(1000).collect::<String>(),
                        stdout.chars().take(1000).collect::<String>()
                    )
                };
                // Clean up .gpr and .rep on failure — NOT .analysis.lock (we hold flock)
                let _ = std::fs::remove_file(&gpr_path);
                let rep_dir = project_dir.join(format!("{project_name}.rep"));
                let _ = std::fs::remove_dir_all(&rep_dir);
                return Err(tool_err(
                    "analysis_failed",
                    format!("Ghidra analysis failed (exit {}): {}", output.status, combined),
                ));
            }
        }

        // 2. Extract function list using ListFunctionsJSON.java script
        //    analyzeHeadless <project_dir> <project_name>
        //      -process <binary_name>
        //      -noanalysis
        //      -scriptPath <scripts_dir>
        //      -postScript ListFunctionsJSON.java <output_json_path>
        let functions_json_path = ctx.scratch_dir.join("functions.json");

        let output = Command::new(&analyze_headless)
            .arg(&project_dir)
            .arg(project_name)
            .arg("-process")
            .arg(artifact.storage_path.file_name().unwrap_or_default())
            .arg("-noanalysis")
            .arg("-scriptPath")
            .arg(&self.scripts_dir)
            .arg("-postScript")
            .arg("ListFunctionsJSON.java")
            .arg(&functions_json_path)
            .output()
            .map_err(|e| {
                tool_err(
                    "exec_failed",
                    format!("failed to run function list extraction: {e}"),
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = if stderr.is_empty() {
                stdout.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>()
            } else {
                format!(
                    "stderr: {} | stdout(tail): {}",
                    stderr.chars().take(1000).collect::<String>(),
                    stdout.chars().rev().take(1000).collect::<String>().chars().rev().collect::<String>()
                )
            };
            return Err(tool_err(
                "script_failed",
                format!(
                    "Function list extraction failed (exit {}): {}",
                    output.status, combined
                ),
            ));
        }

        let stderr_str = String::from_utf8_lossy(&output.stderr);

        // 3. Read script output — now a JSON object with program_info + functions
        let script_output: Value = if functions_json_path.exists() {
            let data = std::fs::read_to_string(&functions_json_path).map_err(|e| {
                tool_err("io_error", format!("failed to read functions.json: {e}"))
            })?;
            serde_json::from_str(&data).unwrap_or_default()
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

        // Build compact function index: just name + address for inline summary
        let fn_index: Vec<Value> = real_fns.iter().map(|f| {
            json!({
                "name": f.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                "address": f.get("address").and_then(|a| a.as_str()).unwrap_or("?"),
            })
        }).collect();

        // Always store full output (program_info + functions) as artifact
        let full_json =
            serde_json::to_vec_pretty(&script_output).map_err(|e| {
                tool_err("serialize_error", format!("failed to serialize: {e}"))
            })?;
        let artifact_id = ctx
            .output_store
            .store_with_description(
                "functions.json",
                &full_json,
                Some("application/json"),
                &format!(
                    "Ghidra analysis: {} code functions, {} imports. Program info includes entry points, sections, architecture.",
                    real_fns.len(), thunks.len()
                ),
            )
            .await?;

        let summary = json!({
            "program_info": program_info,
            "total_functions": total,
            "code_functions": real_fns.len(),
            "thunk_count": thunks.len(),
            "cached": !needs_analysis,
            "functions": fn_index,
            "imported_symbols": thunk_names,
            "full_list_artifact": artifact_id.to_string(),
            "hint": "Use file.grep or file.read_range on the artifact for full function details (size, calling convention, etc.)",
        });

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: summary,
            stdout: None,
            stderr: if stderr_str.is_empty() {
                None
            } else {
                Some(stderr_str.into_owned())
            },
            produced_artifacts: vec![artifact_id],
            primary_artifact: Some(artifact_id),
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}
