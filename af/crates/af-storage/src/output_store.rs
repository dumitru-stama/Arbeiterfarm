use async_trait::async_trait;
use af_core::{OutputStore, ToolError};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use uuid::Uuid;

/// FileOutputStore — stores blobs, creates artifacts with source_tool_run_id,
/// links via tool_run_artifacts. Enforces per-tool quotas.
pub struct FileOutputStore {
    pool: PgPool,
    storage_root: PathBuf,
    project_id: Uuid,
    tool_run_id: Uuid,
    // Quota tracking
    max_output_bytes: u64,
    max_produced_artifacts: u32,
    bytes_written: AtomicU64,
    artifacts_produced: AtomicU32,
}

impl FileOutputStore {
    pub fn new(
        pool: PgPool,
        storage_root: PathBuf,
        project_id: Uuid,
        tool_run_id: Uuid,
        max_output_bytes: u64,
        max_produced_artifacts: u32,
    ) -> Self {
        Self {
            pool,
            storage_root,
            project_id,
            tool_run_id,
            max_output_bytes,
            max_produced_artifacts,
            bytes_written: AtomicU64::new(0),
            artifacts_produced: AtomicU32::new(0),
        }
    }

    /// Shared quota check + blob store + link logic.
    async fn store_inner(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
        metadata: Option<&serde_json::Value>,
        description: Option<&str>,
    ) -> Result<Uuid, ToolError> {
        // Check quotas
        let new_bytes = self.bytes_written.load(Ordering::Relaxed) + data.len() as u64;
        if new_bytes > self.max_output_bytes {
            return Err(ToolError {
                code: "output_quota_exceeded".to_string(),
                message: format!(
                    "output would exceed quota: {} + {} > {}",
                    self.bytes_written.load(Ordering::Relaxed),
                    data.len(),
                    self.max_output_bytes
                ),
                retryable: false,
                details: serde_json::Value::Null,
            });
        }

        let new_count = self.artifacts_produced.load(Ordering::Relaxed) + 1;
        if new_count > self.max_produced_artifacts {
            return Err(ToolError {
                code: "artifact_count_exceeded".to_string(),
                message: format!(
                    "would exceed max produced artifacts: {}",
                    self.max_produced_artifacts
                ),
                retryable: false,
                details: serde_json::Value::Null,
            });
        }

        // Store blob
        let (sha256, _path) =
            crate::blob_store::store_blob(&self.pool, &self.storage_root, data)
                .await
                .map_err(|e| ToolError {
                    code: "storage_error".to_string(),
                    message: e.to_string(),
                    retryable: true,
                    details: serde_json::Value::Null,
                })?;

        // Create artifact
        let row = if let Some(desc) = description {
            af_db::artifacts::create_artifact_with_description(
                &self.pool,
                self.project_id,
                &sha256,
                filename,
                mime_type,
                Some(self.tool_run_id),
                desc,
            )
            .await
        } else if let Some(meta) = metadata {
            af_db::artifacts::create_artifact_with_metadata(
                &self.pool,
                self.project_id,
                &sha256,
                filename,
                mime_type,
                Some(self.tool_run_id),
                meta,
            )
            .await
        } else {
            af_db::artifacts::create_artifact(
                &self.pool,
                self.project_id,
                &sha256,
                filename,
                mime_type,
                Some(self.tool_run_id),
            )
            .await
        }
        .map_err(|e| ToolError {
            code: "db_error".to_string(),
            message: e.to_string(),
            retryable: true,
            details: serde_json::Value::Null,
        })?;

        // Link artifact to tool run
        af_db::tool_run_artifacts::link_artifact(
            &self.pool,
            self.tool_run_id,
            row.id,
            "output",
        )
        .await
        .map_err(|e| ToolError {
            code: "db_error".to_string(),
            message: e.to_string(),
            retryable: true,
            details: serde_json::Value::Null,
        })?;

        // Update counters
        self.bytes_written
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        self.artifacts_produced.fetch_add(1, Ordering::Relaxed);

        Ok(row.id)
    }
}

#[async_trait]
impl OutputStore for FileOutputStore {
    async fn store(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
    ) -> Result<Uuid, ToolError> {
        self.store_inner(filename, data, mime_type, None, None).await
    }

    async fn store_with_metadata(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<Uuid, ToolError> {
        self.store_inner(filename, data, mime_type, Some(&metadata), None).await
    }

    async fn store_with_description(
        &self,
        filename: &str,
        data: &[u8],
        mime_type: Option<&str>,
        description: &str,
    ) -> Result<Uuid, ToolError> {
        self.store_inner(filename, data, mime_type, None, Some(description)).await
    }
}
