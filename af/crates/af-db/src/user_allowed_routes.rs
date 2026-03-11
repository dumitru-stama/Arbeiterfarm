use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserAllowedRouteRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub route: String,
    pub created_at: DateTime<Utc>,
}

/// List all allowed routes for a user.
pub async fn list_routes(pool: &PgPool, user_id: Uuid) -> Result<Vec<UserAllowedRouteRow>, sqlx::Error> {
    sqlx::query_as::<_, UserAllowedRouteRow>(
        "SELECT id, user_id, route, created_at FROM user_allowed_routes \
         WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Add an allowed route for a user. Returns the new row.
/// If the route already exists, returns the existing row (upsert).
pub async fn add_route(pool: &PgPool, user_id: Uuid, route: &str) -> Result<UserAllowedRouteRow, sqlx::Error> {
    sqlx::query_as::<_, UserAllowedRouteRow>(
        "INSERT INTO user_allowed_routes (user_id, route) VALUES ($1, $2) \
         ON CONFLICT (user_id, route) DO UPDATE SET route = EXCLUDED.route \
         RETURNING id, user_id, route, created_at",
    )
    .bind(user_id)
    .bind(route)
    .fetch_one(pool)
    .await
}

/// Remove a specific route for a user. Returns true if a row was deleted.
pub async fn remove_route(pool: &PgPool, user_id: Uuid, route: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM user_allowed_routes WHERE user_id = $1 AND route = $2",
    )
    .bind(user_id)
    .bind(route)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Remove all routes for a user (returns to unrestricted mode). Returns count deleted.
pub async fn remove_all_routes(pool: &PgPool, user_id: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM user_allowed_routes WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Check whether a route is allowed for a user.
/// No rows = unrestricted (returns true). Otherwise checks exact and wildcard matches.
pub async fn check_route_allowed(pool: &PgPool, user_id: Uuid, route_str: &str) -> Result<bool, sqlx::Error> {
    let rows = list_routes(pool, user_id).await?;
    if rows.is_empty() {
        return Ok(true); // unrestricted
    }
    for row in &rows {
        if row.route == route_str {
            return Ok(true); // exact match
        }
        // Wildcard: "openai:*" matches "openai:gpt-4o-mini"
        if let Some(prefix) = row.route.strip_suffix('*') {
            if route_str.starts_with(prefix) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
