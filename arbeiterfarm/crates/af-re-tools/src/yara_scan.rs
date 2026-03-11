//! YARA scan output parser and summary builder.
//! Used by the OOP executor (re_executor.rs) to parse `yara -s` output.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Parse `yara -s` stdout into structured JSON.
///
/// Format:
/// ```text
/// rule_name [tag1,tag2] /path/to/file
/// 0xOFFSET:$string_id: matched data
/// 0xOFFSET:$string_id: matched data
/// rule_name2 /path/to/file
/// ```
pub fn parse_yara_output(stdout: &str) -> Value {
    let mut rules: Vec<Value> = Vec::new();
    let mut current_rule: Option<(String, Vec<String>, Vec<Value>)> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // String match line: starts with 0x
        if line.starts_with("0x") {
            if let Some((_, _, ref mut matches)) = current_rule {
                if let Some(m) = parse_match_line(line) {
                    matches.push(m);
                }
            }
            continue;
        }

        // Rule match line: "rule_name [tags] filepath" or "rule_name filepath"
        // Flush previous rule
        if let Some((name, tags, matches)) = current_rule.take() {
            rules.push(json!({
                "rule": name,
                "tags": tags,
                "matches": matches,
                "match_count": matches.len(),
            }));
        }

        // Parse new rule line
        let (name, tags) = parse_rule_line(line);
        if !name.is_empty() {
            current_rule = Some((name, tags, Vec::new()));
        }
    }

    // Flush last rule
    if let Some((name, tags, matches)) = current_rule {
        let count = matches.len();
        rules.push(json!({
            "rule": name,
            "tags": tags,
            "matches": matches,
            "match_count": count,
        }));
    }

    json!({
        "total_rules_matched": rules.len(),
        "rules": rules,
    })
}

