CREATE TABLE IF NOT EXISTS re.yara_rules (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    source      TEXT NOT NULL,
    description TEXT,
    tags        TEXT[] NOT NULL DEFAULT '{}',
    project_id  UUID,          -- NULL = global
    created_by  UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Per-project uniqueness + partial index for global rules
CREATE UNIQUE INDEX IF NOT EXISTS idx_re_yara_rules_project
    ON re.yara_rules(name, project_id) WHERE project_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_re_yara_rules_global
    ON re.yara_rules(name) WHERE project_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_re_yara_rules_tags
    ON re.yara_rules USING GIN(tags);

CREATE TABLE IF NOT EXISTS re.yara_scan_results (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    artifact_id UUID NOT NULL,
    rule_name   TEXT NOT NULL,
    match_count INTEGER NOT NULL DEFAULT 0,
    match_data  JSONB,         -- [{offset, string_id, data}]
    matched_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    tool_run_id UUID,
    UNIQUE(artifact_id, rule_name)
);

CREATE INDEX IF NOT EXISTS idx_re_yara_scan_results_artifact
    ON re.yara_scan_results(artifact_id);
CREATE INDEX IF NOT EXISTS idx_re_yara_scan_results_rule
    ON re.yara_scan_results(rule_name);
