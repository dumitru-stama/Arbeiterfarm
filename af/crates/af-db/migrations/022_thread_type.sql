-- Migration 022: Add thread_type column to threads
-- Supports: 'agent' (single agent chat), 'workflow' (predefined pipeline), 'thinking' (autonomous orchestration)

ALTER TABLE threads
  ADD COLUMN IF NOT EXISTS thread_type TEXT NOT NULL DEFAULT 'agent'
  CHECK (thread_type IN ('agent', 'workflow', 'thinking'));

CREATE INDEX IF NOT EXISTS idx_threads_type ON threads(thread_type);
