use crate::app::WorkerCommand;
use crate::CliConfig;
use af_jobs::daemon::WorkerDaemon;
use tokio::sync::watch;

pub async fn handle(config: &CliConfig, cmd: WorkerCommand) -> anyhow::Result<()> {
    use crate::app::WorkerAction;

    match cmd.action {
        WorkerAction::Start { concurrency, poll_ms } => {
            let pool = crate::get_pool_from(&config.pool).await?;

            let daemon = WorkerDaemon::new(
                pool,
                config.core_config.clone(),
                config.specs.clone(),
                config.executors.clone(),
                concurrency as usize,
                poll_ms,
            )
            .with_tool_config_hooks(config.tool_config_hooks.clone());

            let (shutdown_tx, shutdown_rx) = watch::channel(false);

            // Handle Ctrl-C for graceful shutdown
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                eprintln!("\n[worker] Ctrl-C received, shutting down...");
                shutdown_tx.send(true).ok();
            });

            daemon.run(shutdown_rx).await;
        }
    }
    Ok(())
}
