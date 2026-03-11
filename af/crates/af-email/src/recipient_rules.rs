use af_db::email::EmailRecipientRuleRow;

/// Result of evaluating recipient rules.
#[derive(Debug, Clone)]
pub enum RuleResult {
    Allowed,
    Blocked(String),
}

/// Evaluate a single email address against the rule set.
/// Block-wins semantics (identical to web fetch rules):
/// 1. If any block rule matches → blocked
/// 2. If no allow rules exist → allowed (blocklist-only mode)
/// 3. If allow rules exist → must match at least one
pub fn evaluate_recipient(email: &str, rules: &[EmailRecipientRuleRow]) -> RuleResult {
    let email_lower = email.to_lowercase();

    // Check block rules first — block always wins
    for rule in rules.iter().filter(|r| r.rule_type == "block") {
        if matches_email_rule(&email_lower, rule) {
            return RuleResult::Blocked(format!(
                "'{}' blocked by {} rule: {}",
                email, rule.pattern_type, rule.pattern
            ));
        }
    }

    // Collect allow rules
    let allow_rules: Vec<&EmailRecipientRuleRow> =
        rules.iter().filter(|r| r.rule_type == "allow").collect();

    // No allow rules → blocklist-only mode, default allow
    if allow_rules.is_empty() {
        return RuleResult::Allowed;
    }

    // Allowlist mode — must match at least one
    for rule in &allow_rules {
        if matches_email_rule(&email_lower, rule) {
            return RuleResult::Allowed;
        }
    }

    RuleResult::Blocked(format!("'{}' not matched by any allow rule", email))
}

/// Evaluate ALL recipients (to + cc + bcc). If ANY is blocked, reject the entire send.
pub fn evaluate_all_recipients(
    to: &[String],
    cc: &[String],
    bcc: &[String],
    rules: &[EmailRecipientRuleRow],
) -> Result<(), String> {
    for addr in to.iter().chain(cc.iter()).chain(bcc.iter()) {
        match evaluate_recipient(addr, rules) {
            RuleResult::Allowed => {}
            RuleResult::Blocked(reason) => return Err(reason),
        }
    }
    Ok(())
}

fn matches_email_rule(email_lower: &str, rule: &EmailRecipientRuleRow) -> bool {
    let pattern_lower = rule.pattern.to_lowercase();
    match rule.pattern_type.as_str() {
        "exact_email" => email_lower == pattern_lower,
        "domain" => {
            // Extract domain after @
            if let Some(domain) = email_lower.split('@').nth(1) {
                domain == pattern_lower
            } else {
                false
            }
        }
        "domain_suffix" => {
            // Boundary-safe suffix match
            if let Some(domain) = email_lower.split('@').nth(1) {
                domain == pattern_lower.trim_start_matches('.')
                    || domain.ends_with(&format!(".{}", pattern_lower.trim_start_matches('.')))
            } else {
                false
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_rule(rule_type: &str, pattern_type: &str, pattern: &str) -> EmailRecipientRuleRow {
        EmailRecipientRuleRow {
            id: Uuid::new_v4(),
            scope: "global".to_string(),
            project_id: None,
            rule_type: rule_type.to_string(),
            pattern_type: pattern_type.to_string(),
            pattern: pattern.to_string(),
            description: None,
            created_by: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_no_rules_allows_all() {
        let rules = vec![];
        assert!(matches!(
            evaluate_recipient("anyone@example.com", &rules),
            RuleResult::Allowed
        ));
    }

    #[test]
    fn test_block_exact_email() {
        let rules = vec![make_rule("block", "exact_email", "spam@bad.com")];
        assert!(matches!(
            evaluate_recipient("spam@bad.com", &rules),
            RuleResult::Blocked(_)
        ));
        assert!(matches!(
            evaluate_recipient("good@bad.com", &rules),
            RuleResult::Allowed
        ));
    }

    #[test]
    fn test_block_domain() {
        let rules = vec![make_rule("block", "domain", "evil.com")];
        assert!(matches!(
            evaluate_recipient("anyone@evil.com", &rules),
            RuleResult::Blocked(_)
        ));
        assert!(matches!(
            evaluate_recipient("anyone@notevil.com", &rules),
            RuleResult::Allowed
        ));
    }

    #[test]
    fn test_block_domain_suffix() {
        let rules = vec![make_rule("block", "domain_suffix", ".ru")];
        assert!(matches!(
            evaluate_recipient("user@evil.ru", &rules),
            RuleResult::Blocked(_)
        ));
        assert!(matches!(
            evaluate_recipient("user@sub.evil.ru", &rules),
            RuleResult::Blocked(_)
        ));
        assert!(matches!(
            evaluate_recipient("user@guru.com", &rules),
            RuleResult::Allowed
        ));
    }

    #[test]
    fn test_allowlist_mode() {
        let rules = vec![make_rule("allow", "domain", "example.com")];
        assert!(matches!(
            evaluate_recipient("alice@example.com", &rules),
            RuleResult::Allowed
        ));
        assert!(matches!(
            evaluate_recipient("bob@other.com", &rules),
            RuleResult::Blocked(_)
        ));
    }

    #[test]
    fn test_block_wins_over_allow() {
        let rules = vec![
            make_rule("allow", "domain", "example.com"),
            make_rule("block", "exact_email", "evil@example.com"),
        ];
        assert!(matches!(
            evaluate_recipient("evil@example.com", &rules),
            RuleResult::Blocked(_)
        ));
        assert!(matches!(
            evaluate_recipient("good@example.com", &rules),
            RuleResult::Allowed
        ));
    }

    #[test]
    fn test_case_insensitive() {
        let rules = vec![make_rule("block", "exact_email", "Spam@Bad.COM")];
        assert!(matches!(
            evaluate_recipient("spam@bad.com", &rules),
            RuleResult::Blocked(_)
        ));
    }

    #[test]
    fn test_evaluate_all_recipients_blocks_on_any() {
        let rules = vec![make_rule("block", "exact_email", "blocked@example.com")];
        assert!(evaluate_all_recipients(
            &["good@example.com".into()],
            &["blocked@example.com".into()],
            &[],
            &rules,
        )
        .is_err());
    }

    #[test]
    fn test_domain_suffix_boundary() {
        let rules = vec![make_rule("block", "domain_suffix", "example.com")];
        assert!(matches!(
            evaluate_recipient("user@example.com", &rules),
            RuleResult::Blocked(_)
        ));
        assert!(matches!(
            evaluate_recipient("user@sub.example.com", &rules),
            RuleResult::Blocked(_)
        ));
        // Must not match notexample.com
        assert!(matches!(
            evaluate_recipient("user@notexample.com", &rules),
            RuleResult::Allowed
        ));
    }
}
