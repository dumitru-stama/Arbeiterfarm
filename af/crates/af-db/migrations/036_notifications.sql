-- Named notification channels per project
CREATE TABLE IF NOT EXISTS notification_channels (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    channel_type    TEXT NOT NULL CHECK (channel_type IN ('webhook','email','matrix','webdav')),
    config_json     JSONB NOT NULL DEFAULT '{}',
    enabled         BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(project_id, name)
);

-- Notification queue with state machine
CREATE TABLE IF NOT EXISTS notification_queue (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    channel_id      UUID NOT NULL REFERENCES notification_channels(id) ON DELETE CASCADE,
    subject         TEXT NOT NULL,
    body            TEXT NOT NULL DEFAULT '',
    attachment_artifact_id UUID REFERENCES artifacts(id) ON DELETE SET NULL,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','processing','completed','failed','cancelled')),
    error_message   TEXT,
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER NOT NULL DEFAULT 5,
    submitted_by    UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_notif_queue_pending
    ON notification_queue(status, created_at) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_notif_queue_project
    ON notification_queue(project_id);
CREATE INDEX IF NOT EXISTS idx_notif_channels_project
    ON notification_channels(project_id);

-- pg_notify trigger for near-real-time delivery
CREATE OR REPLACE FUNCTION notify_queue_insert() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('notification_queue', NEW.id::text);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DO $$ BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger WHERE tgname = 'trg_notify_queue_insert'
    ) THEN
        CREATE TRIGGER trg_notify_queue_insert
            AFTER INSERT ON notification_queue
            FOR EACH ROW EXECUTE FUNCTION notify_queue_insert();
    END IF;
END $$;

-- Seed notify.* as restricted tools
INSERT INTO restricted_tools (tool_pattern, description)
VALUES ('notify.*', 'Notification tools — requires admin grant and channel configuration')
ON CONFLICT (tool_pattern) DO NOTHING;
