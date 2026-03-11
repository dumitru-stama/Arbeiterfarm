-- Per-request LLM usage logging with cache token tracking
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'llm_usage_log') THEN
        CREATE TABLE llm_usage_log (
            id                    UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
            thread_id             UUID NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
            project_id            UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            user_id               UUID REFERENCES users(id) ON DELETE SET NULL,
            route                 TEXT NOT NULL,
            prompt_tokens         INTEGER NOT NULL DEFAULT 0,
            completion_tokens     INTEGER NOT NULL DEFAULT 0,
            cached_read_tokens    INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE INDEX idx_llm_usage_log_project ON llm_usage_log(project_id);
        CREATE INDEX idx_llm_usage_log_thread ON llm_usage_log(thread_id);
        CREATE INDEX idx_llm_usage_log_user ON llm_usage_log(user_id);
        CREATE INDEX idx_llm_usage_log_created ON llm_usage_log(created_at);
    END IF;
END $$;
