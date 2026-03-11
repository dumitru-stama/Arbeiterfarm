use serde_json::Value;
use std::path::{Path, PathBuf};

/// Compute the Ghidra cache path for a binary.
///
/// Non-NDA projects share a single cache at `{cache_dir}/shared/{sha256}/`.
/// NDA projects get an isolated cache at `{cache_dir}/{project_id}/{sha256}/`.
pub fn ghidra_cache_path(cache_dir: &Path, project_id: &str, sha256: &str, is_nda: bool) -> PathBuf {
    if is_nda {
        cache_dir.join(project_id).join(sha256)
    } else {
        cache_dir.join("shared").join(sha256)
    }
}

/// Validate that a string is a valid hex address (0x followed by hex digits).
pub fn is_valid_hex_address(s: &str) -> bool {
    s.starts_with("0x") && s.len() > 2 && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Parse the last valid JSON line from rizin output.
/// Analysis commands (aa) may produce non-JSON text before the actual result.
/// Only considers lines starting with `{` or `[` as JSON candidates.
pub fn parse_last_json(stdout: &str) -> Value {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                return parsed;
            }
        }
    }
    // If no JSON found, return the raw text
    Value::String(stdout.to_string())
}

/// Apply renames overlay to decompiled JSON output.
///
/// Walks the decompiled structure and replaces old function names with new ones
/// in both the `name` fields and in the `code` strings. Uses word-boundary-aware
/// replacement to avoid false matches (e.g. renaming `FUN_001` won't affect `FUN_0010`).
/// Renames are applied longest-first for deterministic ordering.
pub fn apply_renames_overlay(decompiled: &mut Value, renames: &std::collections::HashMap<&str, &str>) {
    if renames.is_empty() {
        return;
    }

    // Sort renames by old name length descending so longer names match first.
    // This prevents `FUN_00401` from partially matching `FUN_004010`.
    let mut sorted_renames: Vec<(&&str, &&str)> = renames.iter().collect();
    sorted_renames.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    match decompiled {
        Value::Array(arr) => {
            for func in arr.iter_mut() {
                if let Some(name) = func.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()) {
                    if let Some(&new_name) = renames.get(name.as_str()) {
                        func["name"] = serde_json::json!(new_name);
                    }
                }
                if let Some(code) = func.get("code").and_then(|c| c.as_str()).map(|s| s.to_string()) {
                    let new_code = replace_all_with_word_boundary(&code, &sorted_renames);
                    if new_code != code {
                        func["code"] = serde_json::json!(new_code);
                    }
                }
            }
        }
        Value::Object(obj) => {
            let entries: Vec<(String, Value)> = obj.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            obj.clear();
            for (key, mut val) in entries {
                let new_key = renames.get(key.as_str()).map(|&s| s.to_string()).unwrap_or(key);
                if let Some(code) = val.get("code").and_then(|c| c.as_str()).map(|s| s.to_string()) {
                    let new_code = replace_all_with_word_boundary(&code, &sorted_renames);
                    if new_code != code {
                        val["code"] = serde_json::json!(new_code);
                    }
                }
                obj.insert(new_key, val);
            }
        }
        _ => {}
    }
}

/// Replace all occurrences of old→new in text, but only at word boundaries.
/// A word boundary means the character before/after the match is not alphanumeric or underscore.
fn replace_all_with_word_boundary(text: &str, sorted_renames: &[(&&str, &&str)]) -> String {
    let mut result = text.to_string();
    for (&old, &new) in sorted_renames {
        result = replace_word_boundary(&result, old, new);
    }
    result
}

