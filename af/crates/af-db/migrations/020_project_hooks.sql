-- Migration 020: Project hooks (event-driven automation)

CREATE TABLE IF NOT EXISTS project_hooks (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id),
    name            TEXT NOT NULL,
    enabled         BOOLEAN NOT NULL DEFAULT true,
    event_type      TEXT NOT NULL CHECK (event_type IN ('artifact_uploaded', 'tick')),
    workflow_name   TEXT,
    agent_name      TEXT,
    prompt_template TEXT NOT NULL,
    route_override  TEXT,
    tick_interval_minutes INTEGER CHECK (tick_interval_minutes > 0),
    last_tick_at    TIMESTAMPTZ,
    tick_generation BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_hook_target CHECK (
        (workflow_name IS NOT NULL AND agent_name IS NULL)
        OR (workflow_name IS NULL AND agent_name IS NOT NULL)
    ),
    CONSTRAINT chk_tick_interval CHECK (
        event_type != 'tick' OR tick_interval_minutes IS NOT NULL
    ),
    UNIQUE (project_id, name)
);

CREATE INDEX IF NOT EXISTS idx_project_hooks_project ON project_hooks(project_id);
CREATE INDEX IF NOT EXISTS idx_project_hooks_tick ON project_hooks(event_type, enabled)
    WHERE event_type = 'tick' AND enabled = true;
