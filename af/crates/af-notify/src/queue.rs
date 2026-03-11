use sqlx::PgPool;
use std::path::Path;

const STALE_PROCESSING_MINUTES: i32 = 2;
const BATCH_SIZE: i64 = 20;

/// Process pending notification queue items. Called from `af tick` as fallback
/// for PgListener-based delivery.
pub async fn process_notification_queue(
    pool: &PgPool,
    storage_root: &Path,
) -> anyhow::Result<u64> {
    // Recover stale processing items first
    let stale = af_db::notifications::recover_stale(pool, STALE_PROCESSING_MINUTES).await?;
    if !stale.is_empty() {
        let retried = stale.iter().filter(|r| r.status == "pending").count();
        let failed = stale.iter().filter(|r| r.status == "failed").count();
        if retried > 0 {
            tracing::info!(count = retried, "recovered stale notification items");
        }
        if failed > 0 {
            tracing::warn!(
                count = failed,
                "notification items permanently failed after stale recovery"
            );
        }
    }

    let pending = af_db::notifications::list_pending(pool, BATCH_SIZE).await?;
    let mut completed = 0u64;

    for item in &pending {
        let claimed = match af_db::notifications::claim(pool, item.id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "failed to claim notification item");
                continue;
            }
        };
        if !claimed {
            continue; // already claimed by another worker
        }

        let channel = match af_db::notifications::get_channel(pool, item.channel_id).await {
            Ok(Some(ch)) if ch.enabled => ch,
            Ok(Some(_)) => {
                if let Err(e) = af_db::notifications::fail(pool, item.id, "channel is disabled").await {
                    tracing::warn!(id = %item.id, error = %e, "failed to mark notification as failed");
                }
                continue;
            }
            Ok(None) => {
                if let Err(e) = af_db::notifications::fail(pool, item.id, "channel not found").await {
                    tracing::warn!(id = %item.id, error = %e, "failed to mark notification as failed");
                }
                continue;
            }
            Err(e) => {
                tracing::warn!(id = %item.id, error = %e, "failed to fetch channel for notification");
                continue;
            }
        };

        match crate::channels::deliver(pool, storage_root, item, &channel).await {
            Ok(()) => {
                if let Err(e) = af_db::notifications::complete(pool, item.id).await {
                    tracing::warn!(id = %item.id, error = %e, "failed to mark notification as completed");
                } else {
                    completed += 1;
                }
                tracing::info!(
                    channel = %channel.name,
                    channel_type = %channel.channel_type,
                    subject = %item.subject,
                    "notification delivered"
                );
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let is_perm = crate::channels::is_permanent(&e);
                tracing::warn!(
                    channel = %channel.name,
                    error = %msg,
                    attempt = item.attempt_count + 1,
                    permanent = is_perm,
                    "notification delivery failed"
                );
                let fail_result = if is_perm {
                    af_db::notifications::fail_permanent(pool, item.id, &msg).await
                } else {
                    af_db::notifications::fail(pool, item.id, &msg).await
                };
                if let Err(e) = fail_result {
                    tracing::warn!(id = %item.id, error = %e, "failed to record notification failure");
                }
            }
        }
    }

    Ok(completed)
}

/// Process a single notification by ID (used by PgListener for near-real-time delivery).
pub async fn process_single(pool: &PgPool, storage_root: &Path, id: uuid::Uuid) -> anyhow::Result<()> {
    if !af_db::notifications::claim(pool, id).await? {
        return Ok(()); // already claimed
    }

    // Fetch the queue item by ID after claiming
    let item = match af_db::notifications::get_queue_item(pool, id).await? {
        Some(i) => i,
        None => return Ok(()), // claim raced with another worker
    };

    let channel = match af_db::notifications::get_channel(pool, item.channel_id).await? {
        Some(ch) if ch.enabled => ch,
        Some(_) => {
            af_db::notifications::fail(pool, item.id, "channel is disabled").await?;
            return Ok(());
        }
        None => {
            af_db::notifications::fail(pool, item.id, "channel not found").await?;
            return Ok(());
        }
    };

    match crate::channels::deliver(pool, storage_root, &item, &channel).await {
        Ok(()) => {
            af_db::notifications::complete(pool, item.id).await?;
            tracing::info!(
                channel = %channel.name,
                channel_type = %channel.channel_type,
                subject = %item.subject,
                "notification delivered (real-time)"
            );
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let is_perm = crate::channels::is_permanent(&e);
            tracing::warn!(
                channel = %channel.name,
                error = %msg,
                permanent = is_perm,
                "notification delivery failed (real-time)"
            );
            if is_perm {
                af_db::notifications::fail_permanent(pool, item.id, &msg).await?;
            } else {
                af_db::notifications::fail(pool, item.id, &msg).await?;
            }
        }
    }

    Ok(())
}
