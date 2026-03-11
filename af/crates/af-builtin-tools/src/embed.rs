use async_trait::async_trait;
use af_core::{ToolContext, ToolError, ToolExecutor, ToolOutputKind, ToolResult};
use af_llm::EmbeddingBackend;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

fn tool_err(code: &str, msg: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: msg,
        retryable: false,
        details: Value::Null,
    }
}

/// Verify that the returned embedding has the expected number of dimensions.
fn check_dims(embedding: &[f32], expected: u32) -> Result<(), ToolError> {
    if embedding.len() != expected as usize {
        return Err(tool_err(
            "dimension_mismatch",
            format!(
                "embedding model returned {} dimensions, expected {}. \
                 Check AF_EMBEDDING_DIMENSIONS or model configuration.",
                embedding.len(),
                expected
            ),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// embed.text — generate embedding and store in DB
// ---------------------------------------------------------------------------

pub struct EmbedTextExecutor {
    pub backend: Arc<dyn EmbeddingBackend>,
    pub pool: PgPool,
}

#[async_trait]
impl ToolExecutor for EmbedTextExecutor {
    fn tool_name(&self) -> &str {
        "embed.text"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("'text' is required")?;
        if text.trim().is_empty() {
            return Err("'text' must not be empty".into());
        }
        input
            .get("label")
            .and_then(|v| v.as_str())
            .ok_or("'label' is required")?;
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let text = input["text"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'text' is required".into()))?
            .to_string();
        let label = input["label"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'label' is required".into()))?;
        let artifact_id = input
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        let metadata = input
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let embeddings = self
            .backend
            .embed(vec![text.clone()])
            .await
            .map_err(|e| tool_err("embedding_error", format!("embedding failed: {e}")))?;

        let embedding = embeddings
            .into_iter()
            .next()
            .ok_or_else(|| tool_err("embedding_error", "no embedding returned".into()))?;

        check_dims(&embedding, self.backend.dimensions())?;

        let row = af_db::embeddings::insert_embedding(
            &self.pool,
            ctx.project_id,
            artifact_id,
            label,
            &text,
            self.backend.name(),
            self.backend.dimensions() as i32,
            &embedding,
            &metadata,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("insert embedding failed: {e}")))?;

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "status": "embedded",
                "embedding_id": row.id,
                "label": row.label,
                "model": row.model,
                "dimensions": row.dimensions,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// embed.search — search for similar embeddings
// ---------------------------------------------------------------------------

pub struct EmbedSearchExecutor {
    pub backend: Arc<dyn EmbeddingBackend>,
    pub pool: PgPool,
}

#[async_trait]
impl ToolExecutor for EmbedSearchExecutor {
    fn tool_name(&self) -> &str {
        "embed.search"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("'query' is required")?;
        if query.trim().is_empty() {
            return Err("'query' must not be empty".into());
        }
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| tool_err("invalid_input", "'query' is required".into()))?
            .to_string();
        let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
        let artifact_id_filter = input
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let embeddings = self
            .backend
            .embed(vec![query])
            .await
            .map_err(|e| tool_err("embedding_error", format!("query embedding failed: {e}")))?;

        let query_embedding = embeddings
            .into_iter()
            .next()
            .ok_or_else(|| tool_err("embedding_error", "no embedding returned".into()))?;

        check_dims(&query_embedding, self.backend.dimensions())?;

        let results = af_db::embeddings::search_similar(
            &self.pool,
            ctx.project_id,
            self.backend.name(),
            self.backend.dimensions() as i32,
            &query_embedding,
            limit,
            artifact_id_filter,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("search failed: {e}")))?;

        let results_json: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "label": r.label,
                    "content": r.content,
                    "similarity": r.similarity,
                    "artifact_id": r.artifact_id,
                    "metadata": r.metadata,
                })
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "model": self.backend.name(),
                "count": results_json.len(),
                "results": results_json,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// embed.batch — batch-embed multiple texts
// ---------------------------------------------------------------------------

pub struct EmbedBatchExecutor {
    pub backend: Arc<dyn EmbeddingBackend>,
    pub pool: PgPool,
}

#[async_trait]
impl ToolExecutor for EmbedBatchExecutor {
    fn tool_name(&self) -> &str {
        "embed.batch"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    fn validate(&self, _ctx: &ToolContext, input: &Value) -> Result<(), String> {
        let items = input
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or("'items' array is required")?;
        if items.is_empty() {
            return Err("'items' must not be empty".into());
        }
        if items.len() > 100 {
            return Err("'items' must contain at most 100 entries".into());
        }
        for (i, item) in items.iter().enumerate() {
            if item.get("text").and_then(|v| v.as_str()).is_none() {
                return Err(format!("items[{i}].text is required"));
            }
            if item.get("label").and_then(|v| v.as_str()).is_none() {
                return Err(format!("items[{i}].label is required"));
            }
        }
        Ok(())
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let items = input["items"]
            .as_array()
            .ok_or_else(|| tool_err("invalid_input", "'items' array is required".into()))?;

        let texts: Vec<String> = items
            .iter()
            .map(|item| item["text"].as_str().unwrap_or("").to_string())
            .collect();

        let embeddings = self
            .backend
            .embed(texts)
            .await
            .map_err(|e| tool_err("embedding_error", format!("batch embedding failed: {e}")))?;

        if embeddings.len() != items.len() {
            return Err(tool_err(
                "embedding_error",
                format!(
                    "expected {} embeddings, got {}",
                    items.len(),
                    embeddings.len()
                ),
            ));
        }

        // Validate dimensions on first embedding (all come from the same model call)
        if let Some(first) = embeddings.first() {
            check_dims(first, self.backend.dimensions())?;
        }

        // Use a transaction for atomicity and reduced round-trip overhead
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| tool_err("db_error", format!("begin transaction failed: {e}")))?;

        let mut inserted = 0u64;
        let mut errors = Vec::new();

        for (i, (item, embedding)) in items.iter().zip(embeddings.iter()).enumerate() {
            let label = item["label"].as_str().unwrap_or("");
            let text = item["text"].as_str().unwrap_or("");
            let artifact_id = item
                .get("artifact_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            let metadata = item
                .get("metadata")
                .cloned()
                .unwrap_or_else(|| json!({}));

            match af_db::embeddings::insert_embedding(
                &mut *tx,
                ctx.project_id,
                artifact_id,
                label,
                text,
                self.backend.name(),
                self.backend.dimensions() as i32,
                embedding,
                &metadata,
            )
            .await
            {
                Ok(_) => inserted += 1,
                Err(e) => errors.push(format!("items[{i}] ({label}): {e}")),
            }
        }

        tx.commit()
            .await
            .map_err(|e| tool_err("db_error", format!("commit transaction failed: {e}")))?;

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "status": "batch_complete",
                "model": self.backend.name(),
                "dimensions": self.backend.dimensions(),
                "inserted": inserted,
                "total": items.len(),
                "errors": errors,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// embed.list — list embeddings for a project/artifact
// ---------------------------------------------------------------------------

pub struct EmbedListExecutor {
    pub pool: PgPool,
}

#[async_trait]
impl ToolExecutor for EmbedListExecutor {
    fn tool_name(&self) -> &str {
        "embed.list"
    }

    fn tool_version(&self) -> u32 {
        1
    }

    async fn execute(&self, ctx: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        let artifact_id = input
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        let model = input.get("model").and_then(|v| v.as_str());
        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(500);

        let rows = af_db::embeddings::list_embeddings(
            &self.pool,
            ctx.project_id,
            artifact_id,
            model,
            limit,
        )
        .await
        .map_err(|e| tool_err("db_error", format!("list embeddings failed: {e}")))?;

        let entries: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "label": r.label,
                    "model": r.model,
                    "dimensions": r.dimensions,
                    "artifact_id": r.artifact_id,
                    "metadata": r.metadata,
                    "created_at": r.created_at.to_rfc3339(),
                })
            })
            .collect();

        Ok(ToolResult {
            kind: ToolOutputKind::InlineJson,
            output_json: json!({
                "count": entries.len(),
                "embeddings": entries,
            }),
            stdout: None,
            stderr: None,
            produced_artifacts: vec![],
            primary_artifact: None,
            evidence: vec![],
        })
    }
}
