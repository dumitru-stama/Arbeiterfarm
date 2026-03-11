-- Migration 031: Fix SECURITY DEFINER functions missing SET search_path
--
-- All RLS helper functions are SECURITY DEFINER but were missing a fixed
-- search_path. When called from a plugin schema context (e.g. search_path =
-- re,pg_temp), the functions couldn't find public tables like projects and
-- project_members, poisoning the transaction.

CREATE OR REPLACE FUNCTION af_visible_projects() RETURNS SETOF UUID AS $$
    SELECT id FROM projects WHERE owner_id::text = current_setting('af.current_user_id', true)
    UNION
    SELECT project_id FROM project_members WHERE user_id::text = current_setting('af.current_user_id', true)
    UNION
    SELECT project_id FROM project_members WHERE user_id = '00000000-0000-0000-0000-000000000000'
$$ LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public;

CREATE OR REPLACE FUNCTION af_visible_threads() RETURNS SETOF UUID AS $$
    SELECT id FROM threads WHERE project_id IN (SELECT af_visible_projects())
$$ LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public;

CREATE OR REPLACE FUNCTION af_visible_tool_runs() RETURNS SETOF UUID AS $$
    SELECT id FROM tool_runs WHERE project_id IN (SELECT af_visible_projects())
$$ LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public;

CREATE OR REPLACE FUNCTION af_visible_messages() RETURNS SETOF UUID AS $$
    SELECT id FROM messages WHERE thread_id IN (SELECT af_visible_threads())
$$ LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public;

CREATE OR REPLACE FUNCTION af_shareable_projects() RETURNS SETOF UUID AS $$
    SELECT id FROM projects
    WHERE id IN (SELECT af_visible_projects())
      AND nda = false
      AND COALESCE(settings->>'exclude_from_search', 'false') <> 'true'
$$ LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public;

-- Also fix the re-schema wrapper if it exists
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_proc p JOIN pg_namespace n ON p.pronamespace = n.oid
               WHERE p.proname = 'af_visible_projects' AND n.nspname = 're') THEN
        EXECUTE 'CREATE OR REPLACE FUNCTION re.af_visible_projects() RETURNS SETOF UUID AS $fn$
            SELECT public.af_visible_projects()
        $fn$ LANGUAGE sql STABLE SET search_path = public';
    END IF;
END $$;
