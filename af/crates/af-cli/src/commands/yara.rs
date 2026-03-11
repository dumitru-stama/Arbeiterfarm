use crate::app::YaraCommand;
use crate::backend::Backend;

pub async fn handle(backend: &dyn Backend, cmd: YaraCommand) -> anyhow::Result<()> {
    use crate::app::YaraAction;

    match cmd.action {
        YaraAction::List { project, filter } => {
            let project_id = project
                .as_deref()
                .map(|s| s.parse::<uuid::Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rules = backend.list_yara_rules(project_id).await?;
            if rules.is_empty() {
                println!("No YARA rules found.");
            } else {
                for r in &rules {
                    // Apply optional filter
                    if let Some(ref f) = filter {
                        let f_lower = f.to_lowercase();
                        let name_match = r.name.to_lowercase().contains(&f_lower);
                        let desc_match = r
                            .description
                            .as_deref()
                            .map(|d| d.to_lowercase().contains(&f_lower))
                            .unwrap_or(false);
                        let tag_match = r.tags.iter().any(|t| t.to_lowercase().contains(&f_lower));
                        if !name_match && !desc_match && !tag_match {
                            continue;
                        }
                    }

                    let scope = if let Some(pid) = r.project_id {
                        format!("project:{}", &pid.to_string()[..8])
                    } else {
                        "global".to_string()
                    };
                    let tags = if r.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", r.tags.join(", "))
                    };
                    let desc = r.description.as_deref().unwrap_or("");
                    println!(
                        "{} | {} | {}{} {}",
                        &r.id.to_string()[..8],
                        scope,
                        r.name,
                        tags,
                        if desc.is_empty() {
                            String::new()
                        } else {
                            format!("({})", desc)
                        }
                    );
                }
            }
        }

        YaraAction::Show { id } => {
            let uuid: uuid::Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid UUID"))?;
            match backend.get_yara_rule(uuid).await? {
                Some(rule) => {
                    println!("Name: {}", rule.name);
                    if let Some(ref desc) = rule.description {
                        println!("Description: {}", desc);
                    }
                    if !rule.tags.is_empty() {
                        println!("Tags: {}", rule.tags.join(", "));
                    }
                    if let Some(pid) = rule.project_id {
                        println!("Project: {}", pid);
                    } else {
                        println!("Scope: global");
                    }
                    println!("Created: {}", rule.created_at);
                    println!("---");
                    println!("{}", rule.source);
                }
                None => {
                    println!("YARA rule {id} not found");
                }
            }
        }

        YaraAction::Remove { id } => {
            let uuid: uuid::Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid UUID"))?;
            let deleted = backend.remove_yara_rule(uuid).await?;
            if deleted {
                println!("Removed YARA rule {id}");
            } else {
                println!("YARA rule {id} not found");
            }
        }

        YaraAction::ScanResults { artifact, rule } => {
            let artifact_id = artifact
                .as_deref()
                .map(|s| s.parse::<uuid::Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid artifact UUID"))?;

            let results = backend
                .list_yara_scan_results(artifact_id, rule.as_deref())
                .await?;
            if results.is_empty() {
                println!("No scan results found.");
            } else {
                for r in &results {
                    println!(
                        "{} | artifact:{} | {} | matches: {} | {}",
                        &r.id.to_string()[..8],
                        &r.artifact_id.to_string()[..8],
                        r.rule_name,
                        r.match_count,
                        r.matched_at,
                    );
                }
            }
        }
    }

    Ok(())
}
