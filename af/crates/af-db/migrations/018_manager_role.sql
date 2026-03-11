-- 018: Manager role, editor→collaborator rename, @all sentinel user
--
-- Adds 'manager' role (Read+Write+ManageMembers, no Delete).
-- Renames 'editor' → 'collaborator'.
-- Inserts sentinel @all user for public project visibility.
-- Updates af_visible_projects() to include @all-shared projects.

-- 1. Rename existing 'editor' rows to 'collaborator'
UPDATE project_members SET role = 'collaborator' WHERE role = 'editor';

-- 2. Replace constraint to allow new role set
ALTER TABLE project_members DROP CONSTRAINT IF EXISTS project_members_role_check;
ALTER TABLE project_members ADD CONSTRAINT project_members_role_check
    CHECK (role IN ('owner', 'manager', 'collaborator', 'viewer'));

-- 3. Insert sentinel @all user (idempotent)
INSERT INTO users (id, subject, display_name, roles, enabled)
VALUES ('00000000-0000-0000-0000-000000000000', '@all', 'All Users', '{}', true)
ON CONFLICT (id) DO NOTHING;

-- 4. Update RLS helper to include @all-shared projects
CREATE OR REPLACE FUNCTION af_visible_projects() RETURNS SETOF UUID AS $$
    SELECT id FROM projects WHERE owner_id::text = current_setting('af.current_user_id', true)
    UNION
    SELECT project_id FROM project_members WHERE user_id::text = current_setting('af.current_user_id', true)
    UNION
    SELECT project_id FROM project_members WHERE user_id = '00000000-0000-0000-0000-000000000000'
$$ LANGUAGE sql STABLE SECURITY DEFINER;
