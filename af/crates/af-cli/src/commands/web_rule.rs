use crate::app::WebRuleCommand;
use crate::backend::Backend;

pub async fn handle(backend: &dyn Backend, cmd: WebRuleCommand) -> anyhow::Result<()> {
    use crate::app::WebRuleAction;

    match cmd.action {
        WebRuleAction::Add {
            block,
            allow,
            domain,
            domain_suffix,
            url_prefix,
            url_regex,
            ip_cidr,
            description,
            project,
        } => {
            let rule_type = if block {
                "block"
            } else if allow {
                "allow"
            } else {
                anyhow::bail!("must specify --block or --allow");
            };

            let (pattern_type, pattern) = if let Some(d) = domain {
                ("domain", d)
            } else if let Some(ds) = domain_suffix {
                ("domain_suffix", ds)
            } else if let Some(up) = url_prefix {
                ("url_prefix", up)
            } else if let Some(ur) = url_regex {
                ("url_regex", ur)
            } else if let Some(ic) = ip_cidr {
                ("ip_cidr", ic)
            } else {
                anyhow::bail!(
                    "must specify one of: --domain, --domain-suffix, --url-prefix, --url-regex, --ip-cidr"
                );
            };

            let project_id = project
                .as_deref()
                .map(|s| s.parse::<uuid::Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let scope = if project_id.is_some() {
                "project"
            } else {
                "global"
            };

            let rule = backend
                .add_web_rule(scope, project_id, rule_type, pattern_type, &pattern, description.as_deref())
                .await?;
            println!(
                "Added {} rule: {} {} = \"{}\" (id: {})",
                rule.rule_type, rule.scope, rule.pattern_type, rule.pattern, rule.id
            );
        }

        WebRuleAction::Remove { id } => {
            let uuid: uuid::Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid UUID"))?;
            let deleted = backend.remove_web_rule(uuid).await?;
            if deleted {
                println!("Removed rule {id}");
            } else {
                println!("Rule {id} not found");
            }
        }

        WebRuleAction::List { project } => {
            let project_id = project
                .as_deref()
                .map(|s| s.parse::<uuid::Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;
            let rules = backend.list_web_rules(project_id).await?;
            if rules.is_empty() {
                println!("No URL rules configured.");
            } else {
                for r in &rules {
                    let desc = r.description.as_deref().unwrap_or("");
                    let scope = if let Some(pid) = r.project_id {
                        format!("project:{}", &pid.to_string()[..8])
                    } else {
                        "global".to_string()
                    };
                    println!(
                        "{} | {} | {} {} = \"{}\" {}",
                        &r.id.to_string()[..8],
                        r.rule_type,
                        scope,
                        r.pattern_type,
                        r.pattern,
                        if desc.is_empty() {
                            String::new()
                        } else {
                            format!("({})", desc)
                        }
                    );
                }
            }
        }

        WebRuleAction::BlockCountry { code, name } => {
            let row = backend
                .add_country_block(&code.to_uppercase(), name.as_deref())
                .await?;
            println!(
                "Blocked country: {} ({})",
                row.country_code,
                row.country_name.as_deref().unwrap_or("unnamed")
            );
        }

        WebRuleAction::UnblockCountry { code } => {
            let deleted = backend.remove_country_block(&code.to_uppercase()).await?;
            if deleted {
                println!("Unblocked country: {}", code.to_uppercase());
            } else {
                println!("Country {} was not blocked", code.to_uppercase());
            }
        }

        WebRuleAction::ListCountries => {
            let blocks = backend.list_country_blocks().await?;
            if blocks.is_empty() {
                println!("No country blocks configured.");
            } else {
                for b in &blocks {
                    println!(
                        "{} | {}",
                        b.country_code,
                        b.country_name.as_deref().unwrap_or("unnamed")
                    );
                }
            }
        }
    }

    Ok(())
}
