-- 030_email.sql — Email tools: recipient rules, tone presets, scheduling, logs, credentials

-- ---------------------------------------------------------------------------
-- Recipient Rules (allowlist/blocklist)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS email_recipient_rules (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    scope         TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project_id    UUID REFERENCES projects(id) ON DELETE CASCADE,
    rule_type     TEXT NOT NULL CHECK (rule_type IN ('allow', 'block')),
    pattern_type  TEXT NOT NULL CHECK (pattern_type IN ('exact_email', 'domain', 'domain_suffix')),
    pattern       TEXT NOT NULL,
    description   TEXT,
    created_by    UUID REFERENCES users(id),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT email_scope_project_ck CHECK (
        (scope = 'global' AND project_id IS NULL) OR
        (scope = 'project' AND project_id IS NOT NULL)
    ),
    UNIQUE (scope, project_id, rule_type, pattern_type, pattern)
);
CREATE INDEX IF NOT EXISTS idx_email_rules_scope ON email_recipient_rules(scope);
CREATE INDEX IF NOT EXISTS idx_email_rules_project ON email_recipient_rules(project_id) WHERE project_id IS NOT NULL;
-- Partial unique index for global rules (project_id IS NULL defeats the table-level UNIQUE)
CREATE UNIQUE INDEX IF NOT EXISTS idx_email_rules_global_unique
    ON email_recipient_rules(scope, rule_type, pattern_type, pattern)
    WHERE scope = 'global';

-- ---------------------------------------------------------------------------
-- Tone Presets
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS email_tone_presets (
    name               TEXT PRIMARY KEY,
    description        TEXT,
    system_instruction TEXT NOT NULL,
    is_builtin         BOOLEAN NOT NULL DEFAULT false,
    created_by         UUID REFERENCES users(id),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO email_tone_presets (name, description, system_instruction, is_builtin) VALUES
('brief',             'Short and to the point',
 'Write concisely. Short sentences, no filler. 2-3 short paragraphs max.', true),
('formal',            'Professional and formal',
 'Professional, formal tone. Proper salutations/sign-offs. No contractions or colloquialisms.', true),
('informal',          'Casual and friendly',
 'Casual, friendly tone. Contractions, conversational language, warm approach.', true),
('technical',         'Precise technical communication',
 'Precise technical language. Include specifications and details. Clear and unambiguous.', true),
('executive_summary', 'High-level overview for leadership',
 'Lead with conclusion/key takeaway. Bullet points for details. Focus on impact and action items.', true),
('friendly',          'Warm and personable',
 'Warm, personable tone. Show genuine interest. Friendly while remaining professional.', true),
('urgent',            'Time-sensitive with clear action items',
 'Convey urgency. State deadline and required actions upfront. Focused and actionable.', true),
('diplomatic',        'Careful phrasing for sensitive topics',
 'Diplomatic, balanced language. Acknowledge perspectives. No accusatory language. Propose constructive paths.', true)
ON CONFLICT (name) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Scheduled Emails
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS email_scheduled (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id        UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id           UUID REFERENCES users(id),
    provider          TEXT NOT NULL CHECK (provider IN ('gmail', 'protonmail')),
    status            TEXT NOT NULL DEFAULT 'scheduled'
                      CHECK (status IN ('scheduled','sending','sent','failed','cancelled')),
    from_address      TEXT NOT NULL,
    to_addresses      JSONB NOT NULL,
    cc_addresses      JSONB DEFAULT '[]',
    bcc_addresses     JSONB DEFAULT '[]',
    subject           TEXT NOT NULL,
    body_text         TEXT,
    body_html         TEXT,
    reply_to_msg_id   TEXT,
    tone              TEXT,
    scheduled_at      TIMESTAMPTZ NOT NULL,
    error_message     TEXT,
    attempt_count     INTEGER NOT NULL DEFAULT 0,
    max_attempts      INTEGER NOT NULL DEFAULT 3,
    thread_id         UUID REFERENCES threads(id),
    tool_run_id       UUID,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sent_at           TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_email_sched_due ON email_scheduled(status, scheduled_at) WHERE status = 'scheduled';
CREATE INDEX IF NOT EXISTS idx_email_sched_project ON email_scheduled(project_id);

-- ---------------------------------------------------------------------------
-- Email Log
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS email_log (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id          UUID REFERENCES projects(id),
    user_id             UUID REFERENCES users(id),
    action              TEXT NOT NULL CHECK (action IN (
        'send','draft','schedule','cancel_schedule',
        'list_inbox','read','reply','search',
        'scheduled_send','scheduled_fail'
    )),
    provider            TEXT NOT NULL,
    from_address        TEXT,
    to_addresses        JSONB,
    subject             TEXT,
    tone                TEXT,
    success             BOOLEAN NOT NULL,
    error_message       TEXT,
    provider_message_id TEXT,
    scheduled_email_id  UUID REFERENCES email_scheduled(id),
    tool_run_id         UUID,
    thread_id           UUID REFERENCES threads(id),
    metadata            JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_email_log_project ON email_log(project_id);
CREATE INDEX IF NOT EXISTS idx_email_log_user ON email_log(user_id);
CREATE INDEX IF NOT EXISTS idx_email_log_created ON email_log(created_at DESC);

-- ---------------------------------------------------------------------------
-- Provider Credentials (per-user)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS email_credentials (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider         TEXT NOT NULL CHECK (provider IN ('gmail', 'protonmail')),
    email_address    TEXT NOT NULL,
    credentials_json JSONB NOT NULL,
    is_default       BOOLEAN NOT NULL DEFAULT false,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, provider, email_address)
);
CREATE INDEX IF NOT EXISTS idx_email_creds_user ON email_credentials(user_id);

-- ---------------------------------------------------------------------------
-- Seed email.* as restricted tool
-- ---------------------------------------------------------------------------

INSERT INTO restricted_tools (tool_pattern, description)
VALUES ('email.*', 'Email tools — requires admin grant and configured credentials')
ON CONFLICT (tool_pattern) DO NOTHING;