/// Replace occurrences of `old` with `new` in `text`, only when `old` appears at word boundaries.
/// Word boundary = the character immediately before/after is not [a-zA-Z0-9_].
fn replace_word_boundary(text: &str, old: &str, new: &str) -> String {
    if old.is_empty() {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let old_bytes = old.as_bytes();
    let old_len = old_bytes.len();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i <= bytes.len().saturating_sub(old_len) {
        if &bytes[i..i + old_len] == old_bytes {
            let before_ok = i == 0 || !is_word_char(bytes[i - 1]);
            let after_ok = i + old_len >= bytes.len() || !is_word_char(bytes[i + old_len]);
            if before_ok && after_ok {
                result.push_str(new);
                i += old_len;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    // Append remaining bytes that couldn't start a match
    if i < bytes.len() {
        result.push_str(&text[i..]);
    }
    result
}

#[inline]
fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_valid_hex_address ---

    #[test]
    fn test_valid_hex_addresses() {
        assert!(is_valid_hex_address("0x00401000"));
        assert!(is_valid_hex_address("0xDEADBEEF"));
        assert!(is_valid_hex_address("0x0"));
        assert!(is_valid_hex_address("0xabcdef0123456789"));
    }

    #[test]
    fn test_invalid_hex_addresses() {
        assert!(!is_valid_hex_address("0x"));       // no digits after 0x
        assert!(!is_valid_hex_address("401000"));   // missing 0x prefix
        assert!(!is_valid_hex_address("0xGHIJ"));   // non-hex chars
        assert!(!is_valid_hex_address(""));          // empty
        assert!(!is_valid_hex_address("0x12;rm -rf /")); // injection attempt
    }

    // --- parse_last_json ---

    #[test]
    fn test_parse_last_json_simple() {
        let stdout = r#"{"key": "value"}"#;
        let result = parse_last_json(stdout);
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_parse_last_json_with_noise() {
        let stdout = "WARNING: unknown opcode\nAnalyzing...\n[{\"addr\":4096}]\n";
        let result = parse_last_json(stdout);
        assert!(result.is_array());
        assert_eq!(result[0]["addr"], 4096);
    }

    #[test]
    fn test_parse_last_json_picks_last() {
        let stdout = "[1,2,3]\n{\"final\": true}\n";
        let result = parse_last_json(stdout);
        assert_eq!(result["final"], true);
    }

    #[test]
    fn test_parse_last_json_no_json() {
        let stdout = "no json here\njust text\n";
        let result = parse_last_json(stdout);
        assert!(result.is_string());
    }

    #[test]
    fn test_parse_last_json_empty() {
        let result = parse_last_json("");
        assert!(result.is_string());
    }

    // --- replace_word_boundary ---

    #[test]
    fn test_replace_word_boundary_basic() {
        assert_eq!(
            replace_word_boundary("call FUN_001 here", "FUN_001", "parse_header"),
            "call parse_header here"
        );
    }

    #[test]
    fn test_replace_word_boundary_no_false_substring() {
        // FUN_001 should NOT match inside FUN_0010
        assert_eq!(
            replace_word_boundary("call FUN_0010 here", "FUN_001", "parse_header"),
            "call FUN_0010 here"
        );
    }

    #[test]
    fn test_replace_word_boundary_at_start_end() {
        assert_eq!(
            replace_word_boundary("FUN_001", "FUN_001", "parse"),
            "parse"
        );
        assert_eq!(
            replace_word_boundary("FUN_001 end", "FUN_001", "parse"),
            "parse end"
        );
        assert_eq!(
            replace_word_boundary("start FUN_001", "FUN_001", "parse"),
            "start parse"
        );
    }

    #[test]
    fn test_replace_word_boundary_multiple_occurrences() {
        assert_eq!(
            replace_word_boundary("FUN_001(FUN_001, FUN_001)", "FUN_001", "f"),
            "f(f, f)"
        );
    }

    #[test]
    fn test_replace_word_boundary_no_match_inside_identifier() {
        // Should NOT replace when the match is part of a larger identifier
        assert_eq!(
            replace_word_boundary("my_FUN_001_extra", "FUN_001", "parse"),
            "my_FUN_001_extra"
        );
    }

    #[test]
    fn test_replace_word_boundary_delimiters() {
        // Should replace when surrounded by non-word chars like (, ), *, ;, etc.
        assert_eq!(
            replace_word_boundary("(FUN_001)", "FUN_001", "parse"),
            "(parse)"
        );
        assert_eq!(
            replace_word_boundary("*FUN_001;", "FUN_001", "parse"),
            "*parse;"
        );
    }

    // --- apply_renames_overlay ---

    #[test]
    fn test_overlay_array_format() {
        use serde_json::json;
        let mut decompiled = json!([
            { "name": "FUN_001", "code": "void FUN_001() { FUN_002(); }" },
            { "name": "FUN_002", "code": "int FUN_002() { return 42; }" },
        ]);
        let mut renames = std::collections::HashMap::new();
        renames.insert("FUN_001", "parse_header");
        renames.insert("FUN_002", "get_value");
        apply_renames_overlay(&mut decompiled, &renames);

        let arr = decompiled.as_array().unwrap();
        assert_eq!(arr[0]["name"], "parse_header");
        assert_eq!(arr[1]["name"], "get_value");
        // Check code was updated too
        let code0 = arr[0]["code"].as_str().unwrap();
        assert!(code0.contains("parse_header"));
        assert!(code0.contains("get_value"));
        assert!(!code0.contains("FUN_001"));
        assert!(!code0.contains("FUN_002"));
    }

    #[test]
    fn test_overlay_object_format() {
        use serde_json::json;
        let mut decompiled = json!({
            "FUN_001": { "code": "void FUN_001() {}" },
        });
        let mut renames = std::collections::HashMap::new();
        renames.insert("FUN_001", "main");
        apply_renames_overlay(&mut decompiled, &renames);

        let obj = decompiled.as_object().unwrap();
        assert!(obj.contains_key("main"));
        assert!(!obj.contains_key("FUN_001"));
    }

    #[test]
    fn test_overlay_no_false_matches() {
        use serde_json::json;
        let mut decompiled = json!([
            { "name": "FUN_0010", "code": "void FUN_0010() { FUN_001(); }" },
        ]);
        let mut renames = std::collections::HashMap::new();
        renames.insert("FUN_001", "parse");
        apply_renames_overlay(&mut decompiled, &renames);

        let arr = decompiled.as_array().unwrap();
        // FUN_0010 should NOT be renamed (it's a different function)
        assert_eq!(arr[0]["name"], "FUN_0010");
        let code = arr[0]["code"].as_str().unwrap();
        // FUN_001 call should be renamed, but FUN_0010 should remain
        assert!(code.contains("FUN_0010"));
        assert!(code.contains("parse()"));
    }

    #[test]
    fn test_overlay_deterministic_ordering() {
        use serde_json::json;
        // Longer names should be replaced first to avoid partial matches
        let mut decompiled = json!([
            { "name": "FUN_00401000", "code": "FUN_00401000 calls FUN_0040100" },
        ]);
        let mut renames = std::collections::HashMap::new();
        renames.insert("FUN_00401000", "long_name");
        renames.insert("FUN_0040100", "short_name");
        apply_renames_overlay(&mut decompiled, &renames);

        let code = decompiled[0]["code"].as_str().unwrap();
        assert!(code.contains("long_name"));
        assert!(code.contains("short_name"));
    }

    #[test]
    fn test_overlay_empty_renames() {
        use serde_json::json;
        let mut decompiled = json!([{ "name": "FUN_001", "code": "test" }]);
        let renames = std::collections::HashMap::new();
        apply_renames_overlay(&mut decompiled, &renames);
        assert_eq!(decompiled[0]["name"], "FUN_001");
    }
}
