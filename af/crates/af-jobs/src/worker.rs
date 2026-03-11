use crate::context_builder;
use af_core::{CoreConfig, ExecutorEntry, ToolConfigHook, ToolError, ToolExecutorRegistry, ToolOutputKind, ToolSpecRegistry};
use af_storage::scratch;
use sqlx::PgPool;
use std::sync::Arc;
use tracing;

const LEASE_DURATION_SECS: i64 = 120;
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Worker — claim loop + heartbeat task + execute + store result.
///
/// Security model:
/// - `claim()` runs as `af` (table owner, bypasses RLS) — sees all queued jobs
/// - Post-execution DB ops use `begin_scoped()` with the job's `actor_user_id`
///   for defense-in-depth RLS enforcement on tenant-scoped tables
/// - `audit_log` and `heartbeat` run unscoped (not RLS-protected)
pub struct Worker {
    pool: PgPool,
    config: CoreConfig,
    specs: Arc<ToolSpecRegistry>,
    executors: Arc<ToolExecutorRegistry>,
    tool_config_hooks: Arc<Vec<Arc<dyn ToolConfigHook>>>,
}

impl Worker {
    pub fn new(
        pool: PgPool,
        config: CoreConfig,
        specs: Arc<ToolSpecRegistry>,
        executors: Arc<ToolExecutorRegistry>,
        tool_config_hooks: Arc<Vec<Arc<dyn ToolConfigHook>>>,
    ) -> Self {
        Self {
            pool,
            config,
            specs,
            executors,
            tool_config_hooks,
        }
    }

    /// Try to claim and execute one job. Returns true if a job was processed.
    pub async fn try_execute_one(&self) -> Result<bool, WorkerError> {
        eprintln!("[worker-debug] try_execute_one: attempting to claim a job...");
        // Claim runs as af (table owner) — needs to see all queued jobs across tenants
        let run = match af_db::tool_runs::claim(&self.pool, LEASE_DURATION_SECS)
            .await
            .map_err(|e| WorkerError::Db(e.to_string()))?
        {
            Some(r) => r,
            None => {
                eprintln!("[worker-debug] try_execute_one: no job to claim!");
                return Ok(false);
            }
        };

        eprintln!("[worker-debug] claimed job run_id={} tool={} input={}",
            run.id, run.tool_name,
            serde_json::to_string(&run.input_json).unwrap_or_else(|_| "<serialize error>".into()));
        tracing::info!(tool_run_id = %run.id, tool = %run.tool_name, "claimed job");

        // Look up spec + executor
        let spec = self
            .specs
            .get(&run.tool_name, run.tool_version as u32)
            .ok_or_else(|| WorkerError::ToolNotFound(run.tool_name.clone()))?;

        let entry = self
            .executors
            .get(&run.tool_name, run.tool_version as u32)
            .ok_or_else(|| WorkerError::ExecutorNotFound(run.tool_name.clone()))?;

        // Build context
        let mut ctx = context_builder::build_context(
            &self.pool,
            &self.config,
            &run,
            spec.policy.max_output_bytes,
            spec.policy.max_produced_artifacts,
        )
        .await
        .map_err(|e| WorkerError::Context(e.to_string()))?;

        // Run tool config hooks to enrich ctx.tool_config before execution
        for hook in self.tool_config_hooks.iter() {
            hook.enrich(
                &run.tool_name,
                run.project_id,
                &ctx.artifacts,
                &mut ctx.tool_config,
            )
            .await;
        }

        let scratch_dir = ctx.scratch_dir.clone();

        // Execute the job, ensuring scratch_dir cleanup on ALL paths (success, error, panic)
        let result = self.execute_claimed(&run, &spec, entry, ctx).await;

        // Cleanup scratch — always runs, even if execute_claimed returned Err
        let _ = scratch::cleanup_scratch_dir(&scratch_dir).await;

        result
    }

