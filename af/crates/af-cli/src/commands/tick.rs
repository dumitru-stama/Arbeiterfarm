use crate::CliConfig;
use std::sync::Arc;
use std::time::Duration;

/// Fire all due tick hooks once and exit.
/// Designed to be called from cron every minute:
///   * * * * * /usr/local/bin/af tick
pub async fn handle(config: &CliConfig) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;

    // Purge expired web fetch cache entries (cheap, idempotent)
    match af_db::web_fetch::cache_purge_expired(&pool).await {
        Ok(n) if n > 0 => println!("[tick] purged {n} expired cache entries"),
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: cache purge failed: {e}"),
    }

    // Scratch dir cleanup — remove orphaned dirs from crashed workers (> 2 hours old)
    let scratch_root = &config.core_config.scratch_root;
    match af_storage::scratch::cleanup_stale_dirs(scratch_root, Duration::from_secs(7200)).await {
        Ok(n) if n > 0 => println!("[tick] removed {n} stale scratch dir(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: scratch cleanup failed: {e}"),
    }

    // Blob garbage collection — two-phase: find candidates, then delete file + DB row
    // per blob with a re-check to avoid TOCTOU races with concurrent store_blob.
    match af_db::blobs::find_unreferenced_blobs(&pool).await {
        Ok(candidates) if !candidates.is_empty() => {
            let mut gc_count = 0u64;
            let mut skip_count = 0u64;
            let mut file_errs = 0u64;
            for (sha, path) in &candidates {
                // Phase 1: delete file (idempotent, warns on ENOENT)
                if let Err(e) = af_storage::blob_store::delete_blob_file(std::path::Path::new(path)).await {
                    eprintln!("[tick] WARNING: failed to delete blob file {path}: {e}");
                    file_errs += 1;
                    continue; // skip DB delete if file delete failed
                }
                // Phase 2: atomically delete DB row only if still unreferenced
                match af_db::blobs::delete_blob_if_unreferenced(&pool, sha).await {
                    Ok(true) => gc_count += 1,
                    Ok(false) => skip_count += 1, // re-referenced between scan and delete
                    Err(e) => eprintln!("[tick] WARNING: blob DB delete failed for {sha}: {e}"),
                }
            }
            if gc_count > 0 || file_errs > 0 || skip_count > 0 {
                println!(
                    "[tick] blob GC: {gc_count} removed, {skip_count} re-referenced, {file_errs} file error(s)"
                );
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: blob GC failed: {e}"),
    }

    // Thread/message TTL — purge threads past their project's retention setting
    match af_db::threads::purge_expired_threads(&pool).await {
        Ok(n) if n > 0 => println!("[tick] purged {n} expired thread(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: thread purge failed: {e}"),
    }

    // Process due scheduled emails
    match af_email::scheduler::process_due_emails(&pool).await {
        Ok(n) if n > 0 => println!("[tick] sent {n} scheduled email(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: scheduled email processing failed: {e}"),
    }

    // Process pending URL ingestion queue items
    match af_builtin_tools::url_ingest::process_url_queue(
        &pool, &config.core_config.storage_root,
    ).await {
        Ok(n) if n > 0 => println!("[tick] ingested {n} URL(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: URL ingestion failed: {e}"),
    }

    // Process pending embedding queue items
    if let Some(eb) = crate::bootstrap::build_embedding_backend() {
        match af_builtin_tools::embed_queue::process_embed_queue(&pool, &*eb).await {
            Ok(n) if n > 0 => println!("[tick] embedded {n} queued chunk set(s)"),
            Ok(_) => {}
            Err(e) => eprintln!("[tick] WARNING: embed queue processing failed: {e}"),
        }
    }

    // Process pending notification queue items (fallback for PgListener)
    match af_notify::queue::process_notification_queue(
        &pool, &config.core_config.storage_root,
    ).await {
        Ok(n) if n > 0 => println!("[tick] delivered {n} notification(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("[tick] WARNING: notification delivery failed: {e}"),
    }

    // Quick check: any due hooks at all? Exit fast if none.
    let due = af_db::project_hooks::list_due_tick_hooks(&pool).await?;
    if due.is_empty() {
        return Ok(());
    }

    let router = config
        .router
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no LLM backends configured — cannot fire tick hooks"))?
        .clone();

    let state = Arc::new(af_api::AppState {
        pool,
        specs: config.specs.clone(),
        executors: config.executors.clone(),
        evidence_resolvers: config.evidence_resolvers.clone(),
        post_tool_hook: config.post_tool_hook.clone(),
        core_config: config.core_config.clone(),
        agent_configs: config.agent_configs.clone(),
        router,
        upload_max_bytes: 0,
        rate_limiter: None,
        cors_origin: None,
        ghidra_cache_dir: config.ghidra_cache_dir.clone(),
        source_map: config.source_map.clone(),
        security_config: af_api::SecurityConfig {
            sandbox_available: false,
            sandbox_enforced: false,
            tls_enabled: false,
        },
        stream_tracker: af_api::ActiveStreamTracker::new(0),
        compaction_threshold: config.compaction.threshold,
        summarization_backend: None,
    });

    println!("[tick] {} due hook(s), firing...", due.len());
    af_api::hooks::fire_tick_hooks_blocking(&state).await;
    println!("[tick] done");

    Ok(())
}
