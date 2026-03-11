-- 007: RBAC — project membership roles
CREATE TABLE IF NOT EXISTS project_members (
    project_id  UUID NOT NULL REFERENCES projects(id),
    user_id     UUID NOT NULL REFERENCES users(id),
    role        TEXT NOT NULL DEFAULT 'viewer'
                CHECK (role IN ('owner', 'editor', 'viewer')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
