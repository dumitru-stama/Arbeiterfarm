-- 006: User accounts, API keys, and ownership columns
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TABLE IF NOT EXISTS users (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    subject         TEXT UNIQUE NOT NULL,
    display_name    TEXT,
    email           TEXT,
    roles           TEXT[] NOT NULL DEFAULT '{}',
    enabled         BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS api_keys (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id         UUID NOT NULL REFERENCES users(id),
    key_hash        TEXT UNIQUE NOT NULL,
    key_prefix      TEXT NOT NULL,
    name            TEXT NOT NULL,
    scopes          TEXT[] NOT NULL DEFAULT '{all}',
    expires_at      TIMESTAMPTZ,
    last_used_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);

ALTER TABLE projects ADD COLUMN IF NOT EXISTS owner_id UUID REFERENCES users(id);
ALTER TABLE threads ADD COLUMN IF NOT EXISTS owner_id UUID REFERENCES users(id);
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS actor_user_id UUID REFERENCES users(id);
