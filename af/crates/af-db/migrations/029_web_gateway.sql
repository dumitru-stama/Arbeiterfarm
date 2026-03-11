-- Migration 029: Web gateway + user tool restrictions

-- URL rules: global and per-project allowlist/blocklist
CREATE TABLE IF NOT EXISTS web_fetch_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    scope TEXT NOT NULL CHECK (scope IN ('global', 'project')),
    project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    rule_type TEXT NOT NULL CHECK (rule_type IN ('allow', 'block')),
    pattern_type TEXT NOT NULL CHECK (pattern_type IN (
        'domain', 'domain_suffix', 'url_prefix', 'url_regex', 'ip_cidr'
    )),
    pattern TEXT NOT NULL,
    description TEXT,
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT scope_project CHECK (
        (scope = 'global' AND project_id IS NULL) OR
        (scope = 'project' AND project_id IS NOT NULL)
    ),
    CONSTRAINT uq_web_fetch_rules UNIQUE (scope, project_id, rule_type, pattern_type, pattern)
);
CREATE INDEX IF NOT EXISTS idx_web_fetch_rules_scope ON web_fetch_rules (scope);
CREATE INDEX IF NOT EXISTS idx_web_fetch_rules_project ON web_fetch_rules (project_id)
    WHERE project_id IS NOT NULL;

-- Country blocks (ISO 3166-1 alpha-2)
CREATE TABLE IF NOT EXISTS web_fetch_country_blocks (
    country_code TEXT PRIMARY KEY CHECK (length(country_code) = 2),
    country_name TEXT,
    created_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Response cache (URL-keyed, TTL-based)
CREATE TABLE IF NOT EXISTS web_fetch_cache (
    url_hash TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    status_code INTEGER NOT NULL,
    content_type TEXT,
    body TEXT,
    headers JSONB,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_web_fetch_cache_expires ON web_fetch_cache (expires_at);

-- Restricted tools: tools that require explicit user grants (generic, reusable)
-- If a tool_pattern is in this table, only users with a matching grant can use it.
-- Tools NOT in this table are unrestricted (backward compatible).
CREATE TABLE IF NOT EXISTS restricted_tools (
    tool_pattern TEXT PRIMARY KEY,
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- User grants for restricted tools
CREATE TABLE IF NOT EXISTS user_tool_grants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tool_pattern TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, tool_pattern)
);
CREATE INDEX IF NOT EXISTS idx_user_tool_grants_user ON user_tool_grants (user_id);

-- Seed web.* as restricted by default (requires explicit user grants)
INSERT INTO restricted_tools (tool_pattern, description)
VALUES ('web.*', 'Web fetch and search tools — requires explicit user grant')
ON CONFLICT (tool_pattern) DO NOTHING;
