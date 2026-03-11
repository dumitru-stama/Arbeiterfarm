-- Arbeiterfarm Slice 1 initial schema

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Projects
CREATE TABLE IF NOT EXISTS projects (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Content-addressed blobs
CREATE TABLE IF NOT EXISTS blobs (
    sha256        TEXT PRIMARY KEY,
    size_bytes    BIGINT NOT NULL,
    storage_path  TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Artifacts (project-scoped references to blobs)
CREATE TABLE IF NOT EXISTS artifacts (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id),
    sha256              TEXT NOT NULL REFERENCES blobs(sha256),
    filename            TEXT NOT NULL,
    mime_type           TEXT,
    source_tool_run_id  UUID,  -- NULL for user-uploaded
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_artifacts_project ON artifacts(project_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_sha256 ON artifacts(sha256);

-- Tool runs (job queue)
CREATE TABLE IF NOT EXISTS tool_runs (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id),
    tool_name       TEXT NOT NULL,
    tool_version    INTEGER NOT NULL,
    input_json      JSONB NOT NULL,
    status          TEXT NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    output_json     JSONB,
    output_kind     TEXT,
    error_json      JSONB,
    stdout          TEXT,
    stderr          TEXT,
    thread_id       UUID,
    parent_message_id UUID,
    actor_subject   TEXT,
    attempt         INTEGER NOT NULL DEFAULT 0,
    lease_expires   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_tool_runs_status ON tool_runs(status) WHERE status = 'queued';
CREATE INDEX IF NOT EXISTS idx_tool_runs_project ON tool_runs(project_id);

-- Link table: tool_run <-> produced artifacts
CREATE TABLE IF NOT EXISTS tool_run_artifacts (
    tool_run_id  UUID NOT NULL REFERENCES tool_runs(id),
    artifact_id  UUID NOT NULL REFERENCES artifacts(id),
    role         TEXT NOT NULL DEFAULT 'output',  -- 'input' or 'output'
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tool_run_id, artifact_id, role)
);

-- Tool run events (progress / streaming)
CREATE TABLE IF NOT EXISTS tool_run_events (
    id           UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tool_run_id  UUID NOT NULL REFERENCES tool_runs(id),
    event_type   TEXT NOT NULL,
    payload      JSONB,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_tool_run_events_run ON tool_run_events(tool_run_id);

-- Plugin migrations tracking (used by ScopedPluginDb)
CREATE TABLE IF NOT EXISTS plugin_migrations (
    id          SERIAL PRIMARY KEY,
    plugin      TEXT NOT NULL,
    version     INTEGER NOT NULL,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(plugin, version)
);
