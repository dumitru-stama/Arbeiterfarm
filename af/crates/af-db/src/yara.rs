use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct YaraRuleRow {
    pub id: Uuid,
    pub name: String,
    pub source: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub project_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct YaraScanResultRow {
    pub id: Uuid,
    pub artifact_id: Uuid,
    pub rule_name: String,
    pub match_count: i32,
    pub match_data: Option<serde_json::Value>,
    pub matched_at: DateTime<Utc>,
    pub tool_run_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Rules CRUD
// ---------------------------------------------------------------------------

pub async fn list_rules<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    project_id: Option<Uuid>,
) -> Result<Vec<YaraRuleRow>, sqlx::Error> {
    if let Some(pid) = project_id {
        sqlx::query_as::<_, YaraRuleRow>(
            "SELECT * FROM re.yara_rules WHERE (project_id IS NULL OR project_id = $1) ORDER BY name",
        )
        .bind(pid)
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, YaraRuleRow>(
            "SELECT * FROM re.yara_rules ORDER BY name",
        )
        .fetch_all(db)
        .await
    }
}

pub async fn get_rule<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<Option<YaraRuleRow>, sqlx::Error> {
    sqlx::query_as::<_, YaraRuleRow>(
        "SELECT * FROM re.yara_rules WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn delete_rule<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM re.yara_rules WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Scan results
// ---------------------------------------------------------------------------

pub async fn list_scan_results<'e, E: Executor<'e, Database = Postgres>>(
    db: E,
    artifact_id: Option<Uuid>,
    rule_name: Option<&str>,
) -> Result<Vec<YaraScanResultRow>, sqlx::Error> {
    match (artifact_id, rule_name) {
        (Some(aid), Some(rn)) => {
            sqlx::query_as::<_, YaraScanResultRow>(
                "SELECT * FROM re.yara_scan_results WHERE artifact_id = $1 AND rule_name = $2 ORDER BY matched_at DESC",
            )
            .bind(aid)
            .bind(rn)
            .fetch_all(db)
            .await
        }
        (Some(aid), None) => {
            sqlx::query_as::<_, YaraScanResultRow>(
                "SELECT * FROM re.yara_scan_results WHERE artifact_id = $1 ORDER BY matched_at DESC",
            )
            .bind(aid)
            .fetch_all(db)
            .await
        }
        (None, Some(rn)) => {
            sqlx::query_as::<_, YaraScanResultRow>(
                "SELECT * FROM re.yara_scan_results WHERE rule_name = $1 ORDER BY matched_at DESC",
            )
            .bind(rn)
            .fetch_all(db)
            .await
        }
        (None, None) => {
            sqlx::query_as::<_, YaraScanResultRow>(
                "SELECT * FROM re.yara_scan_results ORDER BY matched_at DESC LIMIT 100",
            )
            .fetch_all(db)
            .await
        }
    }
}