    /// Inner execution logic for a claimed job. Separated from try_execute_one so
    /// scratch_dir cleanup always runs in the caller regardless of how this returns.
    async fn execute_claimed(
        &self,
        run: &af_db::tool_runs::ToolRunRow,
        spec: &af_core::ToolSpec,
        entry: &ExecutorEntry,
        ctx: af_core::ToolContext,
    ) -> Result<bool, WorkerError> {
        // Start heartbeat task (runs unscoped — just extends lease on specific row).
        // Uses a notify to signal execution to abort when heartbeat fails (job was reaped).
        let pool_hb = self.pool.clone();
        let run_id = run.id;
        let heartbeat_cancel = Arc::new(tokio::sync::Notify::new());
        let heartbeat_cancel_tx = heartbeat_cancel.clone();
        let heartbeat_handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
            loop {
                interval.tick().await;
                if let Err(e) =
                    af_db::tool_runs::heartbeat(&pool_hb, run_id, LEASE_DURATION_SECS).await
                {
                    tracing::warn!(tool_run_id = %run_id, "heartbeat failed (job likely reaped): {e}");
                    heartbeat_cancel_tx.notify_one();
                    break;
                }
            }
        });

        eprintln!("[worker-debug] run_id={} tool={} executor_type={} artifacts_count={}",
            run.id, run.tool_name,
            match entry { ExecutorEntry::InProcess(_) => "InProcess", ExecutorEntry::OutOfProcess(_) => "OOP" },
            ctx.artifacts.len());
        for art in &ctx.artifacts {
            eprintln!("[worker-debug]   artifact: id={} sha256={} filename={} path={}",
                art.id, art.sha256, art.filename, art.storage_path.display());
        }

        // Dispatch based on executor type — abort if heartbeat fails
        let result = match entry {
            ExecutorEntry::InProcess(executor) => {
                // Semantic validation
                if let Err(msg) = executor.validate(&ctx, &run.input_json) {
                    heartbeat_handle.abort();
                    let err_json = serde_json::json!({
                        "code": "validation_error",
                        "message": msg,
                        "retryable": false,
                        "details": null
                    });

                    // Fail uses scoped tx for defense-in-depth
                    self.scoped_fail(run.actor_user_id, run.id, &err_json).await?;

                    // Audit log (not RLS-protected, runs unscoped)
                    let detail = serde_json::json!({
                        "tool_run_id": run.id.to_string(),
                        "tool_name": run.tool_name,
                        "status": "validation_failed",
                        "error": msg,
                    });
                    let _ = af_db::audit_log::insert(
                        &self.pool,
                        "tool_run",
                        None,
                        run.actor_user_id,
                        Some(&detail),
                    )
                    .await;

                    return Ok(true);
                }
                let exec_fut = executor.execute(ctx, run.input_json.clone());
                tokio::select! {
                    res = exec_fut => res,
                    _ = heartbeat_cancel.notified() => {
                        tracing::warn!(tool_run_id = %run.id, tool = %run.tool_name, "aborting execution: heartbeat lost");
                        Err(ToolError {
                            code: "heartbeat_lost".into(),
                            message: "execution aborted: job heartbeat failed (likely reaped)".into(),
                            retryable: false,
                            details: serde_json::Value::Null,
                        })
                    }
                }
            }
            ExecutorEntry::OutOfProcess(spawn_config) => {
                // Create stderr streaming channel for real-time log observation
                let (stderr_tx, mut stderr_rx) = tokio::sync::mpsc::channel::<String>(64);
                let writer_pool = self.pool.clone();
                let writer_run_id = run.id;
                let stderr_writer = tokio::spawn(async move {
                    while let Some(line) = stderr_rx.recv().await {
                        let payload = serde_json::json!(line);
                        if let Err(e) = af_db::tool_run_events::insert_event(
                            &writer_pool, writer_run_id, "stderr", Some(&payload),
                        ).await {
                            tracing::warn!(tool_run_id = %writer_run_id, "failed to write stderr event: {e}");
                        }
                    }
                });

                let exec_fut = crate::oop_executor::execute_oop(
                    spawn_config,
                    &run.tool_name,
                    run.tool_version as u32,
                    &run.input_json,
                    &ctx,
                    Some(&spec.policy),
                    Some(&self.pool),
                    Some(stderr_tx),
                );
                let res = tokio::select! {
                    res = exec_fut => res,
                    _ = heartbeat_cancel.notified() => {
                        tracing::warn!(tool_run_id = %run.id, tool = %run.tool_name, "aborting OOP execution: heartbeat lost");
                        Err(ToolError {
                            code: "heartbeat_lost".into(),
                            message: "execution aborted: job heartbeat failed (likely reaped)".into(),
                            retryable: false,
                            details: serde_json::Value::Null,
                        })
                    }
                };
                // Wait for stderr writer to flush remaining events.
                // Bounded timeout: after exec completes/cancels, child is killed (kill_on_drop),
                // pipes close, reader tasks finish, channel closes. Give 5s for DB flush.
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    stderr_writer,
                ).await {
                    Ok(_) => {}
                    Err(_) => tracing::warn!(tool_run_id = %run.id, "stderr writer drain timed out after 5s"),
                }
                res
            }
        };

        heartbeat_handle.abort();

        match result {
            Ok(tool_result) => {
                let output_kind = match tool_result.kind {
                    ToolOutputKind::InlineJson => "inline_json",
                    ToolOutputKind::JsonArtifact => "json_artifact",
                    ToolOutputKind::Text => "text",
                    ToolOutputKind::Binary => "binary",
                    ToolOutputKind::Mixed => "mixed",
                };

                // Use scoped transaction for tenant-scoped DB operations
                if let Some(uid) = run.actor_user_id {
                    if let Ok(mut tx) = af_db::begin_scoped(&self.pool, uid).await {
                        for aid in &tool_result.produced_artifacts {
                            let _ = af_db::tool_run_artifacts::link_artifact(
                                &mut *tx,
                                run.id,
                                *aid,
                                "output",
                            )
                            .await;
                        }
                        let _ = af_db::tool_runs::complete(
                            &mut *tx,
                            run.id,
                            &tool_result.output_json,
                            output_kind,
                            tool_result.stdout.as_deref(),
                            tool_result.stderr.as_deref(),
                        )
                        .await;
                        let _ = tx.commit().await;
                    } else {
                        // Fallback: unscoped (table owner bypasses RLS anyway)
                        self.unscoped_complete(&tool_result, run.id, output_kind).await;
                    }
                } else {
                    // No actor_user_id — run unscoped
                    self.unscoped_complete(&tool_result, run.id, output_kind).await;
                }

                // Audit log (not RLS-protected, runs unscoped)
                let detail = serde_json::json!({
                    "tool_run_id": run.id.to_string(),
                    "tool_name": run.tool_name,
                    "status": "completed",
                    "output_kind": output_kind,
                });
                let _ = af_db::audit_log::insert(
                    &self.pool,
                    "tool_run",
                    None,
                    run.actor_user_id,
                    Some(&detail),
                )
                .await;
            }
            Err(tool_err) => {
                let err_json = serde_json::to_value(&tool_err).unwrap_or_default();
                self.scoped_fail(run.actor_user_id, run.id, &err_json).await?;

                // Audit log (not RLS-protected, runs unscoped)
                let detail = serde_json::json!({
                    "tool_run_id": run.id.to_string(),
                    "tool_name": run.tool_name,
                    "status": "failed",
                    "error": tool_err.message,
                });
                let _ = af_db::audit_log::insert(
                    &self.pool,
                    "tool_run",
                    None,
                    run.actor_user_id,
                    Some(&detail),
                )
                .await;
            }
        }

        Ok(true)
    }

    /// Complete a tool run without RLS scoping.
    async fn unscoped_complete(
        &self,
        tool_result: &af_core::ToolResult,
        run_id: uuid::Uuid,
        output_kind: &str,
    ) {
        for aid in &tool_result.produced_artifacts {
            let _ = af_db::tool_run_artifacts::link_artifact(
                &self.pool,
                run_id,
                *aid,
                "output",
            )
            .await;
        }
        let _ = af_db::tool_runs::complete(
            &self.pool,
            run_id,
            &tool_result.output_json,
            output_kind,
            tool_result.stdout.as_deref(),
            tool_result.stderr.as_deref(),
        )
        .await;
    }

    /// Fail a tool run, using scoped transaction if actor_user_id is available.
    async fn scoped_fail(
        &self,
        actor_user_id: Option<uuid::Uuid>,
        run_id: uuid::Uuid,
        err_json: &serde_json::Value,
    ) -> Result<(), WorkerError> {
        if let Some(uid) = actor_user_id {
            if let Ok(mut tx) = af_db::begin_scoped(&self.pool, uid).await {
                af_db::tool_runs::fail(&mut *tx, run_id, err_json, None)
                    .await
                    .map_err(|e| WorkerError::Db(e.to_string()))?;
                tx.commit().await.map_err(|e| WorkerError::Db(e.to_string()))?;
                return Ok(());
            }
        }
        // Fallback: unscoped
        af_db::tool_runs::fail(&self.pool, run_id, err_json, None)
            .await
            .map_err(|e| WorkerError::Db(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("database error: {0}")]
    Db(String),
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    #[error("executor not found: {0}")]
    ExecutorNotFound(String),
    #[error("OOP executor error: {0}")]
    OopError(String),
    #[error("context build error: {0}")]
    Context(String),
}
