-- Migration 027: Project settings + RLS on artifact_families
--
-- 1. Add JSONB settings column to projects
-- 2. Enable RLS on re.artifact_families (missing from 011_security_hardening.sql)

ALTER TABLE projects ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}';

-- RLS on re.artifact_families
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 're' AND tablename = 'artifact_families') THEN
        EXECUTE 'ALTER TABLE re.artifact_families ENABLE ROW LEVEL SECURITY';
        EXECUTE 'DROP POLICY IF EXISTS af_sel ON re.artifact_families';
        EXECUTE 'CREATE POLICY af_sel ON re.artifact_families FOR SELECT TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';
        EXECUTE 'DROP POLICY IF EXISTS af_ins ON re.artifact_families';
        EXECUTE 'CREATE POLICY af_ins ON re.artifact_families FOR INSERT TO af_api
            WITH CHECK (project_id IN (SELECT af_visible_projects()))';
        EXECUTE 'DROP POLICY IF EXISTS af_upd ON re.artifact_families';
        EXECUTE 'CREATE POLICY af_upd ON re.artifact_families FOR UPDATE TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';
        EXECUTE 'DROP POLICY IF EXISTS af_del ON re.artifact_families';
        EXECUTE 'CREATE POLICY af_del ON re.artifact_families FOR DELETE TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';
    END IF;
END $$;