/// Parse a rule line like "rule_name [tag1,tag2] /path" or "rule_name /path"
fn parse_rule_line(line: &str) -> (String, Vec<String>) {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let name = parts.first().unwrap_or(&"").to_string();

    let rest = parts.get(1).unwrap_or(&"");
    let tags = if rest.starts_with('[') {
        if let Some(end) = rest.find(']') {
            rest[1..end]
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    (name, tags)
}

/// Parse a match line like "0x1234:$s1: matched data"
fn parse_match_line(line: &str) -> Option<Value> {
    // Format: 0xOFFSET:$string_id: data
    let colon1 = line.find(':')?;
    let offset = &line[..colon1];

    let rest = &line[colon1 + 1..];
    let colon2 = rest.find(':')?;
    let string_id = rest[..colon2].trim();
    let data = rest[colon2 + 1..].trim();

    Some(json!({
        "offset": offset,
        "string_id": string_id,
        "data": data,
    }))
}

/// Build a compact inline summary from full YARA scan results.
pub fn build_scan_summary(full: &Value) -> Value {
    let total = full["total_rules_matched"].as_u64().unwrap_or(0);

    let rule_names: Vec<&str> = full["rules"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r["rule"].as_str())
                .collect()
        })
        .unwrap_or_default();

    let total_matches: usize = full["rules"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r["match_count"].as_u64())
                .sum::<u64>() as usize
        })
        .unwrap_or(0);

    // Top 10 individual string matches across all rules
    let top_matches: Vec<Value> = full["rules"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .flat_map(|r| {
                    let rule = r["rule"].as_str().unwrap_or("?");
                    r["matches"]
                        .as_array()
                        .map(|m| {
                            m.iter()
                                .map(|entry| {
                                    json!({
                                        "rule": rule,
                                        "offset": entry["offset"],
                                        "string_id": entry["string_id"],
                                        "data": entry["data"],
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .take(10)
                .collect()
        })
        .unwrap_or_default();

    json!({
        "rules_matched": total,
        "rule_names": rule_names,
        "total_string_matches": total_matches,
        "top_matches": top_matches,
        "hint": "Full scan results stored as artifact. Use file.read_range or file.grep to inspect.",
    })
}

/// Sanitize a rule name: no path traversal, valid chars only.
pub fn sanitize_rule_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("rule name cannot be empty".into());
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(format!("invalid rule name: contains forbidden characters: {name}"));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(format!(
            "invalid rule name: only alphanumeric, underscore, hyphen, dot allowed: {name}"
        ));
    }
    Ok(name.to_string())
}

/// Collect .yar/.yara files from a directory.
pub fn collect_rule_files(dir: Option<&Path>) -> Vec<PathBuf> {
    let dir = match dir {
        Some(d) if d.is_dir() => d,
        _ => return vec![],
    };

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "yar" || ext == "yara" {
                        files.push(path);
                    }
                }
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_yara_output_with_matches() {
        let stdout = "\
detect_elf [malware,elf] /tmp/sample.bin\n\
0x0000:$magic: 7f 45 4c 46\n\
0x0100:$entry: 48 89 e5\n\
detect_packed /tmp/sample.bin\n\
0x0200:$upx: UPX!\n";

        let result = parse_yara_output(stdout);
        assert_eq!(result["total_rules_matched"], 2);
        let rules = result["rules"].as_array().unwrap();
        assert_eq!(rules[0]["rule"], "detect_elf");
        assert_eq!(rules[0]["tags"].as_array().unwrap().len(), 2);
        assert_eq!(rules[0]["match_count"], 2);
        assert_eq!(rules[1]["rule"], "detect_packed");
        assert_eq!(rules[1]["match_count"], 1);
    }

    #[test]
    fn test_parse_yara_output_no_matches() {
        let result = parse_yara_output("");
        assert_eq!(result["total_rules_matched"], 0);
        assert!(result["rules"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_parse_yara_output_with_offsets() {
        let stdout = "my_rule /tmp/test\n0x00001234:$hex_str: de ad be ef\n";
        let result = parse_yara_output(stdout);
        let rules = result["rules"].as_array().unwrap();
        let matches = rules[0]["matches"].as_array().unwrap();
        assert_eq!(matches[0]["offset"], "0x00001234");
        assert_eq!(matches[0]["string_id"], "$hex_str");
        assert_eq!(matches[0]["data"], "de ad be ef");
    }

    #[test]
    fn test_parse_yara_output_malformed() {
        // Garbled output — no valid rule lines
        let result = parse_yara_output("some random garbage\n\nnonsense");
        // "some" gets parsed as a rule name (first word) — this is acceptable
        // The important thing is it doesn't panic
        assert!(result["rules"].as_array().is_some());
    }

    #[test]
    fn test_build_scan_summary_many_matches() {
        let full = json!({
            "total_rules_matched": 2,
            "rules": [
                {
                    "rule": "rule1",
                    "tags": [],
                    "match_count": 15,
                    "matches": (0..15).map(|i| json!({"offset": format!("0x{:04x}", i), "string_id": "$s1", "data": "xx"})).collect::<Vec<_>>(),
                },
                {
                    "rule": "rule2",
                    "tags": [],
                    "match_count": 3,
                    "matches": (0..3).map(|i| json!({"offset": format!("0x{:04x}", i+100), "string_id": "$s2", "data": "yy"})).collect::<Vec<_>>(),
                },
            ],
        });
        let summary = build_scan_summary(&full);
        assert_eq!(summary["rules_matched"], 2);
        assert_eq!(summary["total_string_matches"], 18);
        // Top 10 truncation
        assert!(summary["top_matches"].as_array().unwrap().len() <= 10);
    }

    #[test]
    fn test_build_scan_summary_no_matches() {
        let full = json!({ "total_rules_matched": 0, "rules": [] });
        let summary = build_scan_summary(&full);
        assert_eq!(summary["rules_matched"], 0);
        assert!(summary["rule_names"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_sanitize_rule_name_valid() {
        assert!(sanitize_rule_name("detect_emotet").is_ok());
        assert!(sanitize_rule_name("rule-v2.3").is_ok());
        assert!(sanitize_rule_name("YARA_Rule_01").is_ok());
    }

    #[test]
    fn test_sanitize_rule_name_path_traversal() {
        assert!(sanitize_rule_name("../etc/passwd").is_err());
        assert!(sanitize_rule_name("..\\windows").is_err());
    }

    #[test]
    fn test_sanitize_rule_name_empty() {
        assert!(sanitize_rule_name("").is_err());
        assert!(sanitize_rule_name("   ").is_err());
    }

    #[test]
    fn test_sanitize_rule_name_special_chars() {
        assert!(sanitize_rule_name("rule name").is_err());
        assert!(sanitize_rule_name("rule/name").is_err());
        assert!(sanitize_rule_name("rule\0name").is_err());
    }
}
