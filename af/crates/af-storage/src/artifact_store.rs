use crate::blob_store;
use sqlx::PgPool;
use std::path::Path;
use uuid::Uuid;

/// Ingest a file as an artifact: store blob + create artifact row.
/// Returns the artifact ID.
pub async fn ingest_artifact(
    pool: &PgPool,
    storage_root: &Path,
    project_id: Uuid,
    filename: &str,
    data: &[u8],
    mime_type: Option<&str>,
    source_tool_run_id: Option<Uuid>,
) -> Result<Uuid, IngestError> {
    let (sha256, _path) = blob_store::store_blob(pool, storage_root, data)
        .await
        .map_err(|e| IngestError::Storage(e.to_string()))?;

    let row = af_db::artifacts::create_artifact(
        pool,
        project_id,
        &sha256,
        filename,
        mime_type,
        source_tool_run_id,
    )
    .await
    .map_err(|e| IngestError::Db(e.to_string()))?;

    Ok(row.id)
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("DB error: {0}")]
    Db(String),
}
