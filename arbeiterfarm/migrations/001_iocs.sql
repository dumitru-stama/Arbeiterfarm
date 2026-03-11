CREATE SCHEMA IF NOT EXISTS re;

CREATE TABLE IF NOT EXISTS re.iocs (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id        UUID NOT NULL,
    ioc_type          TEXT NOT NULL,
    value             TEXT NOT NULL,
    source_tool_run   UUID,
    context           TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_re_iocs_value ON re.iocs(value);
CREATE INDEX IF NOT EXISTS idx_re_iocs_project ON re.iocs(project_id);
CREATE INDEX IF NOT EXISTS idx_re_iocs_type ON re.iocs(ioc_type);
