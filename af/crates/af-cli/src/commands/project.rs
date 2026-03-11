use crate::app::{ProjectAction, ProjectCommand};
use crate::backend::Backend;
use af_db::project_members::ALL_USERS_SENTINEL;
use uuid::Uuid;

/// Resolve "@all" to sentinel UUID, otherwise parse as UUID.
fn resolve_user_id(s: &str) -> anyhow::Result<Uuid> {
    if s == "@all" {
        Ok(ALL_USERS_SENTINEL)
    } else {
        s.parse::<Uuid>()
            .map_err(|_| anyhow::anyhow!("invalid user ID: {s}"))
    }
}

/// Format user_id for display — show "@all" for sentinel.
fn display_user_id(uid: Uuid) -> String {
    if uid == ALL_USERS_SENTINEL {
        "@all".to_string()
    } else {
        uid.to_string()
    }
}

pub async fn handle(backend: &dyn Backend, cmd: ProjectCommand) -> anyhow::Result<()> {
    match cmd.action {
        ProjectAction::Create { name, nda } => {
            let project = backend.create_project(&name).await?;
            if nda {
                let _ = backend.set_nda(project.id, true).await?;
                println!("Project created (NDA): {}", project.id);
            } else {
                println!("Project created: {}", project.id);
            }
        }
        ProjectAction::List => {
            let projects = backend.list_projects().await?;
            if projects.is_empty() {
                println!("No projects found.");
            } else {
                for p in projects {
                    let nda_tag = if p.nda { " [NDA]" } else { "" };
                    println!("{}  {}{nda_tag}  {}", p.id, p.name, p.created_at.format("%Y-%m-%d %H:%M"));
                }
            }
        }
        ProjectAction::Members { project } => {
            let project_id: Uuid = project.parse()?;
            let members = backend.list_members(project_id).await?;
            if members.is_empty() {
                println!("No members found.");
            } else {
                println!("{:<40} {:<14} {}", "USER", "ROLE", "DISPLAY NAME");
                for m in members {
                    println!(
                        "{:<40} {:<14} {}",
                        display_user_id(m.user_id),
                        m.role,
                        m.display_name.as_deref().unwrap_or("-"),
                    );
                }
            }
        }
        ProjectAction::AddMember {
            project,
            user,
            role,
        } => {
            let project_id: Uuid = project.parse()?;
            let user_id = resolve_user_id(&user)?;

            if !matches!(role.as_str(), "manager" | "collaborator" | "viewer") {
                anyhow::bail!("role must be one of: manager, collaborator, viewer");
            }

            backend.add_member(project_id, user_id, &role).await?;
            println!(
                "Added {} as {} to project {}",
                display_user_id(user_id),
                role,
                project_id
            );
        }
        ProjectAction::RemoveMember { project, user } => {
            let project_id: Uuid = project.parse()?;
            let user_id = resolve_user_id(&user)?;

            backend.remove_member(project_id, user_id).await?;
            println!(
                "Removed {} from project {}",
                display_user_id(user_id),
                project_id
            );
        }
        ProjectAction::Delete { project, yes } => {
            let project_id: Uuid = project.parse()?;
            if !yes {
                eprint!("Delete project {project_id} and ALL its data (artifacts, conversations, hooks)? [y/N] ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            let deleted = backend.delete_project(project_id).await?;
            if deleted {
                println!("Project {project_id} deleted.");
            } else {
                println!("Project {project_id} not found.");
            }
        }
        ProjectAction::Nda { project, on, off } => {
            let project_id: Uuid = project.parse()?;
            if !on && !off {
                anyhow::bail!("specify --on or --off");
            }
            let nda = on;
            let (row, old_nda) = backend.set_nda(project_id, nda).await?;
            let state = if row.nda { "ON" } else { "OFF" };
            println!("NDA {state} for project {project_id}");
            if old_nda != nda {
                if nda {
                    println!("Note: Future Ghidra analyses will use an isolated cache.");
                    println!("Existing shared cache entries (if any) are NOT removed — they may be used by other projects.");
                } else {
                    println!("WARNING: Removing NDA makes ALL project data visible in cross-project searches.");
                    println!("Future Ghidra analyses will use the shared cache.");
                    println!("Existing isolated cache entries will be orphaned (can be cleaned manually).");
                }
            }
        }
        ProjectAction::Settings { project, set } => {
            let project_id: Uuid = project.parse()?;

            if let Some(kv) = set {
                let (key, value) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("expected key=value format"))?;

                // NDA is a dedicated column, not a JSONB setting
                if key == "nda" {
                    let nda = match value {
                        "true" => true,
                        "false" => false,
                        _ => anyhow::bail!("nda must be 'true' or 'false'"),
                    };
                    let (row, old_nda) = backend.set_nda(project_id, nda).await?;
                    println!("Updated NDA={} for project {}:", row.nda, project_id);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&row.settings)?
                    );
                    if old_nda != nda {
                        if nda {
                            println!("Note: Future Ghidra analyses will use an isolated cache.");
                        } else {
                            println!("WARNING: Removing NDA makes ALL project data visible in cross-project searches.");
                        }
                    }
                } else {
                    let value: serde_json::Value = match value {
                        "true" => serde_json::Value::Bool(true),
                        "false" => serde_json::Value::Bool(false),
                        "null" => serde_json::Value::Null,
                        other => {
                            // Try parsing as number, fall back to string
                            if let Ok(n) = other.parse::<i64>() {
                                serde_json::json!(n)
                            } else {
                                serde_json::json!(other)
                            }
                        }
                    };
                    let settings = serde_json::json!({ key: value });
                    let row = backend.update_project_settings(project_id, &settings).await?;
                    println!("Updated settings for project {}:", project_id);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&row.settings)?
                    );
                }
            } else {
                let settings = backend.get_project_settings(project_id).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&settings)?
                );
            }
        }
    }
    Ok(())
}
