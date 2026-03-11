use af_core::{SandboxProfile, SpawnConfig, ToolContext, ToolError, ToolOutputKind, ToolResult, ToolPolicy};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Cached result of bwrap availability check.
static BWRAP_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Cached result of oaie availability check.
static OAIE_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// OOP envelope sent to the executor binary on stdin.
#[derive(Debug, Serialize)]
struct OopEnvelope {
    tool_name: String,
    tool_version: u32,
    input: serde_json::Value,
    context: OopContext,
}

#[derive(Debug, Serialize)]
struct OopContext {
    project_id: Uuid,
    tool_run_id: Uuid,
    scratch_dir: PathBuf,
    artifacts: Vec<OopArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor_user_id: Option<Uuid>,
    #[serde(default)]
    extra: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct OopArtifact {
    id: Uuid,
    sha256: String,
    filename: String,
    storage_path: PathBuf,
    size_bytes: u64,
    mime_type: Option<String>,
}

/// OOP response from the executor binary on stdout.
#[derive(Debug, Deserialize)]
struct OopResponse {
    result: OopResult,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status")]
enum OopResult {
    #[serde(rename = "ok")]
    Ok {
        output: serde_json::Value,
        /// Files produced by the executor in scratch_dir.
        /// Ingested into content-addressed blob storage by `ingest_produced_file()`.
        #[serde(default)]
        produced_files: Vec<ProducedFile>,
    },
    #[serde(rename = "error")]
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Deserialize)]
struct ProducedFile {
    filename: String,
    path: PathBuf,
    mime_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Execute a tool via the OOP executor binary, optionally inside bwrap.
///
/// If `pool` is provided, produced files from the executor are ingested into
/// content-addressed blob storage and linked as artifacts.
///
/// Supports two protocol modes based on `config.context_extra["protocol"]`:
/// - **"simple"**: Flat JSON on stdin (artifact UUIDs replaced with file paths),
///   bare JSON output on stdout. No envelope, no produced_files.
/// - **default (OOP)**: Full `OopEnvelope` on stdin, `OopResponse` on stdout.
pub async fn execute_oop(
    config: &SpawnConfig,
    tool_name: &str,
    tool_version: u32,
    input: &serde_json::Value,
    ctx: &ToolContext,
    policy: Option<&ToolPolicy>,
    pool: Option<&PgPool>,
    stderr_tx: Option<mpsc::Sender<String>>,
) -> Result<ToolResult, ToolError> {
    let is_simple = config.context_extra.get("protocol")
        .and_then(|v| v.as_str()) == Some("simple");

    eprintln!("[oop-debug] tool={tool_name} v={tool_version} protocol={} input={}",
        if is_simple { "simple" } else { "oop" },
        serde_json::to_string(input).unwrap_or_else(|_| "<err>".into()));
    eprintln!("[oop-debug] tool={tool_name} artifacts_count={} scratch_dir={}",
        ctx.artifacts.len(), ctx.scratch_dir.display());
    for art in &ctx.artifacts {
        eprintln!("[oop-debug]   artifact: id={} sha256={} file={} path={}",
            art.id, art.sha256, art.filename, art.storage_path.display());
    }
    let stdin_json = if is_simple {
        build_simple_stdin(input, &config.context_extra, ctx)?
    } else {
        build_oop_envelope(config, tool_name, tool_version, input, ctx)?
    };
    eprintln!("[oop-debug] tool={tool_name} envelope_len={} bytes", stdin_json.len());

    // Determine sandbox requirements from policy
    let sandbox = policy.map(|p| &p.sandbox).cloned().unwrap_or(SandboxProfile::NoNetReadOnly);
    let timeout = Duration::from_millis(policy.map(|p| p.timeout_ms).unwrap_or(120_000));
    let use_oaie = ctx.core_config.use_oaie;

    let output = match sandbox {
        SandboxProfile::Trusted => {
            // Trusted tools run without sandbox
            spawn_direct(config, &stdin_json, timeout, stderr_tx).await?
        }
        SandboxProfile::NoNetReadOnly
        | SandboxProfile::NoNetReadOnlyTmpfs
        | SandboxProfile::PrivateLoopback
        | SandboxProfile::NetEgressAllowlist => {
            if use_oaie && oaie_available().await {
                let mounts = BwrapMounts {
                    artifact_paths: ctx.artifacts.iter().map(|a| a.storage_path.as_path()).collect(),
                    scratch_dir: &ctx.scratch_dir,
                    uds_bind_mounts: policy.map(|p| &p.uds_bind_mounts[..]).unwrap_or(&[]),
                    writable_bind_mounts: policy.map(|p| &p.writable_bind_mounts[..]).unwrap_or(&[]),
                    extra_ro_bind_mounts: policy.map(|p| &p.extra_ro_bind_mounts[..]).unwrap_or(&[]),
                };
                spawn_with_oaie(config, &mounts, &sandbox, &stdin_json, timeout, stderr_tx)
                    .await?
            } else if bwrap_available().await {
                let mounts = BwrapMounts {
                    artifact_paths: ctx.artifacts.iter().map(|a| a.storage_path.as_path()).collect(),
                    scratch_dir: &ctx.scratch_dir,
                    uds_bind_mounts: policy.map(|p| &p.uds_bind_mounts[..]).unwrap_or(&[]),
                    writable_bind_mounts: policy.map(|p| &p.writable_bind_mounts[..]).unwrap_or(&[]),
                    extra_ro_bind_mounts: policy.map(|p| &p.extra_ro_bind_mounts[..]).unwrap_or(&[]),
                };
                spawn_with_bwrap(config, &mounts, &sandbox, &stdin_json, timeout, stderr_tx)
                    .await?
            } else if std::env::var("AF_ALLOW_UNSANDBOXED").is_ok() {
                eprintln!("[WARN] no sandbox available, running {tool_name} unsandboxed (AF_ALLOW_UNSANDBOXED set)");
                spawn_direct(config, &stdin_json, timeout, stderr_tx).await?
            } else {
                return Err(ToolError {
                    code: "sandbox_unavailable".into(),
                    message: format!(
                        "No sandbox available for tool '{tool_name}' (sandbox: {sandbox:?}). \
                         Install bubblewrap, use --oaie, or set AF_ALLOW_UNSANDBOXED=1 to override."
                    ),
                    retryable: false,
                    details: serde_json::Value::Null,
                });
            }
        }
    };

    // Simple protocol: bare JSON output, no envelope
    if is_simple {
        return parse_simple_response(&output);
    }

    // OOP protocol: parse envelope response
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    eprintln!("[oop-debug] tool={tool_name} stdout_len={} stderr_len={}", output.stdout.len(), output.stderr.len());
    if !stderr_str.is_empty() {
        // Print first 2000 chars of stderr for debugging
        let stderr_preview: String = stderr_str.chars().take(2000).collect();
        eprintln!("[oop-debug] tool={tool_name} stderr:\n{stderr_preview}");
    }
    if output.stdout.len() < 5000 {
        eprintln!("[oop-debug] tool={tool_name} stdout:\n{stdout_str}");
    } else {
        let preview: String = stdout_str.chars().take(2000).collect();
        eprintln!("[oop-debug] tool={tool_name} stdout (first 2000 chars):\n{preview}");
    }
    let resp: OopResponse = serde_json::from_slice(&output.stdout).map_err(|e| {
        eprintln!("[oop-debug] tool={tool_name} FAILED to parse OOP response: {e}");
        ToolError {
            code: "parse_error".into(),
            message: format!("failed to parse OOP response: {e}; stderr: {stderr_str}"),
            retryable: false,
            details: serde_json::Value::Null,
        }
    })?;

    match resp.result {
        OopResult::Ok {
            output: output_json,
            produced_files,
        } => {
            // Enforce produced file count limit
            let max_artifacts = policy.map(|p| p.max_produced_artifacts).unwrap_or(16) as usize;
            if produced_files.len() > max_artifacts {
                return Err(ToolError {
                    code: "limit_exceeded".into(),
                    message: format!(
                        "tool produced {} files, max allowed is {}",
                        produced_files.len(),
                        max_artifacts
                    ),
                    retryable: false,
                    details: serde_json::Value::Null,
                });
            }

            let max_file_bytes = policy.map(|p| p.max_output_bytes).unwrap_or(64 * 1024 * 1024);

            // Determine parent sample name for filename prefixing.
            // The first uploaded artifact (source_tool_run_id is None) in ctx.artifacts
            // is the input sample. Its filename stem becomes the prefix.
            let parent_sample_stem: Option<String> = ctx.artifacts.iter()
                .find(|a| a.source_tool_run_id.is_none())
                .map(|a| {
                    let name = &a.filename;
                    // Strip extension to get the stem: "amixer.elf" → "amixer"
                    match name.rfind('.') {
                        Some(pos) if pos > 0 => name[..pos].to_string(),
                        _ => name.clone(),
                    }
                });

            // Ingest produced files into content-addressed storage
            let mut produced_artifacts = Vec::new();
            if let Some(pool) = pool {
                for pf in &produced_files {
                    match ingest_produced_file(
                        pool,
                        &ctx.core_config.storage_root,
                        &ctx.scratch_dir,
                        ctx.project_id,
                        ctx.tool_run_id,
                        pf,
                        max_file_bytes,
                        parent_sample_stem.as_deref(),
                    )
                    .await
                    {
                        Ok(artifact_id) => {
                            produced_artifacts.push(artifact_id);

                            // Auto-enqueue chunks.json from doc tools for background embedding
                            if pf.filename == "chunks.json" && tool_name.starts_with("doc.") {
                                let source_id = ctx.artifacts.first().map(|a| a.id);
                                match af_db::embed_queue::enqueue(
                                    pool, ctx.project_id, artifact_id, source_id, tool_name,
                                ).await {
                                    Ok(Some(_)) => {
                                        tracing::info!("enqueued embedding for chunks.json artifact {artifact_id}");
                                    }
                                    Ok(None) => {
                                        tracing::debug!("chunks.json artifact {artifact_id} already enqueued, skipping");
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "failed to enqueue embedding for chunks.json artifact {artifact_id}: {e}"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "failed to ingest produced file {}: {e}",
                                pf.filename
                            );
                        }
                    }
                }
            }

            // Enrich output_json with produced artifact IDs for LLM visibility
            let mut output_json = output_json;
            if !produced_artifacts.is_empty() {
                if let Some(obj) = output_json.as_object_mut() {
                    let ids: Vec<String> =
                        produced_artifacts.iter().map(|id| id.to_string()).collect();
                    obj.insert(
                        "produced_artifact_ids".to_string(),
                        serde_json::json!(ids),
                    );
                }
            }

            Ok(ToolResult {
                kind: ToolOutputKind::InlineJson,
                output_json,
                stdout: None,
                stderr: if output.stderr.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&output.stderr).to_string())
                },
                produced_artifacts,
                primary_artifact: None,
                evidence: vec![],
            })
        }
        OopResult::Error {
            code,
            message,
            retryable,
        } => Err(ToolError {
            code,
            message,
            retryable,
            details: serde_json::Value::Null,
        }),
    }
}

/// Validate and ingest a single produced file from the OOP executor.
///
/// When `parent_sample_stem` is Some, the artifact filename is prefixed with the
/// parent sample name for disambiguation: "decompiled.json" → "amixer_decompiled.json".
///
/// Security: validates that the file path does not escape the scratch directory
/// (rejects `..` components and absolute paths).
async fn ingest_produced_file(
    pool: &PgPool,
    storage_root: &Path,
    scratch_dir: &Path,
    project_id: Uuid,
    tool_run_id: Uuid,
    pf: &ProducedFile,
    max_file_bytes: u64,
    parent_sample_stem: Option<&str>,
) -> Result<Uuid, ToolError> {
    // Path traversal validation: reject absolute paths and .. components
    let rel_path = &pf.path;
    if rel_path.is_absolute() {
        return Err(ToolError {
            code: "path_traversal".into(),
            message: format!("produced file path must be relative: {}", rel_path.display()),
            retryable: false,
            details: serde_json::Value::Null,
        });
    }
    for component in rel_path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(ToolError {
                code: "path_traversal".into(),
                message: format!(
                    "produced file path must not contain '..': {}",
                    rel_path.display()
                ),
                retryable: false,
                details: serde_json::Value::Null,
            });
        }
    }

    let file_path = scratch_dir.join(rel_path);

    // Extra safety: canonicalize and verify it's under scratch_dir
    let canonical = tokio::fs::canonicalize(&file_path).await.map_err(|e| ToolError {
        code: "io_error".into(),
        message: format!("failed to resolve produced file path: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;
    let canonical_scratch = tokio::fs::canonicalize(scratch_dir).await.map_err(|e| ToolError {
        code: "io_error".into(),
        message: format!("failed to resolve scratch dir: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;
    if !canonical.starts_with(&canonical_scratch) {
        return Err(ToolError {
            code: "path_traversal".into(),
            message: format!(
                "produced file escapes scratch dir: {}",
                canonical.display()
            ),
            retryable: false,
            details: serde_json::Value::Null,
        });
    }

    // Check file size before reading into memory
    let meta = tokio::fs::metadata(&canonical).await.map_err(|e| ToolError {
        code: "io_error".into(),
        message: format!("failed to stat produced file: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;
    if meta.len() > max_file_bytes {
        return Err(ToolError {
            code: "limit_exceeded".into(),
            message: format!(
                "produced file '{}' is {} bytes, max allowed is {}",
                pf.filename,
                meta.len(),
                max_file_bytes
            ),
            retryable: false,
            details: serde_json::Value::Null,
        });
    }

    let data = tokio::fs::read(&canonical).await.map_err(|e| ToolError {
        code: "io_error".into(),
        message: format!("failed to read produced file: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;

    // Store in content-addressed blob storage
    let (sha256, _storage_path) =
        af_storage::blob_store::store_blob(pool, storage_root, &data)
            .await
            .map_err(|e| ToolError {
                code: "storage_error".into(),
                message: format!("failed to store produced file: {e}"),
                retryable: false,
                details: serde_json::Value::Null,
            })?;

    // Prefix filename with parent sample stem for disambiguation:
    // "decompiled.json" → "amixer_decompiled.json"
    let stored_filename = match parent_sample_stem {
        Some(stem) => format!("{}_{}", stem, pf.filename),
        None => pf.filename.clone(),
    };

    // Create artifact record in DB
    let artifact = if let Some(desc) = &pf.description {
        af_db::artifacts::create_artifact_with_description(
            pool,
            project_id,
            &sha256,
            &stored_filename,
            pf.mime_type.as_deref(),
            Some(tool_run_id),
            desc,
        )
        .await
    } else {
        af_db::artifacts::create_artifact(
            pool,
            project_id,
            &sha256,
            &stored_filename,
            pf.mime_type.as_deref(),
            Some(tool_run_id),
        )
        .await
    }
    .map_err(|e| ToolError {
        code: "db_error".into(),
        message: format!("failed to create artifact record: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;

    Ok(artifact.id)
}

struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub async fn bwrap_available() -> bool {
    if let Some(&cached) = BWRAP_AVAILABLE.get() {
        return cached;
    }
    let available = tokio::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .await
        .is_ok();
    let _ = BWRAP_AVAILABLE.set(available);
    available
}

/// Check whether the `oaie` binary is available on the system.
pub async fn oaie_available() -> bool {
    if let Some(&cached) = OAIE_AVAILABLE.get() {
        return cached;
    }
    let available = tokio::process::Command::new("oaie")
        .arg("--version")
        .output()
        .await
        .is_ok();
    let _ = OAIE_AVAILABLE.set(available);
    available
}

/// Build the standard OOP envelope JSON string for stdin.
fn build_oop_envelope(
    config: &SpawnConfig,
    tool_name: &str,
    tool_version: u32,
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    // Merge tool_config into extra so per-invocation data (NDA, renames)
    // flows through to OOP executors alongside static context_extra.
    let mut merged_extra = config.context_extra.clone();
    if let Some(obj) = merged_extra.as_object_mut() {
        if let Some(tc) = ctx.tool_config.as_object() {
            for (k, v) in tc {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

    let envelope = OopEnvelope {
        tool_name: tool_name.to_string(),
        tool_version,
        input: input.clone(),
        context: OopContext {
            project_id: ctx.project_id,
            tool_run_id: ctx.tool_run_id,
            scratch_dir: ctx.scratch_dir.clone(),
            artifacts: ctx
                .artifacts
                .iter()
                .map(|a| OopArtifact {
                    id: a.id,
                    sha256: a.sha256.clone(),
                    filename: a.filename.clone(),
                    storage_path: a.storage_path.clone(),
                    size_bytes: a.size_bytes,
                    mime_type: a.mime_type.clone(),
                })
                .collect(),
            actor_user_id: ctx.actor_user_id,
            extra: merged_extra,
        },
    };

    serde_json::to_string(&envelope).map_err(|e| ToolError {
        code: "serialize_error".into(),
        message: format!("failed to serialize OOP envelope: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })
}

/// Build flat JSON for simple-protocol tools.
///
/// Clones the input, walks the pre-computed artifact_schema_paths, and replaces
/// each artifact UUID string with the corresponding storage_path from ctx.artifacts.
fn build_simple_stdin(
    input: &serde_json::Value,
    context_extra: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let mut flat_input = input.clone();

    if let Some(paths) = context_extra.get("artifact_schema_paths").and_then(|v| v.as_array()) {
        let schema_paths: Vec<String> = paths
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        inject_artifact_paths(&mut flat_input, &schema_paths, &ctx.artifacts);
    }

    serde_json::to_string(&flat_input).map_err(|e| ToolError {
        code: "serialize_error".into(),
        message: format!("failed to serialize simple input: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })
}

/// Walk JSON pointer paths and replace artifact UUID strings with file path strings.
///
/// For each path like "/artifact_id", find the value at that JSON pointer and replace
/// the UUID string with the corresponding storage_path from the artifacts list.
fn inject_artifact_paths(
    input: &mut serde_json::Value,
    schema_paths: &[String],
    artifacts: &[af_core::ArtifactRef],
) {
    for path in schema_paths {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        replace_at_path(input, &parts, artifacts);
    }
}

fn replace_at_path(
    value: &mut serde_json::Value,
    path: &[&str],
    artifacts: &[af_core::ArtifactRef],
) {
    if path.is_empty() {
        // Replace UUID string with file path
        if let Some(uuid_str) = value.as_str() {
            if let Ok(id) = uuid::Uuid::parse_str(uuid_str) {
                if let Some(art) = artifacts.iter().find(|a| a.id == id) {
                    *value = serde_json::Value::String(art.storage_path.to_string_lossy().into());
                }
            }
        }
        // Also handle arrays of UUIDs
        if let Some(arr) = value.as_array_mut() {
            for item in arr {
                if let Some(uuid_str) = item.as_str() {
                    if let Ok(id) = uuid::Uuid::parse_str(uuid_str) {
                        if let Some(art) = artifacts.iter().find(|a| a.id == id) {
                            *item = serde_json::Value::String(
                                art.storage_path.to_string_lossy().into(),
                            );
                        }
                    }
                }
            }
        }
        return;
    }

    if let Some(child) = value.get_mut(path[0]) {
        replace_at_path(child, &path[1..], artifacts);
    }
}

/// Parse simple-protocol stdout: bare JSON value, no envelope.
fn parse_simple_response(output: &ProcessOutput) -> Result<ToolResult, ToolError> {
    let output_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            ToolError {
                code: "parse_error".into(),
                message: format!("failed to parse simple tool output: {e}; stderr: {stderr}"),
                retryable: false,
                details: serde_json::Value::Null,
            }
        })?;

    Ok(ToolResult {
        kind: ToolOutputKind::InlineJson,
        output_json,
        stdout: None,
        stderr: if output.stderr.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        },
        produced_artifacts: vec![],
        primary_artifact: None,
        evidence: vec![],
    })
}

/// Collect stdout and stderr from a spawned child process, streaming stderr lines
/// through an optional channel for real-time observation (e.g. Ghidra progress).
/// Stdout is collected fully (needed for JSON response parsing).
async fn collect_output_streaming(
    mut child: tokio::process::Child,
    timeout: Duration,
    stderr_tx: Option<mpsc::Sender<String>>,
) -> Result<ProcessOutput, ToolError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let stdout_pipe = child.stdout.take().expect("stdout not piped");
    let stderr_pipe = child.stderr.take().expect("stderr not piped");

    // Spawn stdout reader — collects all bytes
    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut reader = stdout_pipe;
        let _ = reader.read_to_end(&mut buf).await;
        buf
    });

    // Spawn stderr reader — streams lines through channel, also collects all bytes
    let stderr_handle = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut reader = BufReader::new(stderr_pipe);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim_end().to_string();
                    collected.extend_from_slice(line.as_bytes());
                    if !trimmed.is_empty() {
                        if let Some(ref tx) = stderr_tx {
                            // Best-effort: if channel is full or closed, skip
                            let _ = tx.try_send(trimmed);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        collected
    });

    // Wait for process exit with timeout.
    // On error/timeout, abort reader tasks to avoid leaking them (and their channel senders).
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            stdout_handle.abort();
            stderr_handle.abort();
            return Err(ToolError {
                code: "io_error".into(),
                message: format!("failed to wait for process: {e}"),
                retryable: false,
                details: serde_json::Value::Null,
            });
        }
        Err(_) => {
            stdout_handle.abort();
            stderr_handle.abort();
            return Err(ToolError {
                code: "timeout".into(),
                message: format!("executor timed out after {}s", timeout.as_secs()),
                retryable: true,
                details: serde_json::Value::Null,
            });
        }
    };

    // Collect stdout/stderr from their tasks
    let stdout = stdout_handle.await.unwrap_or_default();
    let stderr = stderr_handle.await.unwrap_or_default();

    if !status.success() {
        let stderr_str = String::from_utf8_lossy(&stderr);
        return Err(ToolError {
            code: "exit_error".into(),
            message: format!("executor exited with {status}: {stderr_str}"),
            retryable: false,
            details: serde_json::Value::Null,
        });
    }

    Ok(ProcessOutput { stdout, stderr })
}

/// Bind mount configuration for bwrap sandbox.
struct BwrapMounts<'a> {
    artifact_paths: Vec<&'a Path>,
    scratch_dir: &'a Path,
    uds_bind_mounts: &'a [PathBuf],
    writable_bind_mounts: &'a [PathBuf],
    extra_ro_bind_mounts: &'a [PathBuf],
}

/// Conditionally add --ro-bind for a host path if it exists.
fn add_ro_bind_if_exists(cmd: &mut tokio::process::Command, path: &str) {
    if Path::new(path).exists() {
        cmd.arg("--ro-bind").arg(path).arg(path);
    }
}

/// Conditionally add --symlink if the host path is a symlink.
/// Returns true if it was a symlink (so we skip the ro-bind).
fn add_symlink_if_link(cmd: &mut tokio::process::Command, path: &str) -> bool {
    let p = Path::new(path);
    if let Ok(target) = std::fs::read_link(p) {
        let target_str = target.to_string_lossy();
        cmd.arg("--symlink").arg(target_str.as_ref()).arg(path);
        true
    } else {
        false
    }
}

async fn spawn_with_bwrap(
    config: &SpawnConfig,
    mounts: &BwrapMounts<'_>,
    sandbox: &SandboxProfile,
    stdin_data: &str,
    timeout: Duration,
    stderr_tx: Option<mpsc::Sender<String>>,
) -> Result<ProcessOutput, ToolError> {
    let mut cmd = tokio::process::Command::new("bwrap");

    // Minimal root filesystem — NOT --ro-bind / /
    cmd.arg("--tmpfs").arg("/");

    // System directories — check existence, handle symlinks (e.g. /lib -> usr/lib on some distros)
    for path in &["/usr", "/bin", "/sbin"] {
        if !add_symlink_if_link(&mut cmd, path) {
            add_ro_bind_if_exists(&mut cmd, path);
        }
    }
    // /lib and /lib64 are often symlinks to /usr/lib on modern distros
    for path in &["/lib", "/lib64", "/lib32"] {
        if !add_symlink_if_link(&mut cmd, path) {
            add_ro_bind_if_exists(&mut cmd, path);
        }
    }

    // /etc — needed for dynamic linker cache, Java security config, SSL certs, etc.
    add_ro_bind_if_exists(&mut cmd, "/etc");

    // User's home config (e.g. ~/.ghidra for saved JDK path, logs, OSGi cache)
    // Must be writable — Ghidra writes application.log, script.log, osgi/felixcache/cache.lock
    if let Ok(home) = std::env::var("HOME") {
        let ghidra_config = format!("{home}/.ghidra");
        if Path::new(&ghidra_config).exists() {
            cmd.arg("--bind").arg(&ghidra_config).arg(&ghidra_config);
        }
    }

    // Virtual filesystems
    cmd.arg("--dev").arg("/dev");
    cmd.arg("--proc").arg("/proc");
    cmd.arg("--tmpfs").arg("/tmp");

    // Isolation — PrivateLoopback keeps host network (Java/Ghidra needs loopback)
    match sandbox {
        SandboxProfile::PrivateLoopback => {
            // No namespace unsharing — Java/Ghidra hangs with user/net/pid namespaces.
            // Still sandboxed via tmpfs root, selective bind mounts, and cap-drop ALL.
        }
        _ => {
            cmd.arg("--unshare-all");
        }
    }
    cmd.arg("--die-with-parent")
        .arg("--cap-drop").arg("ALL");

    // Clear inherited environment to prevent leaking API keys / credentials
    let real_home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    cmd.arg("--clearenv")
        .arg("--setenv").arg("PATH").arg("/usr/local/bin:/usr/bin:/bin")
        .arg("--setenv").arg("HOME").arg(&real_home);

    // Per-artifact read-only bind mounts (instead of entire storage root)
    let mut created_dirs = std::collections::HashSet::new();
    for artifact_path in &mounts.artifact_paths {
        if let Some(parent) = artifact_path.parent() {
            if created_dirs.insert(parent.to_path_buf()) {
                cmd.arg("--dir").arg(parent);
            }
        }
        cmd.arg("--ro-bind").arg(artifact_path).arg(artifact_path);
    }

    // Writable scratch dir
    cmd.arg("--bind")
        .arg(mounts.scratch_dir)
        .arg(mounts.scratch_dir);

    // UDS bind-mounts for gateway-pattern tools
    for uds_path in mounts.uds_bind_mounts {
        cmd.arg("--ro-bind").arg(uds_path).arg(uds_path);
    }

    // Policy-declared writable bind mounts (e.g. Ghidra cache dir)
    for path in mounts.writable_bind_mounts {
        if path.exists() {
            cmd.arg("--bind").arg(path).arg(path);
        }
    }

    // Policy-declared extra read-only bind mounts (e.g. /etc/ssl/certs)
    for path in mounts.extra_ro_bind_mounts {
        if path.exists() {
            cmd.arg("--ro-bind").arg(path).arg(path);
        }
    }

    // The executor binary itself (may not be under /usr)
    let binary = &config.binary_path;
    cmd.arg("--ro-bind").arg(binary).arg(binary);

    // The executor binary
    cmd.arg(binary);

    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Ensure child process is killed if the future is cancelled (e.g. timeout)
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| ToolError {
        code: "spawn_error".into(),
        message: format!("failed to spawn bwrap: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;

    // Write envelope to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(stdin_data.as_bytes()).await.map_err(|e| ToolError {
            code: "io_error".into(),
            message: format!("failed to write to stdin: {e}"),
            retryable: false,
            details: serde_json::Value::Null,
        })?;
        drop(stdin);
    }

    collect_output_streaming(child, timeout, stderr_tx).await
}

/// Run executor inside OAIE sandbox. Since OAIE redirects the child's stdin to
/// /dev/null and captures stdout/stderr internally, we use file-based I/O:
///   1. Write envelope JSON to a temp file in scratch_dir
///   2. Use shell redirection: `sh -c 'executor < envelope > response 2> stderr_file'`
///   3. Read response and stderr from files after execution
async fn spawn_with_oaie(
    config: &SpawnConfig,
    mounts: &BwrapMounts<'_>,
    sandbox: &SandboxProfile,
    stdin_data: &str,
    timeout: Duration,
    _stderr_tx: Option<mpsc::Sender<String>>,
) -> Result<ProcessOutput, ToolError> {
    // Write envelope to scratch_dir (writable inside sandbox)
    let envelope_path = mounts.scratch_dir.join("_af_envelope.json");
    let response_path = mounts.scratch_dir.join("_af_response.json");
    let stderr_path = mounts.scratch_dir.join("_af_stderr.txt");

    tokio::fs::write(&envelope_path, stdin_data.as_bytes()).await.map_err(|e| ToolError {
        code: "io_error".into(),
        message: format!("failed to write envelope file: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;

    let mut cmd = tokio::process::Command::new("oaie");
    cmd.arg("run");

    // Policy based on sandbox profile
    let policy_name = match sandbox {
        SandboxProfile::NoNetReadOnly | SandboxProfile::NoNetReadOnlyTmpfs => "agent-safe",
        SandboxProfile::PrivateLoopback => "agent-analyze",
        SandboxProfile::NetEgressAllowlist => "agent-net",
        SandboxProfile::Trusted => unreachable!("Trusted profile handled before oaie path"),
    };
    cmd.arg("--policy").arg(policy_name);

    // Timeout
    cmd.arg("--timeout").arg(format!("{}s", timeout.as_secs()));

    // No network for most profiles (agent-safe already has network off,
    // but explicit --net=off ensures it regardless of policy defaults)
    if matches!(sandbox, SandboxProfile::NoNetReadOnly | SandboxProfile::NoNetReadOnlyTmpfs) {
        cmd.arg("--net=off");
    }

    // Suppress oaie's own output (we read results from files)
    cmd.arg("--quiet");

    // No auto-mount (we specify all mounts explicitly)
    cmd.arg("--no-auto-mount");

    // RO mounts for input artifacts
    for path in &mounts.artifact_paths {
        cmd.arg("--ro").arg(path);
    }

    // RO mounts for UDS sockets and extra paths
    for path in mounts.uds_bind_mounts {
        cmd.arg("--ro").arg(path);
    }
    for path in mounts.extra_ro_bind_mounts {
        cmd.arg("--ro").arg(path);
    }

    // RO mount for the executor binary itself
    cmd.arg("--ro").arg(&config.binary_path);

    // RW mount for scratch directory (where envelope, response, and produced files live)
    cmd.arg("--rw").arg(mounts.scratch_dir);

    // RW mounts for writable bind mounts (e.g. Ghidra cache)
    for path in mounts.writable_bind_mounts {
        cmd.arg("--rw").arg(path);
    }

    // The command: use shell redirection to pipe envelope to executor via files.
    // Paths are single-quoted to prevent shell injection / breakage with special chars.
    cmd.arg("--");
    cmd.arg("sh").arg("-c").arg(format!(
        "'{}' < '{}' > '{}' 2> '{}'",
        config.binary_path.display(),
        envelope_path.display(),
        response_path.display(),
        stderr_path.display(),
    ));

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd.kill_on_drop(true);

    eprintln!("[oaie] running tool with policy={policy_name} timeout={}s", timeout.as_secs());

    let child = cmd.spawn().map_err(|e| ToolError {
        code: "spawn_error".into(),
        message: format!("failed to spawn oaie: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;

    // Wait for oaie to finish (with extra grace period for oaie's own overhead)
    let oaie_timeout = timeout + Duration::from_secs(30);
    let status = match tokio::time::timeout(oaie_timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(ToolError {
                code: "io_error".into(),
                message: format!("failed to wait for oaie: {e}"),
                retryable: false,
                details: serde_json::Value::Null,
            });
        }
        Err(_) => {
            return Err(ToolError {
                code: "timeout".into(),
                message: format!("oaie timed out after {}s", oaie_timeout.as_secs()),
                retryable: true,
                details: serde_json::Value::Null,
            });
        }
    };

    // Log oaie's own stderr (sandbox setup messages, errors)
    if !status.stderr.is_empty() {
        let oaie_stderr = String::from_utf8_lossy(&status.stderr);
        eprintln!("[oaie] stderr: {}", oaie_stderr.chars().take(2000).collect::<String>());
    }

    // Read the executor's response and stderr from files
    let stdout = match tokio::fs::read(&response_path).await {
        Ok(data) => data,
        Err(e) => {
            eprintln!("[oaie] warning: response file missing or unreadable: {e}");
            Vec::new()
        }
    };
    let stderr = tokio::fs::read(&stderr_path).await.unwrap_or_default();

    // Clean up temp files (best-effort)
    let _ = tokio::fs::remove_file(&envelope_path).await;
    let _ = tokio::fs::remove_file(&response_path).await;
    let _ = tokio::fs::remove_file(&stderr_path).await;

    // Check oaie's exit status — non-zero could be sandbox setup failure OR executor failure
    if !status.status.success() && stdout.is_empty() {
        // No response file means oaie itself failed (not the executor)
        let oaie_stderr = String::from_utf8_lossy(&status.stderr);
        return Err(ToolError {
            code: "sandbox_error".into(),
            message: format!("oaie sandbox failed (exit {}): {oaie_stderr}", status.status),
            retryable: false,
            details: serde_json::Value::Null,
        });
    }

    Ok(ProcessOutput { stdout, stderr })
}

async fn spawn_direct(
    config: &SpawnConfig,
    stdin_data: &str,
    timeout: Duration,
    stderr_tx: Option<mpsc::Sender<String>>,
) -> Result<ProcessOutput, ToolError> {
    let mut cmd = tokio::process::Command::new(&config.binary_path);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Ensure child process is killed if the future is cancelled (e.g. timeout)
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| ToolError {
        code: "spawn_error".into(),
        message: format!("failed to spawn executor: {e}"),
        retryable: false,
        details: serde_json::Value::Null,
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(stdin_data.as_bytes()).await.map_err(|e| ToolError {
            code: "io_error".into(),
            message: format!("failed to write to stdin: {e}"),
            retryable: false,
            details: serde_json::Value::Null,
        })?;
        drop(stdin);
    }

    collect_output_streaming(child, timeout, stderr_tx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_inject_artifact_paths_single() {
        let id = uuid::Uuid::new_v4();
        let mut input = json!({ "artifact_id": id.to_string() });
        let artifacts = vec![af_core::ArtifactRef {
            id,
            sha256: "abc123".into(),
            filename: "test.bin".into(),
            storage_path: PathBuf::from("/tmp/af/storage/ab/abc123"),
            size_bytes: 1024,
            mime_type: None,
            source_tool_run_id: None,
        }];
        inject_artifact_paths(&mut input, &["/artifact_id".into()], &artifacts);
        assert_eq!(input["artifact_id"], "/tmp/af/storage/ab/abc123");
    }

    #[test]
    fn test_inject_artifact_paths_nested() {
        let id = uuid::Uuid::new_v4();
        let mut input = json!({ "opts": { "target": id.to_string() } });
        let artifacts = vec![af_core::ArtifactRef {
            id,
            sha256: "def456".into(),
            filename: "nested.bin".into(),
            storage_path: PathBuf::from("/storage/de/def456"),
            size_bytes: 512,
            mime_type: None,
            source_tool_run_id: None,
        }];
        inject_artifact_paths(&mut input, &["/opts/target".into()], &artifacts);
        assert_eq!(input["opts"]["target"], "/storage/de/def456");
    }

    #[test]
    fn test_inject_artifact_paths_array() {
        let id1 = uuid::Uuid::new_v4();
        let id2 = uuid::Uuid::new_v4();
        let mut input = json!({ "targets": [id1.to_string(), id2.to_string()] });
        let artifacts = vec![
            af_core::ArtifactRef {
                id: id1,
                sha256: "aaa".into(),
                filename: "a.bin".into(),
                storage_path: PathBuf::from("/s/aaa"),
                size_bytes: 100,
                mime_type: None,
                source_tool_run_id: None,
            },
            af_core::ArtifactRef {
                id: id2,
                sha256: "bbb".into(),
                filename: "b.bin".into(),
                storage_path: PathBuf::from("/s/bbb"),
                size_bytes: 200,
                mime_type: None,
                source_tool_run_id: None,
            },
        ];
        inject_artifact_paths(&mut input, &["/targets".into()], &artifacts);
        assert_eq!(input["targets"][0], "/s/aaa");
        assert_eq!(input["targets"][1], "/s/bbb");
    }

    #[test]
    fn test_inject_artifact_paths_missing_path() {
        let mut input = json!({ "other_field": "hello" });
        inject_artifact_paths(&mut input, &["/artifact_id".into()], &[]);
        assert_eq!(input["other_field"], "hello");
    }

    #[test]
    fn test_parse_simple_response_ok() {
        let output = ProcessOutput {
            stdout: br#"{"hash":"abc123","type":"PE32"}"#.to_vec(),
            stderr: vec![],
        };
        let result = parse_simple_response(&output).unwrap();
        assert_eq!(result.output_json["hash"], "abc123");
        assert!(result.produced_artifacts.is_empty());
    }

    #[test]
    fn test_parse_simple_response_invalid_json() {
        let output = ProcessOutput {
            stdout: b"not json".to_vec(),
            stderr: b"some error".to_vec(),
        };
        let err = parse_simple_response(&output).unwrap_err();
        assert_eq!(err.code, "parse_error");
        assert!(err.message.contains("some error"));
    }
}
