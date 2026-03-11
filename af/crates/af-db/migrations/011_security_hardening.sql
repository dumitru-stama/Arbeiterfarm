-- Migration 011: Security Hardening
--
-- 1. Remove BYPASSRLS from af_worker (worker uses begin_scoped() for tenant ops)
-- 2. Enable RLS on plugin tables (re.iocs)
-- 3. Grant af_api access to re schema

-- ────────────────────────────────────────────────────────────────────
-- 1. Remove BYPASSRLS from af_worker
-- Worker uses begin_scoped() with the job's actor_user_id for RLS.
-- Job claiming runs as af (table owner, bypasses RLS by default).
-- ────────────────────────────────────────────────────────────────────
DO $$ BEGIN ALTER ROLE af_worker NOBYPASSRLS; EXCEPTION WHEN OTHERS THEN NULL; END $$;

-- ────────────────────────────────────────────────────────────────────
-- 2. Plugin table RLS (re.iocs)
-- ────────────────────────────────────────────────────────────────────

-- Grant af_api access to the re schema
DO $$ BEGIN GRANT USAGE ON SCHEMA re TO af_api; EXCEPTION WHEN OTHERS THEN NULL; END $$;
DO $$ BEGIN GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA re TO af_api; EXCEPTION WHEN OTHERS THEN NULL; END $$;

-- Enable RLS on re.iocs (idempotent)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 're' AND tablename = 'iocs') THEN
        EXECUTE 'ALTER TABLE re.iocs ENABLE ROW LEVEL SECURITY';

        EXECUTE 'DROP POLICY IF EXISTS iocs_sel ON re.iocs';
        EXECUTE 'CREATE POLICY iocs_sel ON re.iocs FOR SELECT TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS iocs_ins ON re.iocs';
        EXECUTE 'CREATE POLICY iocs_ins ON re.iocs FOR INSERT TO af_api
            WITH CHECK (project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS iocs_upd ON re.iocs';
        EXECUTE 'CREATE POLICY iocs_upd ON re.iocs FOR UPDATE TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS iocs_del ON re.iocs';
        EXECUTE 'CREATE POLICY iocs_del ON re.iocs FOR DELETE TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';
    END IF;
END $$;
