use af_core::{ArtifactRef, CoreConfig, ToolContext, UtcClock};
use af_db::tool_runs::ToolRunRow;
use af_storage::output_store::FileOutputStore;
use af_storage::scratch;
use sqlx::PgPool;
use uuid::Uuid;

/// Build a ToolContext for a claimed tool run.
pub async fn build_context(
    pool: &PgPool,
    config: &CoreConfig,
    run: &ToolRunRow,
    max_output_bytes: u64,
    max_produced_artifacts: u32,
) -> Result<ToolContext, ContextError> {
    // Resolve input artifact IDs from the tool run's input_json
    let input_artifact_rows =
        af_db::tool_run_artifacts::get_for_run(pool, run.id)
            .await
            .map_err(|e| ContextError::Db(e.to_string()))?;

    let input_ids: Vec<Uuid> = input_artifact_rows
        .iter()
        .filter(|r| r.role == "input")
        .map(|r| r.artifact_id)
        .collect();

    let artifact_rows = if input_ids.is_empty() {
        Vec::new()
    } else {
        af_db::artifacts::get_artifacts_by_ids(pool, &input_ids)
            .await
            .map_err(|e| ContextError::Db(e.to_string()))?
    };

    // Resolve blob paths for each artifact
    let mut artifacts = Vec::new();
    for arow in &artifact_rows {
        let blob = af_db::blobs::get_blob(pool, &arow.sha256)
            .await
            .map_err(|e| ContextError::Db(e.to_string()))?
            .ok_or_else(|| ContextError::BlobMissing(arow.sha256.clone()))?;

        artifacts.push(ArtifactRef {
            id: arow.id,
            sha256: arow.sha256.clone(),
            filename: arow.filename.clone(),
            storage_path: blob.storage_path.into(),
            size_bytes: blob.size_bytes as u64,
            mime_type: arow.mime_type.clone(),
            source_tool_run_id: arow.source_tool_run_id,
        });
    }

    // Create scratch dir
    let scratch_dir = scratch::create_scratch_dir(&config.scratch_root, run.id)
        .await
        .map_err(|e| ContextError::Io(e.to_string()))?;

    // Build OutputStore
    let output_store = FileOutputStore::new(
        pool.clone(),
        config.storage_root.clone(),
        run.project_id,
        run.id,
        max_output_bytes,
        max_produced_artifacts,
    );

    eprintln!("[context-debug] run_id={} tool={} input_artifact_ids={:?} resolved_artifacts={}",
        run.id, run.tool_name, input_ids, artifacts.len());
    for art in &artifacts {
        eprintln!("[context-debug]   artifact: id={} sha256={} file={} path={}",
            art.id, art.sha256, art.filename, art.storage_path.display());
    }

    Ok(ToolContext {
        project_id: run.project_id,
        thread_id: run.thread_id,
        tool_run_id: run.id,
        actor_user_id: run.actor_user_id,
        artifacts,
        scratch_dir,
        output_store: Box::new(output_store),
        clock: Box::new(UtcClock),
        core_config: config.clone(),
        plugin_config: serde_json::Value::Object(Default::default()),
        tool_config: serde_json::Value::Object(Default::default()),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("database error: {0}")]
    Db(String),
    #[error("blob missing for sha256: {0}")]
    BlobMissing(String),
    #[error("IO error: {0}")]
    Io(String),
}
