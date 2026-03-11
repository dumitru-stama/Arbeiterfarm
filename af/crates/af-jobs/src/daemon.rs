use af_core::{CoreConfig, ToolConfigHook, ToolExecutorRegistry, ToolSpecRegistry};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::watch;

use crate::worker::Worker;

/// Multi-worker daemon that runs N concurrent claim loops.
pub struct WorkerDaemon {
    pool: PgPool,
    config: CoreConfig,
    specs: Arc<ToolSpecRegistry>,
    executors: Arc<ToolExecutorRegistry>,
    tool_config_hooks: Arc<Vec<Arc<dyn ToolConfigHook>>>,
    concurrency: usize,
    poll_ms: u64,
}

impl WorkerDaemon {
    pub fn new(
        pool: PgPool,
        config: CoreConfig,
        specs: Arc<ToolSpecRegistry>,
        executors: Arc<ToolExecutorRegistry>,
        concurrency: usize,
        poll_ms: u64,
    ) -> Self {
        Self {
            pool,
            config,
            specs,
            executors,
            tool_config_hooks: Arc::new(Vec::new()),
            concurrency,
            poll_ms,
        }
    }

    /// Set tool config hooks (pre-execution enrichment).
    pub fn with_tool_config_hooks(mut self, hooks: Vec<Arc<dyn ToolConfigHook>>) -> Self {
        self.tool_config_hooks = Arc::new(hooks);
        self
    }

    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        eprintln!(
            "[worker-daemon] Starting {} worker(s), poll interval {}ms",
            self.concurrency, self.poll_ms
        );

        // Spawn reaper background task (reuses existing run_reaper with its own shutdown)
        let reaper_pool = self.pool.clone();
        let reaper_shutdown = shutdown.clone();
        let reaper_handle = tokio::spawn(async move {
            crate::reaper::run_reaper(reaper_pool, reaper_shutdown).await;
        });

        // Spawn N worker tasks
        let mut handles = Vec::new();
        for worker_id in 0..self.concurrency {
            let worker = Worker::new(
                self.pool.clone(),
                self.config.clone(),
                self.specs.clone(),
                self.executors.clone(),
                self.tool_config_hooks.clone(),
            );
            let mut worker_shutdown = shutdown.clone();
            let poll_ms = self.poll_ms;

            let handle = tokio::spawn(async move {
                eprintln!("[worker-daemon] Worker {worker_id} started");
                loop {
                    tokio::select! {
                        result = worker.try_execute_one() => {
                            match result {
                                Ok(true) => {
                                    // Processed a job, immediately try next
                                }
                                Ok(false) => {
                                    // No jobs, wait before polling again
                                    tokio::time::sleep(
                                        std::time::Duration::from_millis(poll_ms)
                                    ).await;
                                }
                                Err(e) => {
                                    eprintln!("[worker-daemon] Worker {worker_id} error: {e}");
                                    tokio::time::sleep(
                                        std::time::Duration::from_secs(2)
                                    ).await;
                                }
                            }
                        }
                        _ = worker_shutdown.changed() => {
                            eprintln!("[worker-daemon] Worker {worker_id} shutting down");
                            break;
                        }
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for shutdown signal
        let _ = shutdown.changed().await;
        eprintln!("[worker-daemon] Shutdown signal received, waiting for workers...");

        // Wait for all workers to finish
        for handle in handles {
            let _ = handle.await;
        }

        reaper_handle.abort();
        eprintln!("[worker-daemon] All workers stopped");
    }
}
