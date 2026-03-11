-- Per-thread persistent memory for local LLM context rotation.
-- Key/value store updated deterministically after each tool call.
-- UNIQUE(thread_id, key) enables upsert semantics → bounded growth.

CREATE TABLE IF NOT EXISTS thread_memory (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    thread_id   UUID NOT NULL REFERENCES threads(id),
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(thread_id, key)
);

CREATE INDEX IF NOT EXISTS idx_thread_memory_thread ON thread_memory(thread_id);
