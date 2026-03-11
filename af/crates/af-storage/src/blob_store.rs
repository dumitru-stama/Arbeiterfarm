use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::path::{Path, PathBuf};

/// Store a blob content-addressed: SHA-256 hash, write to data/{xx}/{yy}/{sha256}, upsert blob row.
/// Returns (sha256, storage_path).
pub async fn store_blob(
    pool: &PgPool,
    storage_root: &Path,
    data: &[u8],
) -> Result<(String, PathBuf), StorageError> {
    let hash = hex::encode(Sha256::digest(data));
    let dir = storage_root
        .join("data")
        .join(&hash[0..2])
        .join(&hash[2..4]);
    let file_path = dir.join(&hash);

    // Constant-time write: always write to temp then atomic rename.
    // Eliminates timing oracle that leaks whether a blob already exists.
    let tmp_dir = storage_root.join("tmp");
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| StorageError::Io(e.to_string()))?;
    let tmp_path = tmp_dir.join(format!("blob-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&tmp_path, data)
        .await
        .map_err(|e| StorageError::Io(e.to_string()))?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| StorageError::Io(e.to_string()))?;
    match tokio::fs::rename(&tmp_path, &file_path).await {
        Err(_) if file_path.exists() => {
            // Dedup race: another writer placed the same blob
            let _ = tokio::fs::remove_file(&tmp_path).await;
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(StorageError::Io(e.to_string()));
        }
        Ok(()) => {}
    }

    let storage_path_str = file_path.to_string_lossy().to_string();

    // Upsert blob row
    af_db::blobs::upsert_blob(pool, &hash, data.len() as i64, &storage_path_str)
        .await
        .map_err(|e| StorageError::Db(e.to_string()))?;

    Ok((hash, file_path))
}

/// Store a blob from a temp file that already exists on disk with a precomputed SHA-256 hash.
/// Renames the temp file into the content-addressed path (atomic, same filesystem).
/// The temp file MUST be on the same filesystem as `storage_root` (e.g. under `{storage_root}/tmp/`).
pub async fn store_blob_from_file(
    pool: &PgPool,
    storage_root: &Path,
    temp_path: &Path,
    sha256: &str,
    size_bytes: i64,
) -> Result<(String, PathBuf), StorageError> {
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StorageError::Io(format!(
            "invalid SHA-256 hash: expected 64 hex chars, got '{sha256}'"
        )));
    }
    let dir = storage_root
        .join("data")
        .join(&sha256[0..2])
        .join(&sha256[2..4]);
    let file_path = dir.join(sha256);

    // Constant-time: always attempt rename (no exists() check first).
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| StorageError::Io(e.to_string()))?;
    match tokio::fs::rename(temp_path, &file_path).await {
        Ok(()) => {}
        Err(_) if file_path.exists() => {
            // Dedup race: another writer placed the same blob
            let _ = tokio::fs::remove_file(temp_path).await;
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(temp_path).await;
            return Err(StorageError::Io(e.to_string()));
        }
    }

    let storage_path_str = file_path.to_string_lossy().to_string();

    af_db::blobs::upsert_blob(pool, sha256, size_bytes, &storage_path_str)
        .await
        .map_err(|e| StorageError::Db(e.to_string()))?;

    Ok((sha256.to_string(), file_path))
}

/// Create a temp file under `{storage_root}/tmp/` for streaming uploads.
/// Returns (file, path). Caller is responsible for cleanup on error.
pub async fn create_upload_temp_file(
    storage_root: &Path,
) -> Result<(tokio::fs::File, PathBuf), StorageError> {
    let tmp_dir = storage_root.join("tmp");
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| StorageError::Io(e.to_string()))?;
    let path = tmp_dir.join(format!("upload-{}", uuid::Uuid::new_v4()));
    let file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| StorageError::Io(e.to_string()))?;
    Ok((file, path))
}

/// Delete a blob file from disk. Logs a warning on ENOENT (already gone), propagates other errors.
pub async fn delete_blob_file(path: &Path) -> Result<(), std::io::Error> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("[blob_store] WARNING: blob file already gone: {}", path.display());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("DB error: {0}")]
    Db(String),
}
