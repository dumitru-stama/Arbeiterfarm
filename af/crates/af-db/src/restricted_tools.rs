use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RestrictedToolRow {
    pub tool_pattern: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserToolGrantRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub tool_pattern: String,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Restricted tools CRUD
// ---------------------------------------------------------------------------

pub async fn list_restricted<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
) -> Result<Vec<RestrictedToolRow>, sqlx::Error> {
    sqlx::query_as::<_, RestrictedToolRow>(
        "SELECT * FROM restricted_tools ORDER BY tool_pattern",
    )
    .fetch_all(db)
    .await
}

pub async fn add_restricted<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    tool_pattern: &str,
    description: &str,
) -> Result<RestrictedToolRow, sqlx::Error> {
    sqlx::query_as::<_, RestrictedToolRow>(
        "INSERT INTO restricted_tools (tool_pattern, description)
         VALUES ($1, $2)
         ON CONFLICT (tool_pattern) DO UPDATE SET description = EXCLUDED.description
         RETURNING *",
    )
    .bind(tool_pattern)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn remove_restricted<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    tool_pattern: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM restricted_tools WHERE tool_pattern = $1")
        .bind(tool_pattern)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// User grants CRUD
// ---------------------------------------------------------------------------

pub async fn list_user_grants<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
) -> Result<Vec<UserToolGrantRow>, sqlx::Error> {
    sqlx::query_as::<_, UserToolGrantRow>(
        "SELECT * FROM user_tool_grants WHERE user_id = $1 ORDER BY tool_pattern",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn add_user_grant<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
    tool_pattern: &str,
) -> Result<UserToolGrantRow, sqlx::Error> {
    sqlx::query_as::<_, UserToolGrantRow>(
        "INSERT INTO user_tool_grants (user_id, tool_pattern)
         VALUES ($1, $2)
         ON CONFLICT (user_id, tool_pattern) DO UPDATE SET tool_pattern = EXCLUDED.tool_pattern
         RETURNING *",
    )
    .bind(user_id)
    .bind(tool_pattern)
    .fetch_one(db)
    .await
}

pub async fn remove_user_grant<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
    tool_pattern: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM user_tool_grants WHERE user_id = $1 AND tool_pattern = $2",
    )
    .bind(user_id)
    .bind(tool_pattern)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn clear_user_grants<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    user_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM user_tool_grants WHERE user_id = $1")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------
// Access check
// ---------------------------------------------------------------------------

/// Check whether a user is allowed to use a given tool.
///
/// Logic:
/// 1. Load all restricted tool patterns
/// 2. Check if tool_name matches any restricted pattern (exact + wildcard `web.*`)
/// 3. If not restricted → return true (unrestricted tool, anyone can use it)
/// 4. If restricted → load user's grants, check if any grant pattern matches tool_name
/// 5. No matching grant → return false
pub async fn check_tool_allowed<'e, E: Executor<'e, Database = Postgres> + Copy>(
    db: E,
    user_id: Uuid,
    tool_name: &str,
) -> Result<bool, sqlx::Error> {
    // Load all restricted patterns
    let restricted = list_restricted(db).await?;

    // Check if this tool matches any restricted pattern
    let is_restricted = restricted.iter().any(|r| pattern_matches(&r.tool_pattern, tool_name));
    if !is_restricted {
        return Ok(true); // unrestricted tool, everyone can use it
    }

    // Tool is restricted — check user grants
    let grants = list_user_grants(db, user_id).await?;
    let has_grant = grants.iter().any(|g| pattern_matches(&g.tool_pattern, tool_name));
    Ok(has_grant)
}

/// Pre-loaded restriction data for caching within a single agent run.
pub struct RestrictionCache {
    pub restricted: Vec<RestrictedToolRow>,
    pub grants: Vec<UserToolGrantRow>,
}

impl RestrictionCache {
    /// Check whether a tool is allowed using cached data.
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        let is_restricted = self.restricted.iter().any(|r| pattern_matches(&r.tool_pattern, tool_name));
        if !is_restricted {
            return true;
        }
        self.grants.iter().any(|g| pattern_matches(&g.tool_pattern, tool_name))
    }
}

/// Load restriction data for a user into a cache. Returns None if there's
/// no user_id (local CLI / hooks = unrestricted).
pub async fn load_restrictions<'e, E: Executor<'e, Database = Postgres> + Copy>(
    db: E,
    user_id: Option<Uuid>,
) -> Result<Option<RestrictionCache>, sqlx::Error> {
    let uid = match user_id {
        Some(uid) => uid,
        None => return Ok(None), // no user = unrestricted
    };
    let restricted = list_restricted(db).await?;
    if restricted.is_empty() {
        return Ok(None); // no restrictions configured at all
    }
    let grants = list_user_grants(db, uid).await?;
    Ok(Some(RestrictionCache { restricted, grants }))
}

/// Check whether a pattern (from restricted_tools or user_tool_grants) matches a tool name.
/// Supports exact match and wildcard suffix (e.g., "web.*" matches "web.fetch").
fn pattern_matches(pattern: &str, tool_name: &str) -> bool {
    if pattern == tool_name || pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        if tool_name.starts_with(prefix) && tool_name[prefix.len()..].starts_with('.') {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_matches() {
        assert!(pattern_matches("web.*", "web.fetch"));
        assert!(pattern_matches("web.*", "web.search"));
        assert!(!pattern_matches("web.*", "webhook.fire"));
        assert!(!pattern_matches("web.*", "web"));
        assert!(pattern_matches("web.fetch", "web.fetch"));
        assert!(!pattern_matches("web.fetch", "web.search"));
        assert!(pattern_matches("*", "anything"));
    }
}
