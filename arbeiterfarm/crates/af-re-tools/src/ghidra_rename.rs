use async_trait::async_trait;
use af_plugin_api::{EvidenceRef, PluginDb, ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Acquire a shared lock on the analysis lock file.
/// This blocks until any exclusive (write) lock from ghidra.analyze finishes.
fn acquire_shared_lock(project_dir: &std::path::Path) -> Result<std::fs::File, std::io::Error> {
    let lock_path = project_dir.join(".analysis.lock");
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

/// In-process executor: stores renames in the database instead of modifying the
/// Ghidra project on disk. This enables safe sharing of the analysis cache across
/// non-NDA projects. Renames are applied as an overlay during decompilation.
pub struct GhidraRenameExecutor {
    pub plugin_db: Arc<dyn PluginDb>,
    pub cache_dir: PathBuf,
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
impl ToolExecutor for GhidraRenameExecutor {
    fn tool_name(&self) -> &str {
        "ghidra.rename"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let renames = input["renames"]
            .as_array()
            .ok_or("renames must be an array")?;
        if renames.is_empty() {
            return Err("renames array cannot be empty".into());
        }
        if renames.len() > 50 {
            return Err("renames array cannot exceed 50 items".into());
        }
        for r in renames {
            let old = r["old"].as_str().ok_or("each rename must have an 'old' string")?;
            let new = r["new"].as_str().ok_or("each rename must have a 'new' string")?;
            if old.is_empty() {
                return Err("'old' name cannot be empty".into());
            }
            if new.is_empty() {
                return Err("'new' name cannot be empty".into());
            }
            if old.len() > 256 {
                return Err(format!("'old' name too long ({} chars, max 256)", old.len()));
            }
            if new.len() > 256 {
                return Err(format!("'new' name too long ({} chars, max 256)", new.len()));
            }
            if old.chars().any(|c| c.is_control()) {
                return Err("'old' name contains control characters".into());
            }
            if new.chars().any(|c| c.is_control()) {
                return Err("'new' name contains control characters".into());
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

        let renames_arr = input["renames"]
            .as_array()
            .unwrap();

        // Parse renames with optional address field
        let renames: Vec<(String, String, Option<String>)> = renames_arr
            .iter()
            .filter_map(|r| {
                let old = r["old"].as_str()?.to_string();
                let new = r["new"].as_str()?.to_string();
                if old.is_empty() || new.is_empty() {
                    return None;
                }
                let address = r.get("address").and_then(|a| a.as_str()).map(|s| s.to_string());
                Some((old, new, address))
            })
            .collect();

        if renames.is_empty() {
            return Err(tool_err(
                "invalid_input",
                "no valid renames (each needs non-empty 'old' and 'new')".into(),
            ));
        }

        // Verify Ghidra analysis exists on disk (shared or isolated path).
        // Acquire a shared lock so we wait for any in-progress analysis to finish.
        let is_nda = ctx.tool_config.get("nda")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let project_dir = crate::common::ghidra_cache_path(
            &self.cache_dir,
            &ctx.project_id.to_string(),
            &artifact.sha256,
            is_nda,
        );
        let lock_path = project_dir.join(".analysis.lock");
        let _lock = if lock_path.exists() {
            acquire_shared_lock(&project_dir).ok()
        } else {
            None
        };
        let gpr_exists = project_dir.join("analysis.gpr").exists();
        let rep_idata_exists = project_dir.join("analysis.rep/idata").is_dir();
        if !gpr_exists || !rep_idata_exists {
            return Err(tool_err(
                "no_analysis",
                "Run ghidra.analyze first to create the analysis project".into(),
            ));
        }

        // Upsert renames into database
        let count = crate::ghidra_renames_db::upsert_renames(
            &self.plugin_db,
            ctx.project_id,
            &artifact.sha256,
            &renames,
            ctx.actor_user_id,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("failed to store renames: {e}")))?;

        let renamed_pairs: Vec<Value> = renames
            .iter()
            .map(|(old, new, addr)| {
                let mut obj = json!({ "old": old, "new": new });
                if let Some(a) = addr {
                    obj["address"] = json!(a);
                }
                obj
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "renamed": count,
                "sha256": artifact.sha256,
                "project_id": ctx.project_id.to_string(),
                "renames": renamed_pairs,
                "note": "Renames stored in database. They will be applied as overlay during ghidra.decompile.",
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![EvidenceRef::Artifact(artifact.id)],
        })
    }
}
