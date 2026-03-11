-- Arbeiterfarm Slice 2: threads, messages, message_evidence

-- Threads (conversation containers)
CREATE TABLE IF NOT EXISTS threads (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id  UUID NOT NULL REFERENCES projects(id),
    agent_name  TEXT NOT NULL,
    title       TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_threads_project ON threads(project_id);

-- Messages within a thread
CREATE TABLE IF NOT EXISTS messages (
    id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    thread_id     UUID NOT NULL REFERENCES threads(id),
    role          TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content       TEXT,
    content_json  JSONB,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id);

-- Evidence citations attached to messages
CREATE TABLE IF NOT EXISTS message_evidence (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    message_id  UUID NOT NULL REFERENCES messages(id),
    ref_type    TEXT NOT NULL,
    ref_id      UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_message_evidence_message ON message_evidence(message_id);
