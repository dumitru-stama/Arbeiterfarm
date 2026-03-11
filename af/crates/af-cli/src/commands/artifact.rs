use crate::app::{ArtifactAction, ArtifactCommand};
use crate::backend::Backend;
use crate::CliConfig;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn handle(
    config: &CliConfig,
    backend: &dyn Backend,
    cmd: ArtifactCommand,
) -> anyhow::Result<()> {
    match cmd.action {
        ArtifactAction::Add { file, project } => {
            let project_id: Uuid = project.parse()?;
            let path = std::path::Path::new(&file);
            let filename = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let data = tokio::fs::read(path).await?;

            // Compute sha256 before ingestion (needed for hooks)
            let sha256 = {
                use sha2::{Digest, Sha256};
                hex::encode(Sha256::digest(&data))
            };

            let artifact = backend.upload_artifact(project_id, &filename, &data).await?;
            println!("Artifact added: {}", artifact.id);

            // Fire artifact_uploaded hooks only for local backend.
            // Remote server fires hooks itself on upload.
            if backend.is_local() {
                if let Some(pool) = &config.pool {
                    fire_artifact_hooks_cli(
                        config,
                        pool,
                        project_id,
                        artifact.id,
                        &filename,
                        &sha256,
                    )
                    .await;
                }
            }
        }
        ArtifactAction::Info { id } => {
            let artifact_id: Uuid = id.parse()?;
            let artifact = backend
                .get_artifact(artifact_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("artifact {artifact_id} not found"))?;
            println!("ID:        {}", artifact.id);
            println!("Project:   {}", artifact.project_id);
            println!("Filename:  {}", artifact.filename);
            println!("SHA256:    {}", artifact.sha256);
            println!(
                "MIME:      {}",
                artifact.mime_type.as_deref().unwrap_or("(unknown)")
            );
            println!(
                "Desc:      {}",
                artifact.description.as_deref().unwrap_or("(none)")
            );
            if let Some(run_id) = artifact.source_tool_run_id {
                println!("Source:    tool_run:{run_id}");
            }
            println!("Created:   {}", artifact.created_at.format("%Y-%m-%d %H:%M:%S"));
        }
        ArtifactAction::List { project } => {
            let project_id: Uuid = project.parse()?;
            let artifacts = backend.list_artifacts(project_id).await?;
            if artifacts.is_empty() {
                println!("No artifacts found.");
            } else {
                for a in artifacts {
                    let desc = a
                        .description
                        .as_deref()
                        .map(|d| if d.len() > 40 { format!("{}...", &d[..d.floor_char_boundary(40)]) } else { d.to_string() })
                        .unwrap_or_default();
                    println!(
                        "{}  {}  {}  {}  {}",
                        a.id,
                        a.filename,
                        a.sha256,
                        a.created_at.format("%Y-%m-%d %H:%M"),
                        desc,
                    );
                }
            }
        }
        ArtifactAction::Delete { id, yes } => {
            let artifact_id: Uuid = id.parse()?;
            if !yes {
                eprint!("Delete artifact {artifact_id}? [y/N] ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            let deleted = backend.delete_artifact(artifact_id).await?;
            if deleted {
                println!("Artifact {artifact_id} deleted.");
            } else {
                println!("Artifact {artifact_id} not found.");
            }
        }
        ArtifactAction::CleanGenerated { project, yes } => {
            let project_id: Uuid = project.parse()?;
            if !yes {
                eprint!("Delete all generated artifacts in project {project_id}? [y/N] ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            let count = backend.delete_generated_artifacts(project_id).await?;
            println!("Deleted {count} generated artifact(s).");
        }
        ArtifactAction::Describe { id, description } => {
            let artifact_id: Uuid = id.parse()?;
            if description.is_empty() || description.len() > 1000 {
                anyhow::bail!("description must be 1-1000 characters");
            }
            let updated = backend
                .update_artifact_description(artifact_id, &description)
                .await?
                .ok_or_else(|| anyhow::anyhow!("artifact {artifact_id} not found"))?;
            println!("Description set for {} ({})", updated.id, updated.filename);
        }
    }
    Ok(())
}

/// Fire artifact_uploaded hooks from the CLI. Requires an LLM backend; skips silently if none.
async fn fire_artifact_hooks_cli(
    config: &CliConfig,
    pool: &PgPool,
    project_id: Uuid,
    artifact_id: Uuid,
    filename: &str,
    sha256: &str,
) {
    // Check if any hooks exist before building the full state
    let hooks = match af_db::project_hooks::list_enabled_by_event(pool, project_id, "artifact_uploaded").await {
        Ok(h) => h,
        Err(_) => return,
    };
    if hooks.is_empty() {
        return;
    }

    let router = match config.router.as_ref() {
        Some(r) => r.clone(),
        None => {
            eprintln!("[hooks] skipping artifact hooks: no LLM backend configured");
            return;
        }
    };

    let project_name = af_db::projects::get_project(pool, project_id)
        .await
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();

    let state = std::sync::Arc::new(af_api::AppState {
        pool: pool.clone(),
        specs: config.specs.clone(),
        executors: config.executors.clone(),
        evidence_resolvers: config.evidence_resolvers.clone(),
        post_tool_hook: config.post_tool_hook.clone(),
        core_config: config.core_config.clone(),
        agent_configs: config.agent_configs.clone(),
        router,
        upload_max_bytes: 0,
        rate_limiter: None,
        cors_origin: None,
        ghidra_cache_dir: config.ghidra_cache_dir.clone(),
        source_map: config.source_map.clone(),
        security_config: af_api::SecurityConfig {
            sandbox_available: false,
            sandbox_enforced: false,
            tls_enabled: false,
        },
        stream_tracker: af_api::ActiveStreamTracker::new(0),
        compaction_threshold: config.compaction.threshold,
        summarization_backend: None,
    });

    println!("[hooks] firing {} artifact_uploaded hook(s)...", hooks.len());
    af_api::hooks::fire_artifact_hooks_blocking(
        &state,
        project_id,
        &project_name,
        artifact_id,
        filename,
        sha256,
    )
    .await;
    println!("[hooks] done");
}
