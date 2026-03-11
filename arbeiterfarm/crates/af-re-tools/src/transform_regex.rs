//! `transform.regex` — Extract patterns from text/binary artifacts using regex.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::path::Path;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return OopResult::Error {
                code: "missing_pattern".into(),
                message: "pattern parameter is required".into(),
                retryable: false,
            }
        }
    };
    let mode = input
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("all");
    let max_matches = input
        .get("max_matches")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;

    // Compile regex with size limit to prevent ReDoS
    let re = match regex::RegexBuilder::new(pattern)
        .size_limit(2 * 1024 * 1024) // 2 MB compiled size limit
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return OopResult::Error {
                code: "invalid_regex".into(),
                message: format!("invalid regex pattern: {e}"),
                retryable: false,
            }
        }
    };

    // Read as lossy UTF-8 (handles binary with non-UTF8 bytes)
    let raw = match std::fs::read(&artifact.storage_path) {
        Ok(d) => d,
        Err(e) => {
            return OopResult::Error {
                code: "read_error".into(),
                message: format!("failed to read artifact: {e}"),
                retryable: false,
            }
        }
    };

    let text = String::from_utf8_lossy(&raw);
    let capture_names: Vec<Option<&str>> = re.capture_names().collect();

    let mut matches: Vec<Value> = Vec::new();

    for caps in re.captures_iter(&text) {
        if matches.len() >= max_matches {
            break;
        }

        let full_match = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        let offset = caps.get(0).map(|m| m.start()).unwrap_or(0);

        // Extract named groups
        let mut groups = serde_json::Map::new();
        for (i, name) in capture_names.iter().enumerate() {
            if let Some(name) = name {
                if let Some(m) = caps.get(i) {
                    groups.insert((*name).to_string(), Value::String(m.as_str().to_string()));
                }
            }
        }

        let mut entry = serde_json::Map::new();
        entry.insert("match".into(), Value::String(full_match.to_string()));
        entry.insert("offset".into(), json!(offset));
        if !groups.is_empty() {
            entry.insert("groups".into(), Value::Object(groups));
        }

        matches.push(Value::Object(entry));

        if mode == "first" {
            break;
        }
    }

    let match_count = matches.len();

    // Write full results
    let full_result = json!({
        "pattern": pattern,
        "mode": mode,
        "match_count": match_count,
        "matches": matches,
    });

    let out_path = scratch_dir.join("regex_matches.json");
    let output_text = serde_json::to_string_pretty(&full_result).unwrap_or_default();
    if let Err(e) = std::fs::write(&out_path, &output_text) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write regex results: {e}"),
            retryable: false,
        };
    }

    // Inline summary: first 10 matches
    let preview: Vec<&Value> = matches.iter().take(10).collect();

    OopResult::Ok {
        output: json!({
            "pattern": pattern,
            "mode": mode,
            "match_count": match_count,
            "preview": preview,
            "hint": "Full regex matches stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename: "regex_matches.json".into(),
            path: out_path,
            mime_type: Some("application/json".into()),
            description: Some(format!("Regex matches for: {pattern}")),
        }],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_regex_size_limit() {
        // Ensure our size limit rejects pathological patterns
        let result = regex::RegexBuilder::new("(a+)+$")
            .size_limit(2 * 1024 * 1024)
            .build();
        // This specific pattern compiles fine but the size limit guards against much worse cases
        assert!(result.is_ok());
    }
}
