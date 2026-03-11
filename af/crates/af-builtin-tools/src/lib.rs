pub mod chunking;
pub mod embed;
pub mod embed_queue;
pub mod envelope;
pub mod url_ingest;
pub mod file_grep;
pub mod file_hexdump;
pub mod file_info;
pub mod file_read_range;
pub mod file_strings;
pub mod specs;

use af_core::{SpawnConfig, ToolExecutorRegistry, ToolSpecRegistry};
use af_llm::EmbeddingBackend;
use sqlx::PgPool;
use std::path::Path;
use std::sync::Arc;

/// Register all builtin file tool specs (not including embed tools).
pub fn declare(registry: &mut ToolSpecRegistry) {
    for spec in specs::all_specs() {
        registry.register(spec).expect("failed to register builtin tool spec");
    }
}

/// Register embed tool specs. Called separately because embed tools
/// require an embedding backend + DB, so they are only declared when available.
pub fn declare_embed(registry: &mut ToolSpecRegistry) {
    for spec in specs::embed_specs() {
        registry.register(spec).expect("failed to register embed tool spec");
    }
}

/// Register OOP executor for all builtin file tools.
pub fn wire(registry: &mut ToolExecutorRegistry, executor_path: &Path) {
    let config = SpawnConfig {
        binary_path: executor_path.to_path_buf(),
        protocol_version: 1,
        supported_tools: vec![
            ("file.info".into(), 1),
            ("file.read_range".into(), 1),
            ("file.strings".into(), 1),
            ("file.hexdump".into(), 1),
            ("file.grep".into(), 1),
        ],
        context_extra: serde_json::Value::Null,
    };
    registry
        .register_oop(config)
        .expect("failed to register builtin OOP executors");
}

/// Wire embedding tool executors (Trusted, in-process). Requires EmbeddingBackend + DB pool.
pub fn wire_embed(
    registry: &mut ToolExecutorRegistry,
    embedding_backend: Arc<dyn EmbeddingBackend>,
    pool: PgPool,
) {
    registry
        .register(Box::new(embed::EmbedTextExecutor {
            backend: Arc::clone(&embedding_backend),
            pool: pool.clone(),
        }))
        .expect("failed to register embed.text executor");

    registry
        .register(Box::new(embed::EmbedSearchExecutor {
            backend: Arc::clone(&embedding_backend),
            pool: pool.clone(),
        }))
        .expect("failed to register embed.search executor");

    registry
        .register(Box::new(embed::EmbedBatchExecutor {
            backend: Arc::clone(&embedding_backend),
            pool: pool.clone(),
        }))
        .expect("failed to register embed.batch executor");

    registry
        .register(Box::new(embed::EmbedListExecutor {
            pool,
        }))
        .expect("failed to register embed.list executor");
}
