use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub sha256: String,
    pub filename: String,
    pub mime_type: Option<String>,
    #[serde(default)]
    pub source_tool_run_id: Option<Uuid>,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

fn default_metadata() -> serde_json::Value {
    serde_json::json!({})
}

pub async fn create_artifact(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    sha256: &str,
    filename: &str,
    mime_type: Option<&str>,
    source_tool_run_id: Option<Uuid>,
) -> Result<ArtifactRow, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "INSERT INTO artifacts (project_id, sha256, filename, mime_type, source_tool_run_id)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at",
    )
    .bind(project_id)
    .bind(sha256)
    .bind(filename)
    .bind(mime_type)
    .bind(source_tool_run_id)
    .fetch_one(db)
    .await
}

pub async fn create_artifact_with_description(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    sha256: &str,
    filename: &str,
    mime_type: Option<&str>,
    source_tool_run_id: Option<Uuid>,
    description: &str,
) -> Result<ArtifactRow, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "INSERT INTO artifacts (project_id, sha256, filename, mime_type, source_tool_run_id, description)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at",
    )
    .bind(project_id)
    .bind(sha256)
    .bind(filename)
    .bind(mime_type)
    .bind(source_tool_run_id)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn create_artifact_with_metadata(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    sha256: &str,
    filename: &str,
    mime_type: Option<&str>,
    source_tool_run_id: Option<Uuid>,
    metadata: &serde_json::Value,
) -> Result<ArtifactRow, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "INSERT INTO artifacts (project_id, sha256, filename, mime_type, source_tool_run_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at",
    )
    .bind(project_id)
    .bind(sha256)
    .bind(filename)
    .bind(mime_type)
    .bind(source_tool_run_id)
    .bind(metadata)
    .fetch_one(db)
    .await
}

pub async fn get_artifact(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
) -> Result<Option<ArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "SELECT id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at
         FROM artifacts WHERE id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await
    .map(Some)
    .or_else(|e| match e {
        sqlx::Error::RowNotFound => Ok(None),
        other => Err(other),
    })
}

pub async fn list_artifacts(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
) -> Result<Vec<ArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "SELECT id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at
         FROM artifacts WHERE project_id = $1 ORDER BY created_at DESC",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}

pub async fn get_artifacts_by_ids(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    ids: &[Uuid],
) -> Result<Vec<ArtifactRow>, sqlx::Error> {
    // sqlx doesn't natively support IN ($1) with Vec<Uuid> for runtime queries.
    // Use ANY($1) with a slice instead.
    sqlx::query_as::<_, ArtifactRow>(
        "SELECT id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at
         FROM artifacts WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await
}

/// Find artifacts with `fan_out_from` metadata created since a given timestamp.
pub async fn find_fanout_artifacts_since(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<ArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "SELECT id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at
         FROM artifacts
         WHERE project_id = $1 AND created_at >= $2 AND metadata->>'fan_out_from' IS NOT NULL
         ORDER BY created_at ASC",
    )
    .bind(project_id)
    .bind(since)
    .fetch_all(db)
    .await
}

/// Find artifacts with `repivot_from` metadata created since a given timestamp.
pub async fn find_repivot_artifacts_since(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<ArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "SELECT id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at
         FROM artifacts
         WHERE project_id = $1 AND created_at >= $2 AND metadata->>'repivot_from' IS NOT NULL
         ORDER BY created_at ASC",
    )
    .bind(project_id)
    .bind(since)
    .fetch_all(db)
    .await
}

/// List artifacts scoped to a target sample: the target sample itself plus any
/// generated artifacts whose tool run had the target as input.
pub async fn list_artifacts_for_sample(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
    target_artifact_id: Uuid,
) -> Result<Vec<ArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "SELECT a.id, a.project_id, a.sha256, a.filename, a.mime_type, \
                a.source_tool_run_id, a.metadata, a.description, a.created_at \
         FROM artifacts a \
         WHERE a.project_id = $1 AND ( \
             a.id = $2 \
             OR (a.source_tool_run_id IS NOT NULL AND EXISTS ( \
                 SELECT 1 FROM tool_run_artifacts tra \
                 WHERE tra.tool_run_id = a.source_tool_run_id \
                   AND tra.artifact_id = $2 \
                   AND tra.role = 'input' \
             )) \
         ) \
         ORDER BY a.created_at DESC",
    )
    .bind(project_id)
    .bind(target_artifact_id)
    .fetch_all(db)
    .await
}

/// Delete an artifact and its FK links. Returns true if the artifact existed.
/// Must run inside a transaction that also covers tool_run_artifacts.
pub async fn delete_artifact(
    db: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    // Remove FK links first (tool_run_artifacts has no CASCADE)
    sqlx::query("DELETE FROM tool_run_artifacts WHERE artifact_id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;
    let result = sqlx::query("DELETE FROM artifacts WHERE id = $1")
        .bind(id)
        .execute(&mut *db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete all generated artifacts (those with source_tool_run_id set) for a project.
/// Returns the number of deleted artifacts.
pub async fn delete_generated_artifacts(
    db: &mut sqlx::PgConnection,
    project_id: Uuid,
) -> Result<u64, sqlx::Error> {
    // Remove FK links first
    sqlx::query(
        "DELETE FROM tool_run_artifacts WHERE artifact_id IN \
         (SELECT id FROM artifacts WHERE project_id = $1 AND source_tool_run_id IS NOT NULL)",
    )
    .bind(project_id)
    .execute(&mut *db)
    .await?;
    let result = sqlx::query(
        "DELETE FROM artifacts WHERE project_id = $1 AND source_tool_run_id IS NOT NULL",
    )
    .bind(project_id)
    .execute(&mut *db)
    .await?;
    Ok(result.rows_affected())
}

/// Update an artifact's description. Returns the updated row, or None if the artifact doesn't exist.
pub async fn update_artifact_description(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    id: Uuid,
    description: &str,
) -> Result<Option<ArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtifactRow>(
        "UPDATE artifacts SET description = $2 WHERE id = $1
         RETURNING id, project_id, sha256, filename, mime_type, source_tool_run_id, metadata, description, created_at",
    )
    .bind(id)
    .bind(description)
    .fetch_one(db)
    .await
    .map(Some)
    .or_else(|e| match e {
        sqlx::Error::RowNotFound => Ok(None),
        other => Err(other),
    })
}
