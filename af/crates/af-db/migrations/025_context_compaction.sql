-- Migration 025: Context compaction support
-- Adds a `compacted` flag to messages so that compacted messages are excluded from
-- context building while remaining available for export/audit.

ALTER TABLE messages ADD COLUMN IF NOT EXISTS compacted BOOLEAN NOT NULL DEFAULT FALSE;

-- Index for efficiently fetching only non-compacted messages
CREATE INDEX IF NOT EXISTS idx_messages_thread_not_compacted
    ON messages(thread_id) WHERE NOT compacted;
