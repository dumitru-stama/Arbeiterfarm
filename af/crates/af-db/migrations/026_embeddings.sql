-- Migration 026: Embeddings table (pgvector)
-- Stores text embeddings for vector similarity search.
-- Requires: pgvector extension, PostgreSQL 15+ (for NULLS NOT DISTINCT).

CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS embeddings (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    artifact_id UUID REFERENCES artifacts(id) ON DELETE CASCADE,
    label       TEXT NOT NULL,
    content     TEXT NOT NULL,
    model       TEXT NOT NULL,
    dimensions  INT NOT NULL,
    embedding   vector NOT NULL,
    metadata    JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- NULLS NOT DISTINCT: treat (project, NULL, label, model) as a single slot
-- so upserts work correctly when artifact_id is NULL.
CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_unique
    ON embeddings (project_id, artifact_id, label, model)
    NULLS NOT DISTINCT;

CREATE INDEX IF NOT EXISTS idx_embeddings_project ON embeddings(project_id);
CREATE INDEX IF NOT EXISTS idx_embeddings_artifact ON embeddings(project_id, artifact_id);

-- HNSW indexes per dimension (cast to fixed-size vector for indexing).
-- Queries MUST cast to the same expression: embedding::vector(N) <=> $query::vector(N)
CREATE INDEX IF NOT EXISTS idx_embeddings_hnsw_1024 ON embeddings
    USING hnsw ((embedding::vector(1024)) vector_cosine_ops)
    WHERE dimensions = 1024;

CREATE INDEX IF NOT EXISTS idx_embeddings_hnsw_768 ON embeddings
    USING hnsw ((embedding::vector(768)) vector_cosine_ops)
    WHERE dimensions = 768;
