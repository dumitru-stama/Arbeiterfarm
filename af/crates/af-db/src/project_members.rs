use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

/// Well-known sentinel UUID for the @all pseudo-member.
pub const ALL_USERS_SENTINEL: Uuid = Uuid::from_u128(0);

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectMemberRow {
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// Member row with user display info joined from the users table.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectMemberWithName {
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Add (or update) a project member. Upserts on (project_id, user_id).
pub async fn add_member(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(project_id)
    .bind(user_id)
    .bind(role)
    .execute(db)
    .await?;
    Ok(())
}

/// Remove a member from a project.
pub async fn remove_member(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Get the role a user has on a project (None if not a member).
pub async fn get_role(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.0))
}

/// List all members of a project.
pub async fn list_members(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
) -> Result<Vec<ProjectMemberRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectMemberRow>(
        "SELECT project_id, user_id, role, created_at \
         FROM project_members WHERE project_id = $1 \
         ORDER BY created_at",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}

/// List all members of a project with user display names.
pub async fn list_members_with_names(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
) -> Result<Vec<ProjectMemberWithName>, sqlx::Error> {
    sqlx::query_as::<_, ProjectMemberWithName>(
        "SELECT pm.project_id, pm.user_id, pm.role, u.display_name, pm.created_at \
         FROM project_members pm \
         JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1 \
         ORDER BY pm.created_at",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}
