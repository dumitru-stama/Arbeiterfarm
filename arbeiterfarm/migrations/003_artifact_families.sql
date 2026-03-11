CREATE TABLE IF NOT EXISTS re.artifact_families (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      UUID NOT NULL,
    artifact_id     UUID NOT NULL,
    family_name     TEXT NOT NULL,
    confidence      TEXT NOT NULL DEFAULT 'medium'
                    CHECK (confidence IN ('low', 'medium', 'high', 'confirmed')),
    notes           TEXT,
    tagged_by_agent TEXT,
    source_tool_run UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, artifact_id, family_name)
);

CREATE INDEX IF NOT EXISTS idx_re_artifact_families_project ON re.artifact_families(project_id);
CREATE INDEX IF NOT EXISTS idx_re_artifact_families_family ON re.artifact_families(family_name);
CREATE INDEX IF NOT EXISTS idx_re_artifact_families_artifact ON re.artifact_families(artifact_id);
