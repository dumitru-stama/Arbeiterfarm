use chrono::{DateTime, Utc};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

/// Row returned by insert/get operations (without the embedding vector itself).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct EmbeddingRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub artifact_id: Option<Uuid>,
    pub label: String,
    pub content: String,
    pub model: String,
    pub dimensions: i32,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Row returned by similarity search (includes similarity score).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct EmbeddingSearchResult {
    pub id: Uuid,
    pub project_id: Uuid,
    pub artifact_id: Option<Uuid>,
    pub label: String,
    pub content: String,
    pub model: String,
    pub dimensions: i32,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub similarity: f64,
}

/// Upsert an embedding. Uses the NULLS NOT DISTINCT unique index so upserts
/// work correctly even when artifact_id is NULL.
pub async fn insert_embedding(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    artifact_id: Option<Uuid>,
    label: &str,
    content: &str,
    model: &str,
    dimensions: i32,
    embedding: &[f32],
    metadata: &serde_json::Value,
) -> Result<EmbeddingRow, sqlx::Error> {
    let vec = Vector::from(embedding.to_vec());
    sqlx::query_as::<_, EmbeddingRow>(
        "INSERT INTO embeddings (project_id, artifact_id, label, content, model, dimensions, embedding, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (project_id, artifact_id, label, model)
         DO UPDATE SET content = EXCLUDED.content, embedding = EXCLUDED.embedding, metadata = EXCLUDED.metadata
         RETURNING id, project_id, artifact_id, label, content, model, dimensions, metadata, created_at",
    )
    .bind(project_id)
    .bind(artifact_id)
    .bind(label)
    .bind(content)
    .bind(model)
    .bind(dimensions)
    .bind(vec)
    .bind(metadata)
    .fetch_one(db)
    .await
}

/// Search for similar embeddings by cosine similarity.
/// Returns up to `limit` results sorted by descending similarity.
/// Optionally filter by artifact_id.
///
/// The query casts to `vector(N)` to match the HNSW expression index,
/// ensuring index-accelerated search instead of sequential scan.
/// `dimensions` is an i32 from the backend config (not user input).
pub async fn search_similar(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    model: &str,
    dimensions: i32,
    query_embedding: &[f32],
    limit: i64,
    artifact_id_filter: Option<Uuid>,
) -> Result<Vec<EmbeddingSearchResult>, sqlx::Error> {
    let vec = Vector::from(query_embedding.to_vec());
    // Cast both sides to vector(N) so PostgreSQL uses the HNSW expression index.
    // `dimensions` is a trusted i32 from backend config, safe to interpolate.
    // dimensions is inlined as a literal — bind positions: $1=project_id, $2=model,
    // $3=query_vec, $4=artifact_id_filter, $5=limit
    let sql = format!(
        "SELECT id, project_id, artifact_id, label, content, model, dimensions, metadata, created_at,
                1 - (embedding::vector({d}) <=> $3::vector({d})) AS similarity
         FROM embeddings
         WHERE project_id = $1 AND model = $2 AND dimensions = {d}
           AND ($4::uuid IS NULL OR artifact_id = $4)
         ORDER BY embedding::vector({d}) <=> $3::vector({d})
         LIMIT $5",
        d = dimensions
    );
    sqlx::query_as::<_, EmbeddingSearchResult>(&sql)
        .bind(project_id)
        .bind(model)
        .bind(vec)
        .bind(artifact_id_filter)
        .bind(limit)
        .fetch_all(db)
        .await
}

/// List embeddings for a project, optionally filtered by artifact and/or model.
/// Does not return the embedding vectors themselves.
/// Returns at most `limit` rows (default 500).
pub async fn list_embeddings(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    artifact_id: Option<Uuid>,
    model: Option<&str>,
    limit: i64,
) -> Result<Vec<EmbeddingRow>, sqlx::Error> {
    sqlx::query_as::<_, EmbeddingRow>(
        "SELECT id, project_id, artifact_id, label, content, model, dimensions, metadata, created_at
         FROM embeddings
         WHERE project_id = $1
           AND ($2::uuid IS NULL OR artifact_id = $2)
           AND ($3::text IS NULL OR model = $3)
         ORDER BY created_at DESC
         LIMIT $4",
    )
    .bind(project_id)
    .bind(artifact_id)
    .bind(model)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Delete a specific embedding by project, artifact, label, and model.
pub async fn delete_embedding(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    artifact_id: Option<Uuid>,
    label: &str,
    model: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM embeddings
         WHERE project_id = $1
           AND (($2::uuid IS NULL AND artifact_id IS NULL) OR artifact_id = $2)
           AND label = $3
           AND model = $4",
    )
    .bind(project_id)
    .bind(artifact_id)
    .bind(label)
    .bind(model)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
