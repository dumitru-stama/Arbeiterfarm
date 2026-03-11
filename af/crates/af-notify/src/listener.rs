use sqlx::PgPool;
use std::path::PathBuf;
use uuid::Uuid;

/// Run a PgListener for near-real-time notification delivery.
/// Spawned as a background task in `af serve`.
pub async fn run_notification_listener(
    pool: PgPool,
    storage_root: PathBuf,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut listener = match sqlx::postgres::PgListener::connect_with(&pool).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to create PgListener for notifications: {e}");
            return;
        }
    };

    if let Err(e) = listener.listen("notification_queue").await {
        tracing::error!("failed to LISTEN on notification_queue: {e}");
        return;
    }

    tracing::info!("notification listener started (LISTEN notification_queue)");

    loop {
        tokio::select! {
            notification = listener.recv() => {
                match notification {
                    Ok(notif) => {
                        let payload = notif.payload();
                        let id = match Uuid::parse_str(payload) {
                            Ok(id) => id,
                            Err(_) => {
                                tracing::warn!(payload = %payload, "invalid UUID in notification_queue payload");
                                continue;
                            }
                        };

                        if let Err(e) = crate::queue::process_single(&pool, &storage_root, id).await {
                            tracing::warn!(
                                id = %id,
                                error = %e,
                                "failed to process notification from PgListener"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("PgListener notification_queue error (will reconnect): {e}");
                        // sqlx PgListener auto-reconnects on the next recv()
                    }
                }
            }
            _ = shutdown.changed() => {
                tracing::info!("notification listener shutting down");
                break;
            }
        }
    }
}
