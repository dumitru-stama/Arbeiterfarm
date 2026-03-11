use crate::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub fn execute(artifact: &OopArtifact, input: &serde_json::Value, scratch_dir: &Path) -> OopResult {
    let path = &artifact.storage_path;
    let min_length = input
        .get("min_length")
        .and_then(|v| v.as_u64())
        .unwrap_or(4) as usize;
    let max_strings = input
        .get("max_strings")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;

    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to read file: {e}"),
                retryable: false,
            };
        }
    };

    let mut strings = Vec::new();
    let mut current = Vec::new();
    let mut current_offset = 0usize;

    for (i, &byte) in data.iter().enumerate() {
        if byte >= 0x20 && byte < 0x7f {
            if current.is_empty() {
                current_offset = i;
            }
            current.push(byte);
        } else {
            if current.len() >= min_length {
                strings.push(json!({
                    "offset": current_offset,
                    "string": String::from_utf8_lossy(&current).to_string(),
                }));
                if strings.len() >= max_strings {
                    break;
                }
            }
            current.clear();
        }
    }

    // Don't forget the last string
    if current.len() >= min_length && strings.len() < max_strings {
        strings.push(json!({
            "offset": current_offset,
            "string": String::from_utf8_lossy(&current).to_string(),
        }));
    }

    // Store full results as artifact, return compact summary
    let strings_found = strings.len();
    let truncated = strings_found >= max_strings;
    let top_strings: Vec<serde_json::Value> = strings.iter().take(20).cloned().collect();

    let full_result = json!({
        "filename": artifact.filename,
        "file_size": data.len(),
        "min_length": min_length,
        "strings_found": strings_found,
        "truncated": truncated,
        "strings": strings,
    });

    let strings_path = scratch_dir.join("strings.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&strings_path, serde_json::to_vec_pretty(&full_result).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "strings.json".into(),
                path: PathBuf::from("strings.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "Extracted strings from {}: {} strings (min_length={})",
                    artifact.filename, strings_found, min_length
                )),
            });
        }
        Err(e) => {
            eprintln!("[file.strings] WARNING: failed to write strings.json: {e}");
        }
    }

    OopResult::Ok {
        output: json!({
            "filename": artifact.filename,
            "file_size": data.len(),
            "total_strings": strings_found,
            "truncated": truncated,
            "top_strings": top_strings,
            "hint": "Full string list stored as artifact. Use file.grep to search.",
        }),
        produced_files,
    }
}
