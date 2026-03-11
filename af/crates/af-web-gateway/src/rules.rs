use af_db::web_fetch::WebFetchRuleRow;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use url::Url;

/// Lazily-initialized cache for compiled regexes used by `url_regex` rules.
/// Capped at 1000 entries to prevent unbounded growth.
static REGEX_CACHE: LazyLock<Mutex<HashMap<String, regex::Regex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Result of URL rule evaluation.
#[derive(Debug)]
pub enum RuleResult {
    Allowed,
    Blocked(Option<String>),
}

/// Evaluate URL rules against a parsed URL and its resolved IPs.
///
/// - Block rules are evaluated first; if any match → blocked.
/// - If any allow rules exist → allowlist mode (require at least one match).
/// - If no allow rules exist → allowed by default (blocklist-only mode).
pub fn evaluate_rules(
    url: &Url,
    resolved_ips: &[IpAddr],
    rules: &[WebFetchRuleRow],
) -> RuleResult {
    let (blocks, allows): (Vec<_>, Vec<_>) = rules
        .iter()
        .partition(|r| r.rule_type == "block");

    // Block rules always win
    for rule in &blocks {
        if matches_rule(url, resolved_ips, rule) {
            return RuleResult::Blocked(rule.description.clone());
        }
    }

    // If no allow rules → allowed by default
    if allows.is_empty() {
        return RuleResult::Allowed;
    }

    // Allowlist mode: require at least one match
    for rule in &allows {
        if matches_rule(url, resolved_ips, rule) {
            return RuleResult::Allowed;
        }
    }

    RuleResult::Blocked(Some("no matching allow rule".into()))
}

fn matches_rule(url: &Url, resolved_ips: &[IpAddr], rule: &WebFetchRuleRow) -> bool {
    match rule.pattern_type.as_str() {
        "domain" => {
            if let Some(host) = url.host_str() {
                host.eq_ignore_ascii_case(&rule.pattern)
            } else {
                false
            }
        }
        "domain_suffix" => {
            if let Some(host) = url.host_str() {
                let lower = host.to_lowercase();
                let pattern = rule.pattern.to_lowercase();
                // Ensure domain boundary: exact match or preceded by a dot
                let pat = pattern.trim_start_matches('.');
                lower == pat || lower.ends_with(&format!(".{}", pat))
            } else {
                false
            }
        }
        "url_prefix" => {
            url.as_str().starts_with(&rule.pattern)
        }
        "url_regex" => {
            let cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(re) = cache.get(&rule.pattern) {
                return re.is_match(url.as_str());
            }
            drop(cache);
            match regex::Regex::new(&rule.pattern) {
                Ok(re) => {
                    let matched = re.is_match(url.as_str());
                    let mut cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                    if cache.len() < 1000 {
                        cache.insert(rule.pattern.clone(), re);
                    }
                    matched
                }
                Err(_) => false, // invalid regex = no match
            }
        }
        "ip_cidr" => {
            match parse_cidr(&rule.pattern) {
                Some((net, prefix_len)) => {
                    resolved_ips.iter().any(|ip| ip_in_cidr(ip, &net, prefix_len))
                }
                None => false,
            }
        }
        _ => false,
    }
}

fn parse_cidr(cidr: &str) -> Option<(IpAddr, u8)> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let ip: IpAddr = parts[0].parse().ok()?;
    let prefix: u8 = parts[1].parse().ok()?;
    Some((ip, prefix))
}

fn ip_in_cidr(ip: &IpAddr, network: &IpAddr, prefix_len: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            if prefix_len > 32 {
                return false;
            }
            let mask = if prefix_len == 0 {
                0u32
            } else {
                !0u32 << (32 - prefix_len)
            };
            (u32::from(*ip) & mask) == (u32::from(*net) & mask)
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            if prefix_len > 128 {
                return false;
            }
            let ip_bits = u128::from(*ip);
            let net_bits = u128::from(*net);
            let mask = if prefix_len == 0 {
                0u128
            } else {
                !0u128 << (128 - prefix_len)
            };
            (ip_bits & mask) == (net_bits & mask)
        }
        _ => false, // v4 vs v6 mismatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_in_cidr() {
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        let net: IpAddr = "10.0.0.0".parse().unwrap();
        assert!(ip_in_cidr(&ip, &net, 8));
        assert!(!ip_in_cidr(&"11.0.0.1".parse().unwrap(), &net, 8));
    }

    #[test]
    fn test_domain_suffix() {
        let url = Url::parse("https://evil.ru/path").unwrap();
        let rule = WebFetchRuleRow {
            id: uuid::Uuid::nil(),
            scope: "global".into(),
            project_id: None,
            rule_type: "block".into(),
            pattern_type: "domain_suffix".into(),
            pattern: ".ru".into(),
            description: Some("Block Russian domains".into()),
            created_by: None,
            created_at: chrono::Utc::now(),
        };
        assert!(matches_rule(&url, &[], &rule));
    }

    #[test]
    fn test_domain_suffix_boundary() {
        let rule = WebFetchRuleRow {
            id: uuid::Uuid::nil(),
            scope: "global".into(),
            project_id: None,
            rule_type: "block".into(),
            pattern_type: "domain_suffix".into(),
            pattern: ".example.com".into(),
            description: None,
            created_by: None,
            created_at: chrono::Utc::now(),
        };

        // Should match: exact domain and subdomain
        let url_exact = Url::parse("https://example.com/path").unwrap();
        assert!(matches_rule(&url_exact, &[], &rule));

        let url_sub = Url::parse("https://sub.example.com/path").unwrap();
        assert!(matches_rule(&url_sub, &[], &rule));

        // Should NOT match: suffix appears but not at domain boundary
        let url_not = Url::parse("https://notexample.com/path").unwrap();
        assert!(!matches_rule(&url_not, &[], &rule));
    }
}
