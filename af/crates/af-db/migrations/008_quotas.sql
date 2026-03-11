-- Phase 5: Rate Limiting & Quotas

CREATE TABLE IF NOT EXISTS user_quotas (
    user_id                 UUID PRIMARY KEY REFERENCES users(id),
    max_storage_bytes       BIGINT NOT NULL DEFAULT 10737418240,
    max_projects            INTEGER NOT NULL DEFAULT 50,
    max_concurrent_runs     INTEGER NOT NULL DEFAULT 5,
    max_llm_tokens_per_day  BIGINT NOT NULL DEFAULT 1000000,
    max_upload_bytes        BIGINT NOT NULL DEFAULT 104857600,
    max_vt_lookups_per_day  INTEGER NOT NULL DEFAULT 100
);

CREATE TABLE IF NOT EXISTS usage_daily (
    user_id                 UUID NOT NULL REFERENCES users(id),
    date                    DATE NOT NULL DEFAULT CURRENT_DATE,
    llm_prompt_tokens       BIGINT NOT NULL DEFAULT 0,
    llm_completion_tokens   BIGINT NOT NULL DEFAULT 0,
    vt_lookups              INTEGER NOT NULL DEFAULT 0,
    tool_runs               INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, date)
);

ALTER TABLE users ADD COLUMN IF NOT EXISTS storage_bytes_used BIGINT NOT NULL DEFAULT 0;

ALTER TABLE tool_runs ADD COLUMN IF NOT EXISTS actor_user_id UUID;
