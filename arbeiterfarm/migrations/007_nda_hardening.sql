-- NDA enforcement hardening: RLS for ghidra_function_renames and yara_rules.
-- Defense-in-depth — application code already uses af_shareable_projects(),
-- but RLS ensures DB-level isolation even if application logic is bypassed.

-- 1. RLS for re.ghidra_function_renames (project_id is NOT NULL)
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 're' AND tablename = 'ghidra_function_renames') THEN
        EXECUTE 'ALTER TABLE re.ghidra_function_renames ENABLE ROW LEVEL SECURITY';

        EXECUTE 'DROP POLICY IF EXISTS gfr_sel ON re.ghidra_function_renames';
        EXECUTE 'CREATE POLICY gfr_sel ON re.ghidra_function_renames FOR SELECT TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS gfr_ins ON re.ghidra_function_renames';
        EXECUTE 'CREATE POLICY gfr_ins ON re.ghidra_function_renames FOR INSERT TO af_api
            WITH CHECK (project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS gfr_upd ON re.ghidra_function_renames';
        EXECUTE 'CREATE POLICY gfr_upd ON re.ghidra_function_renames FOR UPDATE TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS gfr_del ON re.ghidra_function_renames';
        EXECUTE 'CREATE POLICY gfr_del ON re.ghidra_function_renames FOR DELETE TO af_api
            USING (project_id IN (SELECT af_visible_projects()))';
    END IF;
END $$;

-- 2. RLS for re.yara_rules (project_id is nullable — NULL = global rules visible to all)
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 're' AND tablename = 'yara_rules') THEN
        EXECUTE 'ALTER TABLE re.yara_rules ENABLE ROW LEVEL SECURITY';

        EXECUTE 'DROP POLICY IF EXISTS yr_sel ON re.yara_rules';
        EXECUTE 'CREATE POLICY yr_sel ON re.yara_rules FOR SELECT TO af_api
            USING (project_id IS NULL OR project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS yr_ins ON re.yara_rules';
        EXECUTE 'CREATE POLICY yr_ins ON re.yara_rules FOR INSERT TO af_api
            WITH CHECK (project_id IS NULL OR project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS yr_upd ON re.yara_rules';
        EXECUTE 'CREATE POLICY yr_upd ON re.yara_rules FOR UPDATE TO af_api
            USING (project_id IS NULL OR project_id IN (SELECT af_visible_projects()))';

        EXECUTE 'DROP POLICY IF EXISTS yr_del ON re.yara_rules';
        EXECUTE 'CREATE POLICY yr_del ON re.yara_rules FOR DELETE TO af_api
            USING (project_id IS NULL OR project_id IN (SELECT af_visible_projects()))';
    END IF;
END $$;
