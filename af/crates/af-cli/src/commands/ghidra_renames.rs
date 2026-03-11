use crate::CliConfig;
use uuid::Uuid;

pub async fn handle(config: &CliConfig, action: crate::app::GhidraRenamesAction) -> anyhow::Result<()> {
    let pool = crate::get_pool_from(&config.pool).await?;

    match action {
        crate::app::GhidraRenamesAction::List { project, sha256 } => {
            let project_id = Uuid::parse_str(&project)
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rows = sqlx::query_as::<_, RenameRow>(
                "SELECT old_name, new_name, address, updated_at \
                 FROM re.ghidra_function_renames \
                 WHERE project_id = $1 AND sha256 = $2 \
                 ORDER BY old_name",
            )
            .bind(project_id)
            .bind(&sha256)
            .fetch_all(&pool)
            .await?;

            if rows.is_empty() {
                println!("No renames found for SHA256 {sha256} in project {project}.");
                return Ok(());
            }

            println!(
                "{:<30}  {:<30}  {:<16}  {}",
                "OLD NAME", "NEW NAME", "ADDRESS", "UPDATED"
            );
            for r in &rows {
                println!(
                    "{:<30}  {:<30}  {:<16}  {}",
                    r.old_name,
                    r.new_name,
                    r.address.as_deref().unwrap_or("-"),
                    r.updated_at.format("%Y-%m-%d %H:%M:%S"),
                );
            }
            println!("\n{} renames total.", rows.len());
        }

        crate::app::GhidraRenamesAction::Suggest { project, sha256 } => {
            let project_id = Uuid::parse_str(&project)
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rows = sqlx::query_as::<_, SuggestionRow>(
                "SELECT r.old_name, r.new_name, r.address, \
                        string_agg(DISTINCT p.name, ', ' ORDER BY p.name) as project_names, \
                        COUNT(DISTINCT r.project_id) as source_count \
                 FROM re.ghidra_function_renames r \
                 JOIN projects p ON p.id = r.project_id \
                 WHERE r.sha256 = $1 \
                   AND r.project_id <> $2 \
                   AND r.project_id IN (SELECT af_shareable_projects()) \
                 GROUP BY r.old_name, r.new_name, r.address \
                 ORDER BY r.old_name",
            )
            .bind(&sha256)
            .bind(project_id)
            .fetch_all(&pool)
            .await?;

            if rows.is_empty() {
                println!("No rename suggestions found from other projects for SHA256 {sha256}.");
                return Ok(());
            }

            println!(
                "{:<30}  {:<30}  {:<16}  {:<6}  {}",
                "OLD NAME", "NEW NAME", "ADDRESS", "COUNT", "SOURCE PROJECTS"
            );
            for r in &rows {
                println!(
                    "{:<30}  {:<30}  {:<16}  {:<6}  {}",
                    r.old_name,
                    r.new_name,
                    r.address.as_deref().unwrap_or("-"),
                    r.source_count,
                    r.project_names,
                );
            }
            println!("\n{} suggestions total. Use ghidra.rename to apply desired renames.", rows.len());
        }

        crate::app::GhidraRenamesAction::Import { project, sha256, from_project } => {
            let target_id = Uuid::parse_str(&project)
                .map_err(|_| anyhow::anyhow!("invalid target project UUID"))?;
            let source_id = Uuid::parse_str(&from_project)
                .map_err(|_| anyhow::anyhow!("invalid source project UUID"))?;

            let result = sqlx::query(
                "INSERT INTO re.ghidra_function_renames (project_id, sha256, old_name, new_name, address) \
                 SELECT $1, sha256, old_name, new_name, address \
                 FROM re.ghidra_function_renames \
                 WHERE project_id = $2 AND sha256 = $3 \
                   AND project_id IN (SELECT af_shareable_projects()) \
                 ON CONFLICT (project_id, sha256, old_name) DO NOTHING",
            )
            .bind(target_id)
            .bind(source_id)
            .bind(&sha256)
            .execute(&pool)
            .await?;

            println!(
                "Imported {} renames from project {} into project {} for SHA256 {}.",
                result.rows_affected(),
                from_project,
                project,
                sha256,
            );
        }
    }

    Ok(())
}

#[derive(sqlx::FromRow)]
struct RenameRow {
    old_name: String,
    new_name: String,
    address: Option<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct SuggestionRow {
    old_name: String,
    new_name: String,
    address: Option<String>,
    project_names: String,
    source_count: i64,
}
