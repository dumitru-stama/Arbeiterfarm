-- Ghidra function renames: per-project overlay on shared analysis cache.
-- Renames are stored in the DB instead of modifying the Ghidra project on disk,
-- enabling safe cache sharing across non-NDA projects.

CREATE TABLE re.ghidra_function_renames (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sha256      TEXT NOT NULL,
    old_name    TEXT NOT NULL,
    new_name    TEXT NOT NULL,
    address     TEXT,
    renamed_by  UUID REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One rename per (project, sha256, old_name) — last write wins within a project
CREATE UNIQUE INDEX uq_ghidra_rename_proj_sha_old
    ON re.ghidra_function_renames(project_id, sha256, old_name);

-- Fast lookup: all renames for a binary in a project
CREATE INDEX idx_ghidra_rename_sha
    ON re.ghidra_function_renames(sha256);
