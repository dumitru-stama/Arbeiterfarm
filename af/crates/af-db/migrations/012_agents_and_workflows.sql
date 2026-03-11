-- 012: Dynamic agents, workflows, message agent attribution

CREATE TABLE IF NOT EXISTS agents (
    name          TEXT PRIMARY KEY,
    system_prompt TEXT NOT NULL,
    allowed_tools JSONB NOT NULL DEFAULT '[]',
    default_route TEXT NOT NULL DEFAULT 'auto',
    metadata      JSONB NOT NULL DEFAULT '{}',
    is_builtin    BOOLEAN NOT NULL DEFAULT false,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS workflows (
    name        TEXT PRIMARY KEY,
    description TEXT,
    steps       JSONB NOT NULL,
    is_builtin  BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE messages ADD COLUMN IF NOT EXISTS agent_name TEXT;
