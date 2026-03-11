-- Migration 014: Distributed rate limiting table
-- Fixed-window counter for API rate limiting across multiple server instances.

CREATE TABLE IF NOT EXISTS api_rate_limits (
    key         TEXT NOT NULL,
    "window"    TIMESTAMPTZ NOT NULL,
    count       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (key, "window")
);

CREATE INDEX IF NOT EXISTS idx_api_rate_limits_window ON api_rate_limits ("window");
