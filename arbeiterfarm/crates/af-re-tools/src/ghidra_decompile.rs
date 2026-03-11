use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Acquire a shared lock on the analysis lock file.
/// This blocks until any exclusive (write) lock from ghidra.analyze finishes.
fn acquire_shared_lock(project_dir: &Path) -> Result<std::fs::File, std::io::Error> {
    let lock_path = project_dir.join(".analysis.lock");
    if !lock_path.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no lock file"));
    }
    let file = std::fs::OpenOptions::new().read(true).open(&lock_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(file)
}

pub struct GhidraDecompileExecutor {
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
impl ToolExecutor for GhidraDecompileExecutor {
    fn tool_name(&self) -> &str {
        "ghidra.decompile"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        // functions is optional — defaults to ["entry"] in execute()
        if let Some(functions) = input.get("functions").and_then(|v| v.as_array()) {
            for f in functions {
                let name = f.as_str().ok_or("each function must be a string")?;
                if name.is_empty() {
                    return Err("function name cannot be empty".into());
                }
            }
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: ToolContext,
        input: Value,
    ) -> Result<ToolResult, ToolError> {
        let artifact = ctx
            .artifacts
            .first()
            .ok_or_else(|| tool_err("no_artifact", "no artifact provided".into()))?;

        let functions: Vec<&str> = input["functions"]
            .as_array()
            .ok_or_else(|| tool_err("invalid_input", "functions array is required".into()))?
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        if functions.is_empty() {
            return Err(tool_err("invalid_input", "functions array must not be empty".into()));
        }

        // 1. Ensure Ghidra project exists (analysis must have run first).
        //    Ghidra 11.4+ creates .gpr as a 0-byte marker — check for analysis.rep/idata/ instead.
        //    Acquire a shared lock so we wait for any in-progress analysis to finish.
        let is_nda = ctx.tool_config.get("nda")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let project_dir = crate::common::ghidra_cache_path(
            &self.cache_dir,
            &ctx.project_id.to_string(),
            &artifact.sha256,
            is_nda,
        );
        let _lock = acquire_shared_lock(&project_dir).ok();

        let gpr_path = project_dir.join("analysis.gpr");
        let rep_idata = project_dir.join("analysis.rep/idata");
        let project_valid = gpr_path.exists() && rep_idata.is_dir();
        if !project_valid {
            return Err(tool_err(
                "no_analysis",
                "Run ghidra.analyze first to create the analysis project".into(),
            ));
        }

        // 2. Run headless decompile script:
        //    analyzeHeadless <project_dir> analysis
        //      -process <binary_name>
        //      -noanalysis
        //      -scriptPath <scripts_dir>
        //      -postScript DecompileFunctionsJSON.java <func_args> <output_path>
        let decompile_output_path = ctx.scratch_dir.join("decompiled.json");
        let func_args = functions.join(",");

        let analyze_headless = self.ghidra_home.join("support").join("analyzeHeadless");

        let output = Command::new(&analyze_headless)
            .arg(&project_dir)
            .arg("analysis")
            .arg("-process")
            .arg(artifact.storage_path.file_name().unwrap_or_default())
            .arg("-noanalysis")
            .arg("-scriptPath")
            .arg(&self.scripts_dir)
            .arg("-postScript")
            .arg("DecompileFunctionsJSON.java")
            .arg(&func_args)
            .arg(&decompile_output_path)
            .output()
            .map_err(|e| {
                tool_err(
                    "exec_failed",
                    format!("failed to run decompilation: {e}"),
                )
            })?;

        let stderr_str = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(tool_err(
                "ghidra_error",
                format!(
                    "Ghidra decompilation exited with {}: {}",
                    output.status, stderr_str
                ),
            ));
        }

        // 3. Read decompiled output
        let mut decompiled: Value = if decompile_output_path.exists() {
            let data =
                std::fs::read_to_string(&decompile_output_path).map_err(|e| {
                    tool_err("io_error", format!("failed to read decompiled.json: {e}"))
                })?;
            serde_json::from_str(&data)
                .unwrap_or(json!({"error": "failed to parse decompilation output"}))
        } else {
            json!({"error": "decompilation produced no output"})
        };

        // 3b. Apply renames overlay from database (injected via ToolConfigHook)
        if let Some(renames_obj) = ctx.tool_config.get("ghidra_renames").and_then(|v| v.as_object()) {
            if !renames_obj.is_empty() {
                let renames: std::collections::HashMap<&str, &str> = renames_obj
                    .iter()
                    .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
                    .collect();
                crate::common::apply_renames_overlay(&mut decompiled, &renames);
            }
        }

        // 4. If large, store as artifact
        let decompiled_str = serde_json::to_string_pretty(&decompiled).map_err(|e| {
            tool_err("serialize_error", format!("failed to serialize: {e}"))
        })?;

        if decompiled_str.len() > 64 * 1024 {
            let artifact_id = ctx
                .output_store
                .store(
                    "decompiled.json",
                    decompiled_str.as_bytes(),
                    Some("application/json"),
                )
                .await?;

            let summary = json!({
                "functions_decompiled": functions.len(),
                "stored_as_artifact": artifact_id.to_string(),
                "hint": "Decompiled output is large. Use file.read_range to read the artifact."
            });

            return Ok(ToolResult {
                kind: ToolOutputKind::JsonArtifact,
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
            });
        }

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: decompiled,
            stdout: None,
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

