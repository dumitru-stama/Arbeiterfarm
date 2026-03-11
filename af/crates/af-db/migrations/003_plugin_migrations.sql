-- Plugin migration tracking table.
-- NOTE: This table is already created in 001_initial.sql.
-- This migration is kept as a no-op for migration ordering history.
CREATE TABLE IF NOT EXISTS plugin_migrations (
    id          SERIAL PRIMARY KEY,
    plugin      TEXT NOT NULL,
    version     INTEGER NOT NULL,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(plugin, version)
);
