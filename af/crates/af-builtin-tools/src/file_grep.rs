use crate::envelope::{OopArtifact, OopResult, ProducedFile};
use regex::Regex;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Maximum number of lines to scan. Files beyond this are too large for grep.
const MAX_SCAN_LINES: usize = 2_000_000;

pub fn execute(artifact: &OopArtifact, input: &serde_json::Value, scratch_dir: &Path) -> OopResult {
    let path = &artifact.storage_path;

    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return OopResult::Error {
                code: "invalid_input".into(),
                message: "missing 'pattern' field".into(),
                retryable: false,
            };
        }
    };

    let context_lines = input
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(2) as usize;
    let max_matches = input
        .get("max_matches")
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;

    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            return OopResult::Error {
                code: "invalid_pattern".into(),
                message: format!("invalid regex: {e}"),
                retryable: false,
            };
        }
    };

    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to open file: {e}"),
                retryable: false,
            };
        }
    };

    // Streaming grep: read lines one at a time using a sliding window for context.
    //
    // Phase 1: Scan for match line numbers (single pass, one line in memory at a time).
    // Phase 2: For each match, read back the context window.
    //
    // Since context_lines is typically small (2-5) and we need both before and after
    // context, we use a ring buffer of size (2 * context_lines + 1) and record
    // match positions. Then we do a second read pass to extract just the context windows.

    let reader = BufReader::new(file);

    // First pass: find match line numbers (reading one line at a time)
    let mut match_line_numbers: Vec<usize> = Vec::new();
    let mut total_lines: usize = 0;
    let mut match_line_contents: Vec<String> = Vec::new();

    for line_result in reader.lines() {
        let line = line_result.unwrap_or_else(|_| "<non-utf8>".to_string());
        if re.is_match(&line) {
            match_line_numbers.push(total_lines);
            match_line_contents.push(line);
            if match_line_numbers.len() >= max_matches {
                // Count remaining lines without storing them
                total_lines += 1;
                break;
            }
        }
        total_lines += 1;
        if total_lines >= MAX_SCAN_LINES {
            return OopResult::Error {
                code: "file_too_large".into(),
                message: format!("file exceeds {MAX_SCAN_LINES} lines — use file.read_range instead"),
                retryable: false,
            };
        }
    }

    if match_line_numbers.is_empty() {
        return OopResult::Ok {
            output: json!({
                "filename": artifact.filename,
                "pattern": pattern,
                "total_lines": total_lines,
                "matches_found": 0,
                "truncated": false,
                "matches": [],
            }),
            produced_files: vec![],
        };
    }

    // Second pass: read context windows around each match.
    // Build a set of line ranges we need, then read only those lines.
    let mut needed_ranges: Vec<(usize, usize)> = Vec::new();
    for &line_num in &match_line_numbers {
        let start = line_num.saturating_sub(context_lines);
        let end = (line_num + context_lines + 1).min(total_lines);
        needed_ranges.push((start, end));
    }

    // Merge overlapping ranges for efficient reading
    needed_ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in &needed_ranges {
        if let Some(last) = merged.last_mut() {
            if *start <= last.1 {
                last.1 = last.1.max(*end);
                continue;
            }
        }
        merged.push((*start, *end));
    }

    // Read just the needed line ranges into a sparse map: line_num -> content
    let mut line_cache: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let file2 = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to reopen file: {e}"),
                retryable: false,
            };
        }
    };
    let reader2 = BufReader::new(file2);
    let mut current_line: usize = 0;
    let mut range_idx = 0;

    for line_result in reader2.lines() {
        if range_idx >= merged.len() {
            break;
        }
        let (_, range_end) = merged[range_idx];
        if current_line >= range_end {
            range_idx += 1;
            if range_idx >= merged.len() {
                break;
            }
        }
        let (rs, re) = merged[range_idx];
        if current_line >= rs && current_line < re {
            let line = line_result.unwrap_or_else(|_| "<non-utf8>".to_string());
            line_cache.insert(current_line, line);
        }
        current_line += 1;
    }

    // Build match results with context
    let mut matches = Vec::new();
    for (idx, &line_num) in match_line_numbers.iter().enumerate() {
        let start = line_num.saturating_sub(context_lines);
        let end = (line_num + context_lines + 1).min(total_lines);

        let context: Vec<serde_json::Value> = (start..end)
            .map(|j| {
                let content = line_cache
                    .get(&j)
                    .map(|s| s.as_str())
                    .unwrap_or("<unavailable>");
                json!({
                    "line_number": j + 1,
                    "content": content,
                    "is_match": j == line_num,
                })
            })
            .collect();

        matches.push(json!({
            "line_number": line_num + 1,
            "line": &match_line_contents[idx],
            "context": context,
        }));
    }

    // Store full results as artifact, return compact summary
    let matches_found = matches.len();
    let truncated = matches_found >= max_matches;
    let top_matches: Vec<serde_json::Value> = matches.iter().take(5).cloned().collect();

    let full_result = json!({
        "filename": artifact.filename,
        "pattern": pattern,
        "total_lines": total_lines,
        "matches_found": matches_found,
        "truncated": truncated,
        "matches": matches,
    });

    let grep_path = scratch_dir.join("grep_results.json");
    let mut produced_files = Vec::new();
    match std::fs::write(&grep_path, serde_json::to_vec_pretty(&full_result).unwrap_or_default()) {
        Ok(()) => {
            produced_files.push(ProducedFile {
                filename: "grep_results.json".into(),
                path: PathBuf::from("grep_results.json"),
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "Grep results for '{}': {} matches in {}",
                    pattern, matches_found, artifact.filename
                )),
            });
        }
        Err(e) => {
            eprintln!("[file.grep] WARNING: failed to write grep_results.json: {e}");
        }
    }

    OopResult::Ok {
        output: json!({
            "filename": artifact.filename,
            "pattern": pattern,
            "total_lines": total_lines,
            "matches_found": matches_found,
            "truncated": truncated,
            "top_matches": top_matches,
            "hint": "Full grep results stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files,
    }
}
