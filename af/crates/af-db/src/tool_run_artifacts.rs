use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ToolRunArtifactRow {
    pub tool_run_id: Uuid,
    pub artifact_id: Uuid,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

pub async fn link_artifact(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    tool_run_id: Uuid,
    artifact_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tool_run_artifacts (tool_run_id, artifact_id, role)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(tool_run_id)
    .bind(artifact_id)
    .bind(role)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_for_run(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    tool_run_id: Uuid,
) -> Result<Vec<ToolRunArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ToolRunArtifactRow>(
        "SELECT tool_run_id, artifact_id, role, created_at
         FROM tool_run_artifacts WHERE tool_run_id = $1",
    )
    .bind(tool_run_id)
    .fetch_all(db)
    .await
}

/// For each generated artifact (identified by its source_tool_run_id), find the
/// input sample artifact that was fed into that tool run.
/// Returns (generated_artifact_id, parent_sample_id) pairs.
///
/// Chain: generated artifact → source_tool_run_id → tool_run_artifacts(role='input') → parent
pub async fn resolve_parent_samples(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    project_id: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT a.id AS generated_id, tra.artifact_id AS parent_id \
         FROM artifacts a \
         JOIN tool_run_artifacts tra ON tra.tool_run_id = a.source_tool_run_id AND tra.role = 'input' \
         WHERE a.project_id = $1 AND a.source_tool_run_id IS NOT NULL",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}
