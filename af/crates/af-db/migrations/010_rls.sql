-- Migration 010: Row-Level Security + Scoped Worker Credentials
--
-- Creates restricted DB roles (af_api, af_worker), enables RLS on
-- tenant-scoped tables, and adds SECURITY DEFINER helper functions for
-- cross-table visibility checks.

-- ────────────────────────────────────────────────────────────────────
-- 1a. Create roles (idempotent)
-- ────────────────────────────────────────────────────────────────────
DO $$ BEGIN CREATE ROLE af_api NOLOGIN; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE af_worker NOLOGIN; EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- Allow the login user (af) to SET ROLE to these
DO $$ BEGIN GRANT af_api TO af; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
DO $$ BEGIN GRANT af_worker TO af; EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- ────────────────────────────────────────────────────────────────────
-- 1b. Grant permissions
-- ────────────────────────────────────────────────────────────────────

-- af_api: full DML on all tables (RLS restricts rows, not operations)
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO af_api;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO af_api;

-- af_worker: restricted to job-related tables only
GRANT SELECT, INSERT, UPDATE ON tool_runs TO af_worker;
GRANT SELECT, INSERT ON tool_run_artifacts, tool_run_events, blobs TO af_worker;
GRANT SELECT, INSERT ON artifacts TO af_worker;
GRANT SELECT ON projects, threads TO af_worker;
GRANT SELECT, INSERT ON audit_log TO af_worker;
GRANT SELECT, UPDATE ON user_quotas TO af_worker;
GRANT SELECT, INSERT, UPDATE ON usage_daily TO af_worker;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO af_worker;

-- ────────────────────────────────────────────────────────────────────
-- 1c. Helper functions (SECURITY DEFINER)
--
-- These run as the table owner (af) regardless of the calling role,
-- breaking the circular dependency where RLS policies need to read
-- RLS-protected tables.
-- ────────────────────────────────────────────────────────────────────

CREATE OR REPLACE FUNCTION af_visible_projects() RETURNS SETOF UUID AS $$
    SELECT id FROM projects WHERE owner_id::text = current_setting('af.current_user_id', true)
    UNION
    SELECT project_id FROM project_members WHERE user_id::text = current_setting('af.current_user_id', true)
$$ LANGUAGE sql STABLE SECURITY DEFINER;

CREATE OR REPLACE FUNCTION af_visible_threads() RETURNS SETOF UUID AS $$
    SELECT id FROM threads WHERE project_id IN (SELECT af_visible_projects())
$$ LANGUAGE sql STABLE SECURITY DEFINER;

CREATE OR REPLACE FUNCTION af_visible_tool_runs() RETURNS SETOF UUID AS $$
    SELECT id FROM tool_runs WHERE project_id IN (SELECT af_visible_projects())
$$ LANGUAGE sql STABLE SECURITY DEFINER;

CREATE OR REPLACE FUNCTION af_visible_messages() RETURNS SETOF UUID AS $$
    SELECT id FROM messages WHERE thread_id IN (SELECT af_visible_threads())
$$ LANGUAGE sql STABLE SECURITY DEFINER;

-- ────────────────────────────────────────────────────────────────────
-- 1d. Enable RLS on project-scoped tables
-- ────────────────────────────────────────────────────────────────────

-- projects
ALTER TABLE projects ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS projects_sel ON projects;
CREATE POLICY projects_sel ON projects FOR SELECT TO af_api
    USING (id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS projects_ins ON projects;
CREATE POLICY projects_ins ON projects FOR INSERT TO af_api
    WITH CHECK (owner_id::text = current_setting('af.current_user_id', true));
DROP POLICY IF EXISTS projects_upd ON projects;
CREATE POLICY projects_upd ON projects FOR UPDATE TO af_api
    USING (id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS projects_del ON projects;
CREATE POLICY projects_del ON projects FOR DELETE TO af_api
    USING (id IN (SELECT af_visible_projects()));

-- threads
ALTER TABLE threads ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS threads_sel ON threads;
CREATE POLICY threads_sel ON threads FOR SELECT TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS threads_ins ON threads;
CREATE POLICY threads_ins ON threads FOR INSERT TO af_api
    WITH CHECK (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS threads_upd ON threads;
CREATE POLICY threads_upd ON threads FOR UPDATE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS threads_del ON threads;
CREATE POLICY threads_del ON threads FOR DELETE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));

-- artifacts
ALTER TABLE artifacts ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS artifacts_sel ON artifacts;
CREATE POLICY artifacts_sel ON artifacts FOR SELECT TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS artifacts_ins ON artifacts;
CREATE POLICY artifacts_ins ON artifacts FOR INSERT TO af_api
    WITH CHECK (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS artifacts_upd ON artifacts;
CREATE POLICY artifacts_upd ON artifacts FOR UPDATE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS artifacts_del ON artifacts;
CREATE POLICY artifacts_del ON artifacts FOR DELETE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));

-- tool_runs
ALTER TABLE tool_runs ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tool_runs_sel ON tool_runs;
CREATE POLICY tool_runs_sel ON tool_runs FOR SELECT TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS tool_runs_ins ON tool_runs;
CREATE POLICY tool_runs_ins ON tool_runs FOR INSERT TO af_api
    WITH CHECK (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS tool_runs_upd ON tool_runs;
CREATE POLICY tool_runs_upd ON tool_runs FOR UPDATE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS tool_runs_del ON tool_runs;
CREATE POLICY tool_runs_del ON tool_runs FOR DELETE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));

