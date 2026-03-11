-- Migration 004: Cross-schema views for RE tools
--
-- ScopedPluginDb restricts query search_path to (re, pg_temp) and blocks
-- the "public." qualifier via validate_sql(). Tools that need cross-project
-- queries (artifact.search, artifact.describe, dedup.prior_analysis) cannot
-- reference public.artifacts, public.projects, etc. directly.
--
-- Solution: create views in the 're' schema that alias the needed public
-- tables, plus wrapper functions for public-schema visibility functions.
-- Tools use unqualified names that resolve via the search_path.
--
-- Note: this migration runs with search_path (re, public, pg_temp), so
-- CREATE VIEW / CREATE FUNCTION land in 're' schema (first in path).

-- Views for public tables used by RE tools
CREATE OR REPLACE VIEW artifacts AS SELECT * FROM public.artifacts;
CREATE OR REPLACE VIEW projects AS SELECT * FROM public.projects;
CREATE OR REPLACE VIEW threads AS SELECT * FROM public.threads;
CREATE OR REPLACE VIEW embeddings AS SELECT * FROM public.embeddings;

-- Wrapper functions for cross-project visibility (originals are SECURITY DEFINER in public schema)
CREATE OR REPLACE FUNCTION af_shareable_projects() RETURNS SETOF UUID AS $$
    SELECT public.af_shareable_projects()
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION af_visible_projects() RETURNS SETOF UUID AS $$
    SELECT public.af_visible_projects()
$$ LANGUAGE sql STABLE;

-- Grant access to af_api role (needed when SET LOCAL ROLE af_api is active)
GRANT USAGE ON SCHEMA re TO af_api;
GRANT SELECT ON artifacts TO af_api;
GRANT SELECT ON projects TO af_api;
GRANT SELECT ON threads TO af_api;
GRANT UPDATE (description) ON artifacts TO af_api;
GRANT SELECT, DELETE ON embeddings TO af_api;
