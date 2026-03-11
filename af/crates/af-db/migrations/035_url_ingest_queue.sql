-- URL ingestion queue for bulk URL import into RAG knowledge base.
-- Managers paste URLs → system fetches, converts to text, chunks, enqueues for embedding.

CREATE TABLE IF NOT EXISTS url_ingest_queue (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    url                 TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','processing','completed','failed','cancelled')),
    title               TEXT,
    content_length      INTEGER,
    text_artifact_id    UUID REFERENCES artifacts(id) ON DELETE SET NULL,
    chunks_artifact_id  UUID REFERENCES artifacts(id) ON DELETE SET NULL,
    chunk_count         INTEGER,
    error_message       TEXT,
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    max_attempts        INTEGER NOT NULL DEFAULT 5,
    submitted_by        UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_url_ingest_pending
    ON url_ingest_queue(status, created_at) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_url_ingest_project
    ON url_ingest_queue(project_id);
-- Prevent duplicate active URLs within the same project
CREATE UNIQUE INDEX IF NOT EXISTS idx_url_ingest_project_url
    ON url_ingest_queue(project_id, url) WHERE status NOT IN ('failed', 'cancelled');