-- messages (join through threads)
ALTER TABLE messages ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS messages_sel ON messages;
CREATE POLICY messages_sel ON messages FOR SELECT TO af_api
    USING (thread_id IN (SELECT af_visible_threads()));
DROP POLICY IF EXISTS messages_ins ON messages;
CREATE POLICY messages_ins ON messages FOR INSERT TO af_api
    WITH CHECK (thread_id IN (SELECT af_visible_threads()));
DROP POLICY IF EXISTS messages_upd ON messages;
CREATE POLICY messages_upd ON messages FOR UPDATE TO af_api
    USING (thread_id IN (SELECT af_visible_threads()));
DROP POLICY IF EXISTS messages_del ON messages;
CREATE POLICY messages_del ON messages FOR DELETE TO af_api
    USING (thread_id IN (SELECT af_visible_threads()));

-- tool_run_artifacts (join through tool_runs)
ALTER TABLE tool_run_artifacts ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tra_sel ON tool_run_artifacts;
CREATE POLICY tra_sel ON tool_run_artifacts FOR SELECT TO af_api
    USING (tool_run_id IN (SELECT af_visible_tool_runs()));
DROP POLICY IF EXISTS tra_ins ON tool_run_artifacts;
CREATE POLICY tra_ins ON tool_run_artifacts FOR INSERT TO af_api
    WITH CHECK (tool_run_id IN (SELECT af_visible_tool_runs()));
DROP POLICY IF EXISTS tra_upd ON tool_run_artifacts;
CREATE POLICY tra_upd ON tool_run_artifacts FOR UPDATE TO af_api
    USING (tool_run_id IN (SELECT af_visible_tool_runs()));
DROP POLICY IF EXISTS tra_del ON tool_run_artifacts;
CREATE POLICY tra_del ON tool_run_artifacts FOR DELETE TO af_api
    USING (tool_run_id IN (SELECT af_visible_tool_runs()));

-- tool_run_events (join through tool_runs)
ALTER TABLE tool_run_events ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tre_sel ON tool_run_events;
CREATE POLICY tre_sel ON tool_run_events FOR SELECT TO af_api
    USING (tool_run_id IN (SELECT af_visible_tool_runs()));
DROP POLICY IF EXISTS tre_ins ON tool_run_events;
CREATE POLICY tre_ins ON tool_run_events FOR INSERT TO af_api
    WITH CHECK (tool_run_id IN (SELECT af_visible_tool_runs()));
DROP POLICY IF EXISTS tre_upd ON tool_run_events;
CREATE POLICY tre_upd ON tool_run_events FOR UPDATE TO af_api
    USING (tool_run_id IN (SELECT af_visible_tool_runs()));
DROP POLICY IF EXISTS tre_del ON tool_run_events;
CREATE POLICY tre_del ON tool_run_events FOR DELETE TO af_api
    USING (tool_run_id IN (SELECT af_visible_tool_runs()));

-- message_evidence (join through messages)
ALTER TABLE message_evidence ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS me_sel ON message_evidence;
CREATE POLICY me_sel ON message_evidence FOR SELECT TO af_api
    USING (message_id IN (SELECT af_visible_messages()));
DROP POLICY IF EXISTS me_ins ON message_evidence;
CREATE POLICY me_ins ON message_evidence FOR INSERT TO af_api
    WITH CHECK (message_id IN (SELECT af_visible_messages()));
DROP POLICY IF EXISTS me_upd ON message_evidence;
CREATE POLICY me_upd ON message_evidence FOR UPDATE TO af_api
    USING (message_id IN (SELECT af_visible_messages()));
DROP POLICY IF EXISTS me_del ON message_evidence;
CREATE POLICY me_del ON message_evidence FOR DELETE TO af_api
    USING (message_id IN (SELECT af_visible_messages()));

-- ────────────────────────────────────────────────────────────────────
-- 1e. Enable RLS on user-scoped tables
-- ────────────────────────────────────────────────────────────────────

-- api_keys
ALTER TABLE api_keys ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ak_all ON api_keys;
CREATE POLICY ak_all ON api_keys FOR ALL TO af_api
    USING (user_id::text = current_setting('af.current_user_id', true))
    WITH CHECK (user_id::text = current_setting('af.current_user_id', true));

-- user_quotas
ALTER TABLE user_quotas ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS uq_all ON user_quotas;
CREATE POLICY uq_all ON user_quotas FOR ALL TO af_api
    USING (user_id::text = current_setting('af.current_user_id', true))
    WITH CHECK (user_id::text = current_setting('af.current_user_id', true));

-- usage_daily
ALTER TABLE usage_daily ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ud_all ON usage_daily;
CREATE POLICY ud_all ON usage_daily FOR ALL TO af_api
    USING (user_id::text = current_setting('af.current_user_id', true))
    WITH CHECK (user_id::text = current_setting('af.current_user_id', true));

-- project_members (visible for projects you belong to)
ALTER TABLE project_members ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pm_sel ON project_members;
CREATE POLICY pm_sel ON project_members FOR SELECT TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS pm_ins ON project_members;
CREATE POLICY pm_ins ON project_members FOR INSERT TO af_api
    WITH CHECK (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS pm_upd ON project_members;
CREATE POLICY pm_upd ON project_members FOR UPDATE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
DROP POLICY IF EXISTS pm_del ON project_members;
CREATE POLICY pm_del ON project_members FOR DELETE TO af_api
    USING (project_id IN (SELECT af_visible_projects()));
