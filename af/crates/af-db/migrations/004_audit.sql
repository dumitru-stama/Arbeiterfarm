-- Slice 7: audit log + tool config

CREATE TABLE IF NOT EXISTS audit_log (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    event_type      TEXT NOT NULL,
    actor_subject   TEXT,
    detail          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_audit_log_type ON audit_log(event_type);
CREATE INDEX IF NOT EXISTS idx_audit_log_actor ON audit_log(actor_subject);
CREATE INDEX IF NOT EXISTS idx_audit_log_created ON audit_log(created_at);

CREATE TABLE IF NOT EXISTS tool_config (
    tool_name        TEXT PRIMARY KEY,
    enabled          BOOLEAN NOT NULL DEFAULT true,
    policy_override  JSONB NOT NULL DEFAULT '{}'
);
