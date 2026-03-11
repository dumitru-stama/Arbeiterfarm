use crate::app::EmailRuleAction;
use crate::backend::Backend;
use uuid::Uuid;

pub async fn handle(action: EmailRuleAction, backend: &dyn Backend) -> anyhow::Result<()> {
    match action {
        EmailRuleAction::Add {
            block,
            allow,
            email,
            domain,
            domain_suffix,
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

            let (pattern_type, pattern) = if let Some(e) = email {
                ("exact_email", e)
            } else if let Some(d) = domain {
                ("domain", d)
            } else if let Some(ds) = domain_suffix {
                ("domain_suffix", ds)
            } else {
                anyhow::bail!("must specify --email, --domain, or --domain-suffix");
            };

            let project_id = project
                .as_deref()
                .map(|p| p.parse::<Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let scope = if project_id.is_some() {
                "project"
            } else {
                "global"
            };

            let rule = backend
                .add_email_rule(
                    scope,
                    project_id,
                    rule_type,
                    &pattern_type,
                    &pattern,
                    description.as_deref(),
                )
                .await?;

            println!("Added email rule: {}", rule.id);
            println!("  scope: {} | type: {} | pattern: {} = {}", rule.scope, rule.rule_type, rule.pattern_type, rule.pattern);
        }

        EmailRuleAction::Remove { id } => {
            let rule_id: Uuid = id.parse().map_err(|_| anyhow::anyhow!("invalid UUID"))?;
            let removed = backend.remove_email_rule(rule_id).await?;
            if removed {
                println!("Removed email rule {id}");
            } else {
                println!("Rule {id} not found");
            }
        }

        EmailRuleAction::List { project } => {
            let project_id = project
                .as_deref()
                .map(|p| p.parse::<Uuid>())
                .transpose()
                .map_err(|_| anyhow::anyhow!("invalid project UUID"))?;

            let rules = backend.list_email_rules(project_id).await?;
            if rules.is_empty() {
                println!("No email recipient rules configured.");
                return Ok(());
            }

            println!("{:<38} {:<8} {:<6} {:<14} {}", "ID", "SCOPE", "TYPE", "PATTERN_TYPE", "PATTERN");
            for r in &rules {
                println!(
                    "{:<38} {:<8} {:<6} {:<14} {}",
                    r.id, r.scope, r.rule_type, r.pattern_type, r.pattern
                );
            }
        }
    }

    Ok(())
}
