-- Migration 019: Add source_plugin column to agents and workflows tables.
-- Tracks which plugin provided the agent/workflow (display metadata).

ALTER TABLE agents ADD COLUMN IF NOT EXISTS source_plugin TEXT;
ALTER TABLE workflows ADD COLUMN IF NOT EXISTS source_plugin TEXT;

-- Backfill: existing builtin rows get 'builtin' source
UPDATE agents SET source_plugin = 'builtin' WHERE is_builtin = true AND source_plugin IS NULL;
UPDATE workflows SET source_plugin = 'builtin' WHERE is_builtin = true AND source_plugin IS NULL;
