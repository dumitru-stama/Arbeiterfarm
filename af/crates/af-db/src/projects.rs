use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default)]
    pub nda: bool,
}

pub async fn create_project(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
) -> Result<ProjectRow, sqlx::Error> {
    let row = sqlx::query_as::<_, ProjectRow>(
        "INSERT INTO projects (name) VALUES ($1) \
         RETURNING id, name, owner_id, created_at, updated_at, settings, nda",
    )
    .bind(name)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn create_project_with_owner(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
    owner_id: Uuid,
) -> Result<ProjectRow, sqlx::Error> {
    let row = sqlx::query_as::<_, ProjectRow>(
        "INSERT INTO projects (name, owner_id) VALUES ($1, $2) \
         RETURNING id, name, owner_id, created_at, updated_at, settings, nda",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_project(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<Option<ProjectRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, ProjectRow>(
        "SELECT id, name, owner_id, created_at, updated_at, settings, nda FROM projects WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// List all projects (admin / local CLI use).
pub async fn list_projects(
    db: impl sqlx::Executor<'_, Database = Postgres>,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ProjectRow>(
        "SELECT id, name, owner_id, created_at, updated_at, settings, nda FROM projects ORDER BY created_at DESC",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// List projects visible to a specific user (owner, member, or @all-shared).
pub async fn list_projects_for_user(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    user_id: Uuid,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ProjectRow>(
        "SELECT DISTINCT p.id, p.name, p.owner_id, p.created_at, p.updated_at, p.settings, p.nda \
         FROM projects p \
         LEFT JOIN project_members pm ON pm.project_id = p.id \
             AND (pm.user_id = $1 OR pm.user_id = '00000000-0000-0000-0000-000000000000') \
         WHERE p.owner_id = $1 OR pm.user_id IS NOT NULL \
         ORDER BY p.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Update project settings (JSONB merge).
pub async fn update_settings(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    settings: &serde_json::Value,
) -> Result<Option<ProjectRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, ProjectRow>(
        "UPDATE projects SET settings = settings || $2, updated_at = NOW() \
         WHERE id = $1 \
         RETURNING id, name, owner_id, created_at, updated_at, settings, nda",
    )
    .bind(id)
    .bind(settings)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Set the NDA flag on a project with audit trail.
///
/// Returns `(project_row, old_nda)` so callers can detect transitions and print guidance.
/// Inserts an audit log entry when the flag actually changes.
/// No-ops (same value) skip the update and audit log.
pub async fn set_nda(
    db: &mut sqlx::Transaction<'_, Postgres>,
    id: Uuid,
    nda: bool,
    changed_by: Option<Uuid>,
) -> Result<Option<(ProjectRow, bool)>, sqlx::Error> {
    // 1. Read current NDA value
    let current = sqlx::query_scalar::<_, bool>(
        "SELECT nda FROM projects WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **db)
    .await?;

    let Some(old_nda) = current else {
        return Ok(None);
    };

    // 2. No-op if unchanged
    if old_nda == nda {
        let row = get_project(&mut **db, id).await?;
        return Ok(row.map(|r| (r, old_nda)));
    }

    // 3. Audit log entry (immutable)
    let detail = serde_json::json!({
        "project_id": id,
        "old_nda": old_nda,
        "new_nda": nda,
    });
    crate::audit_log::insert(&mut **db, "nda_flag_changed", None, changed_by, Some(&detail))
        .await?;

    // 4. Log warning when removing NDA (dangerous direction)
    if old_nda && !nda {
        tracing::warn!(
            project_id = %id,
            changed_by = ?changed_by,
            "NDA flag REMOVED from project — all project data now visible in cross-project queries"
        );
    }

    // 5. Update
    let row = sqlx::query_as::<_, ProjectRow>(
        "UPDATE projects SET nda = $2, updated_at = NOW() WHERE id = $1 \
         RETURNING id, name, owner_id, created_at, updated_at, settings, nda",
    )
    .bind(id)
    .bind(nda)
    .fetch_optional(&mut **db)
    .await?;

    Ok(row.map(|r| (r, old_nda)))
}

/// Delete a project and all associated data.
/// Cleans up FK dependencies in correct order.
pub async fn delete_project(
    db: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    // Check project exists
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1)")
        .bind(id)
        .fetch_one(&mut *db)
        .await?;
    if !exists {
        return Ok(false);
    }

    // Delete in FK dependency order:
    // 1. message_evidence → messages → threads (recursive thread delete with children)
    sqlx::query(
        "DELETE FROM message_evidence WHERE message_id IN \
         (SELECT id FROM messages WHERE thread_id IN \
          (SELECT id FROM threads WHERE project_id = $1))",
    )
    .bind(id)
    .execute(&mut *db)
    .await?;

    sqlx::query(
        "DELETE FROM messages WHERE thread_id IN (SELECT id FROM threads WHERE project_id = $1)",
    )
    .bind(id)
    .execute(&mut *db)
    .await?;

    // 2. email_log and email_scheduled (thread FK, no cascade)
    sqlx::query(
        "DELETE FROM email_log WHERE thread_id IN (SELECT id FROM threads WHERE project_id = $1)",
    )
    .bind(id)
    .execute(&mut *db)
    .await?;

    sqlx::query(
        "DELETE FROM email_scheduled WHERE thread_id IN (SELECT id FROM threads WHERE project_id = $1)",
    )
    .bind(id)
    .execute(&mut *db)
    .await?;

    sqlx::query("DELETE FROM threads WHERE project_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    // 3. tool_run_artifacts → tool_run_events → tool_runs
    sqlx::query(
        "DELETE FROM tool_run_artifacts WHERE tool_run_id IN \
         (SELECT id FROM tool_runs WHERE project_id = $1)",
    )
    .bind(id)
    .execute(&mut *db)
    .await?;

    sqlx::query(
        "DELETE FROM tool_run_events WHERE tool_run_id IN \
         (SELECT id FROM tool_runs WHERE project_id = $1)",
    )
    .bind(id)
    .execute(&mut *db)
    .await?;

    sqlx::query("DELETE FROM tool_runs WHERE project_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    // 4. artifacts
    sqlx::query("DELETE FROM artifacts WHERE project_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    // 5. project_members, project_hooks (no cascade)
    sqlx::query("DELETE FROM project_members WHERE project_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    sqlx::query("DELETE FROM project_hooks WHERE project_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    // 6. email_log by project_id (no cascade)
    sqlx::query("DELETE FROM email_log WHERE project_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    // 7. Finally delete the project (cascade handles llm_usage_log, embeddings, web_fetch_rules, email_recipient_rules, email_scheduled)
    let result = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Get a specific setting value from a project's JSONB settings.
pub async fn get_project_setting(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    key: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT settings->$2 FROM projects WHERE id = $1",
    )
    .bind(id)
    .bind(key)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.0))
}
