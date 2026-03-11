//! YARA rule name extraction and validation utilities.
//! Used by the OOP executor for `yara.generate`.

/// Extract the first rule name from YARA source.
///
/// Handles: `rule foo {`, `private rule foo {`, `rule foo : tag1 tag2 {`
pub fn extract_rule_name(source: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim();
        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") {
            continue;
        }

        // Match "rule <name>" or "private rule <name>" or "global rule <name>"
        let after_rule = if let Some(rest) = trimmed.strip_prefix("private") {
            rest.trim_start().strip_prefix("rule")?
        } else if let Some(rest) = trimmed.strip_prefix("global") {
            rest.trim_start().strip_prefix("rule")?
        } else {
            trimmed.strip_prefix("rule")?
        };

        // Must have whitespace after "rule"
        let after_rule = after_rule.trim_start();
        if after_rule.is_empty() {
            continue;
        }

        // Extract the name (up to whitespace, colon, or brace)
        let name: String = after_rule
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();

        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// Basic structural validation of YARA rule source.
///
/// Checks that the source contains `rule` keyword and `condition` section.
/// Not a full parser — YARA binary does the real validation.
pub fn validate_rule_text(source: &str) -> Result<(), String> {
    let lower = source.to_lowercase();

    let has_rule = lower.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("rule ")
            || trimmed.starts_with("private rule ")
            || trimmed.starts_with("global rule ")
    });
    if !has_rule {
        return Err("YARA source must contain a 'rule' declaration".into());
    }

    if !lower.contains("condition") {
        return Err("YARA rule must contain a 'condition' section".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rule_name_simple() {
        let src = "rule detect_emotet {\n  condition:\n    true\n}";
        assert_eq!(extract_rule_name(src), Some("detect_emotet".into()));
    }

    #[test]
    fn test_extract_rule_name_with_tags() {
        let src = "rule foo : tag1 tag2 {\n  condition:\n    true\n}";
        assert_eq!(extract_rule_name(src), Some("foo".into()));
    }

    #[test]
    fn test_extract_rule_name_private() {
        let src = "private rule internal_check {\n  condition:\n    true\n}";
        assert_eq!(extract_rule_name(src), Some("internal_check".into()));
    }

    #[test]
    fn test_extract_rule_name_none() {
        let src = "no rule here, just text";
        assert_eq!(extract_rule_name(src), None);
    }

    #[test]
    fn test_validate_rule_text_valid() {
        let src = "rule detect_elf {\n  strings:\n    $magic = { 7f 45 4c 46 }\n  condition:\n    $magic at 0\n}";
        assert!(validate_rule_text(src).is_ok());
    }

    #[test]
    fn test_validate_rule_text_no_condition() {
        let src = "rule broken {\n  strings:\n    $s = \"hello\"\n}";
        assert!(validate_rule_text(src).is_err());
        assert!(validate_rule_text(src)
            .unwrap_err()
            .contains("condition"));
    }
}
