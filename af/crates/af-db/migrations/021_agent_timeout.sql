-- Migration 021: Add per-agent timeout (must be positive if set)
ALTER TABLE agents ADD COLUMN IF NOT EXISTS timeout_secs INT CHECK (timeout_secs > 0);
