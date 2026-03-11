-- Embedding queue: background auto-embedding for RAG
-- Follows the email_scheduled pattern (claim/process/complete/fail/retry)

CREATE TABLE IF NOT EXISTS embed_queue (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    chunks_artifact_id  UUID NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    source_artifact_id  UUID REFERENCES artifacts(id) ON DELETE SET NULL,
    tool_name           TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','processing','completed','failed','cancelled')),
    chunk_count         INTEGER,
    chunks_embedded     INTEGER NOT NULL DEFAULT 0,
    error_message       TEXT,
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    max_attempts        INTEGER NOT NULL DEFAULT 5,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_embed_queue_pending
    ON embed_queue(status, created_at) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_embed_queue_project
    ON embed_queue(project_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_embed_queue_chunks_artifact
    ON embed_queue(chunks_artifact_id) WHERE status NOT IN ('failed', 'cancelled');
