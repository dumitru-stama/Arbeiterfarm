//! Background embedding queue processor.
//!
//! Called from `af tick` to process pending embed_queue items.
//! Reads chunks.json artifacts, embeds via the configured backend,
//! stores vectors in pgvector.

use crate::chunking::Chunk;
use af_llm::EmbeddingBackend;
use sqlx::PgPool;

const BATCH_SIZE: usize = 100;

/// How long (minutes) before a 'processing' item is considered stuck.
/// If a worker hasn't updated progress in this many minutes, it's likely crashed.
const STALE_PROCESSING_MINUTES: i32 = 2;

/// Process pending embed queue items. Returns the number of items completed.
///
/// First recovers any items stuck in 'processing' from crashed workers,
/// then processes pending items. Items that exceed max_attempts (default 5)
/// are permanently marked as 'failed'.
pub async fn process_embed_queue(
    pool: &PgPool,
    backend: &dyn EmbeddingBackend,
) -> anyhow::Result<u64> {
    // Recover stale items stuck in 'processing' from crashed tick workers.
    // This makes them available for reprocessing (or permanently fails them).
    let recovered = af_db::embed_queue::recover_stale(pool, STALE_PROCESSING_MINUTES).await?;
    if !recovered.is_empty() {
        let retried = recovered.iter().filter(|r| r.status == "pending").count();
        let failed = recovered.iter().filter(|r| r.status == "failed").count();
        if retried > 0 {
            eprintln!("[embed-queue] recovered {retried} stale item(s) for retry");
        }
        if failed > 0 {
            eprintln!(
                "[embed-queue] permanently failed {failed} item(s) after exhausting retries"
            );
        }
    }

    let pending = af_db::embed_queue::list_pending(pool, 10).await?;
    let mut completed = 0u64;

    for item in &pending {
        // Atomic claim — skip if already taken by another worker
        if !af_db::embed_queue::claim(pool, item.id).await? {
            continue;
        }

        match process_single_item(pool, backend, item).await {
            Ok(()) => {
                af_db::embed_queue::complete(pool, item.id).await?;
                completed += 1;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                eprintln!(
                    "[embed-queue] failed to process item {}: {msg}",
                    item.id
                );
                af_db::embed_queue::fail(pool, item.id, &msg).await?;
            }
        }
    }

    Ok(completed)
}

async fn process_single_item(
    pool: &PgPool,
    backend: &dyn EmbeddingBackend,
    item: &af_db::embed_queue::EmbedQueueRow,
) -> anyhow::Result<()> {
    // Load the chunks.json artifact
    let artifact = af_db::artifacts::get_artifact(pool, item.chunks_artifact_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("chunks artifact {} not found", item.chunks_artifact_id))?;

    let blob = af_db::blobs::get_blob(pool, &artifact.sha256)
        .await?
        .ok_or_else(|| anyhow::anyhow!("blob {} not found", artifact.sha256))?;

    // Read and parse chunks
    let data = std::fs::read(&blob.storage_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", blob.storage_path))?;

    let chunks: Vec<Chunk> = serde_json::from_slice(&data)
        .map_err(|e| anyhow::anyhow!("failed to parse chunks.json: {e}"))?;

    let total = chunks.len() as i32;
    af_db::embed_queue::update_progress(pool, item.id, item.chunks_embedded, Some(total)).await?;

    // Skip already-embedded chunks (resume from partial progress)
    let skip = item.chunks_embedded as usize;
    let remaining: Vec<&Chunk> = chunks.iter().skip(skip).collect();

    if remaining.is_empty() {
        return Ok(());
    }

    // The artifact_id stored in embeddings should be the source document,
    // so embed.search traces back to the original file (not the chunks.json).
    let embed_artifact_id = item.source_artifact_id;

    let metadata = serde_json::json!({
        "source": "embed_queue",
        "tool": item.tool_name,
    });

    let mut embedded_so_far = skip as i32;

    // Process in batches
    for batch in remaining.chunks(BATCH_SIZE) {
        let texts: Vec<String> = batch.iter().map(|c| c.text.clone()).collect();

        let embeddings = backend.embed(texts).await
            .map_err(|e| anyhow::anyhow!("embedding failed: {e}"))?;

        if embeddings.len() != batch.len() {
            return Err(anyhow::anyhow!(
                "expected {} embeddings, got {}",
                batch.len(),
                embeddings.len()
            ));
        }

        // Validate dimensions on first embedding of first batch
        if embedded_so_far == skip as i32 {
            if let Some(first) = embeddings.first() {
                let expected = backend.dimensions() as usize;
                if first.len() != expected {
                    return Err(anyhow::anyhow!(
                        "embedding model returned {} dimensions, expected {}",
                        first.len(),
                        expected
                    ));
                }
            }
        }

        // Insert each embedding in a transaction
        let mut tx = pool.begin().await?;
        for (chunk, embedding) in batch.iter().zip(embeddings.iter()) {
            af_db::embeddings::insert_embedding(
                &mut *tx,
                item.project_id,
                embed_artifact_id,
                &chunk.label,
                &chunk.text,
                backend.name(),
                backend.dimensions() as i32,
                embedding,
                &metadata,
            )
            .await
            .map_err(|e| anyhow::anyhow!("insert embedding failed: {e}"))?;
        }
        tx.commit().await?;

        embedded_so_far += batch.len() as i32;
        af_db::embed_queue::update_progress(pool, item.id, embedded_so_far, None).await?;
    }

    Ok(())
}
