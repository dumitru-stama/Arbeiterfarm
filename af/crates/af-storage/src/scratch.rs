use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

/// Create a scratch directory for a tool run.
pub async fn create_scratch_dir(scratch_root: &Path, tool_run_id: Uuid) -> Result<PathBuf, std::io::Error> {
    let dir = scratch_root.join(tool_run_id.to_string());
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir)
}

/// Clean up a scratch directory after tool execution.
pub async fn cleanup_scratch_dir(dir: &Path) -> Result<(), std::io::Error> {
    if dir.exists() {
        tokio::fs::remove_dir_all(dir).await?;
    }
    Ok(())
}

/// Remove scratch directories older than `max_age`. Returns count of dirs removed.
/// Designed for periodic tick cleanup of orphaned dirs left by crashed workers.
pub async fn cleanup_stale_dirs(scratch_root: &Path, max_age: Duration) -> Result<u64, std::io::Error> {
    let mut count = 0u64;
    let mut entries = match tokio::fs::read_dir(scratch_root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let cutoff = std::time::SystemTime::now() - max_age;
    while let Some(entry) = entries.next_entry().await? {
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue, // skip entries we can't stat
        };
        if !meta.is_dir() {
            continue;
        }
        let modified = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if modified < cutoff {
            if let Err(e) = tokio::fs::remove_dir_all(entry.path()).await {
                eprintln!("[scratch] WARNING: failed to remove stale dir {}: {e}", entry.path().display());
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}
