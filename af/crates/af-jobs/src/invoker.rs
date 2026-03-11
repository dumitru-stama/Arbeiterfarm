use async_trait::async_trait;
use af_core::{CoreConfig, ToolError, ToolExecutorRegistry, ToolInvoker, ToolRequest, ToolResult, ToolSpecRegistry};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

use crate::worker::Worker;

/// Production invoker: enqueue + spawn worker in background + poll for completion.
/// Also starts the reaper background task for crash recovery.
pub struct JobQueueInvoker {
    pool: PgPool,
    config: CoreConfig,
    specs: Arc<ToolSpecRegistry>,
    executors: Arc<ToolExecutorRegistry>,
    _reaper_shutdown: watch::Sender<bool>,
}

impl JobQueueInvoker {
    pub fn new(
        pool: PgPool,
        config: CoreConfig,
        specs: Arc<ToolSpecRegistry>,
        executors: Arc<ToolExecutorRegistry>,
    ) -> Self {
        // Start reaper background task
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let reaper_pool = pool.clone();
        tokio::spawn(crate::reaper::run_reaper(reaper_pool, shutdown_rx));

        Self {
            pool,
            config,
            specs,
            executors,
            _reaper_shutdown: shutdown_tx,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl ToolInvoker for JobQueueInvoker {
    async fn invoke(&self, request: ToolRequest) -> Result<ToolResult, ToolError> {
        eprintln!("[invoker-debug] enqueuing tool={}", request.tool_name);
        // Enqueue
        let run_id = crate::enqueue::enqueue(
            &self.pool,
            &self.specs,
            &request,
            None,
        )
        .await
        .map_err(|e| ToolError {
            code: "enqueue_error".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::Value::Null,
        })?;
        eprintln!("[invoker-debug] enqueued run_id={run_id}");

        // Spawn a worker task to process it
        let worker = Worker::new(
            self.pool.clone(),
            self.config.clone(),
            self.specs.clone(),
            self.executors.clone(),
            Arc::new(Vec::new()), // No tool_config_hooks in inline invocation
        );

        tokio::spawn(async move {
            eprintln!("[invoker-debug] worker task started");
            // Loop to drain the queue — stale jobs from previous invocations
            // may sit ahead of the newly enqueued job in FIFO order.
            loop {
                match worker.try_execute_one().await {
                    Ok(true) => {
                        eprintln!("[invoker-debug] worker processed a job, checking for more...");
                        continue;
                    }
                    Ok(false) => {
                        eprintln!("[invoker-debug] worker: no more jobs in queue");
                        break;
                    }
                    Err(e) => {
                        eprintln!("[invoker] worker error: {e}");
                        break;
                    }
                }
            }
            eprintln!("[invoker-debug] worker task finished");
        });

        // Give the worker task a chance to start
        tokio::task::yield_now().await;

        // Poll for completion
        let poll_interval = Duration::from_millis(100);
        let timeout = Duration::from_secs(300);
        let start = tokio::time::Instant::now();
        let mut poll_count = 0u32;

        loop {
            if start.elapsed() > timeout {
                return Err(ToolError {
                    code: "timeout".to_string(),
                    message: "tool run timed out waiting for completion".to_string(),
                    retryable: true,
                    details: serde_json::Value::Null,
                });
            }

            tokio::time::sleep(poll_interval).await;
            poll_count += 1;
            if poll_count <= 3 || poll_count % 50 == 0 {
                eprintln!("[invoker-debug] poll #{poll_count} for run_id={run_id}");
            }

            let row = af_db::tool_runs::get(&self.pool, run_id)
                .await
                .map_err(|e| ToolError {
                    code: "db_error".to_string(),
                    message: e.to_string(),
                    retryable: true,
                    details: serde_json::Value::Null,
                })?
                .ok_or_else(|| ToolError {
                    code: "not_found".to_string(),
                    message: format!("tool run {run_id} not found"),
                    retryable: false,
                    details: serde_json::Value::Null,
                })?;

            if poll_count <= 3 || poll_count % 50 == 0 {
                eprintln!("[invoker-debug] poll #{poll_count} status={}", row.status);
            }
            match row.status.as_str() {
                "completed" => {
                    let run_artifacts =
                        af_db::tool_run_artifacts::get_for_run(&self.pool, run_id)
                            .await
                            .unwrap_or_default();
                    let produced: Vec<uuid::Uuid> = run_artifacts
                        .iter()
                        .filter(|r| r.role == "output")
                        .map(|r| r.artifact_id)
                        .collect();

                    return Ok(ToolResult {
                        kind: parse_output_kind(row.output_kind.as_deref()),
                        output_json: row.output_json.unwrap_or(serde_json::Value::Null),
                        stdout: row.stdout,
                        stderr: row.stderr,
                        produced_artifacts: produced,
                        primary_artifact: None,
                        evidence: Vec::new(),
                    });
                }
                "failed" => {
                    let err: ToolError = row
                        .error_json
                        .and_then(|v| serde_json::from_value(v).ok())
                        .unwrap_or_else(|| ToolError {
                            code: "unknown".to_string(),
                            message: "tool run failed".to_string(),
                            retryable: false,
                            details: serde_json::Value::Null,
                        });
                    return Err(err);
                }
                _ => continue,
            }
        }
    }
}

fn parse_output_kind(s: Option<&str>) -> af_core::ToolOutputKind {
    match s {
        Some("inline_json") => af_core::ToolOutputKind::InlineJson,
        Some("json_artifact") => af_core::ToolOutputKind::JsonArtifact,
        Some("text") => af_core::ToolOutputKind::Text,
        Some("binary") => af_core::ToolOutputKind::Binary,
        Some("mixed") => af_core::ToolOutputKind::Mixed,
        _ => af_core::ToolOutputKind::InlineJson,
    }
}
