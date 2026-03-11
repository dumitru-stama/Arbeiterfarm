use sqlx::PgPool;
use std::time::Duration;

const MAX_ATTEMPTS: i32 = 3;
const REAPER_INTERVAL_SECS: u64 = 30;

/// Background task: reclaim expired leases, increment attempt.
pub async fn run_reaper(pool: PgPool, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(REAPER_INTERVAL_SECS));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                match af_db::tool_runs::reclaim_expired(&pool, MAX_ATTEMPTS).await {
                    Ok(n) if n > 0 => {
                        tracing::info!("reaper reclaimed {n} expired tool runs");
                    }
                    Err(e) => {
                        tracing::warn!("reaper reclaim error: {e}");
                    }
                    _ => {}
                }
                // Permanently fail runs that exceeded max retry attempts
                match af_db::tool_runs::fail_exhausted(&pool, MAX_ATTEMPTS).await {
                    Ok(n) if n > 0 => {
                        tracing::warn!("reaper permanently failed {n} exhausted tool runs");
                    }
                    Err(e) => {
                        tracing::warn!("reaper fail_exhausted error: {e}");
                    }
                    _ => {}
                }
            }
            _ = shutdown.changed() => {
                tracing::info!("reaper shutting down");
                return;
            }
        }
    }
}
