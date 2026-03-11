-- Migration 023: Per-user model access control
-- No rows for a user = unrestricted (all routes allowed).
-- At least one row = allowlist mode (only listed routes permitted).

CREATE TABLE IF NOT EXISTS user_allowed_routes (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    route       TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, route)
);

CREATE INDEX IF NOT EXISTS idx_user_allowed_routes_user ON user_allowed_routes(user_id);
