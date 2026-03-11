-- Migration 015: Add metadata JSONB column to artifacts for repivot tracking
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}';

CREATE INDEX IF NOT EXISTS idx_artifacts_repivot
    ON artifacts ((metadata->>'repivot_from'))
    WHERE metadata->>'repivot_from' IS NOT NULL;
