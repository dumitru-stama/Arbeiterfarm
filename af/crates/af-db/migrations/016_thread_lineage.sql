-- Migration 016: Thread lineage for fan-out (parent/child threads)

ALTER TABLE threads ADD COLUMN IF NOT EXISTS parent_thread_id UUID REFERENCES threads(id);

CREATE INDEX IF NOT EXISTS idx_threads_parent
    ON threads(parent_thread_id) WHERE parent_thread_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_artifacts_fanout
    ON artifacts ((metadata->>'fan_out_from'))
    WHERE metadata->>'fan_out_from' IS NOT NULL;
