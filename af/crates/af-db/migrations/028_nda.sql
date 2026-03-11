-- Migration 028: NDA flag + shareable projects function

ALTER TABLE projects ADD COLUMN IF NOT EXISTS nda BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX IF NOT EXISTS idx_projects_nda ON projects (nda) WHERE nda = true;

-- af_shareable_projects(): projects visible to current user AND not NDA AND not exclude_from_search
-- Used by cross-project tools (family.search, artifact.search, dedup.prior_analysis)
CREATE OR REPLACE FUNCTION af_shareable_projects() RETURNS SETOF UUID AS $$
    SELECT id FROM projects
    WHERE id IN (SELECT af_visible_projects())
      AND nda = false
      AND COALESCE(settings->>'exclude_from_search', 'false') <> 'true'
$$ LANGUAGE sql STABLE SECURITY DEFINER;
