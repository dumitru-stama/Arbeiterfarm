//! `transform.csv` — Parse, filter, or summarize CSV data.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let operation = match input.get("operation").and_then(|v| v.as_str()) {
        Some(o) => o,
        None => {
            return OopResult::Error {
                code: "missing_operation".into(),
                message: "operation parameter is required".into(),
                retryable: false,
            }
        }
    };

    let delimiter = input
        .get("delimiter")
        .and_then(|v| v.as_str())
        .and_then(|s| s.bytes().next())
        .unwrap_or(b',');
    let has_header = input
        .get("has_header")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let content = match std::fs::read(&artifact.storage_path) {
        Ok(c) => c,
        Err(e) => {
            return OopResult::Error {
                code: "read_error".into(),
                message: format!("failed to read artifact: {e}"),
                retryable: false,
            }
        }
    };

    match operation {
        "parse" => execute_parse(&content, delimiter, has_header, input, scratch_dir),
        "filter" => execute_filter(&content, delimiter, has_header, input, scratch_dir),
        "stats" => execute_stats(&content, delimiter, has_header),
        other => OopResult::Error {
            code: "invalid_operation".into(),
            message: format!("unsupported operation: {other}. Use 'parse', 'filter', or 'stats'"),
            retryable: false,
        },
    }
}

fn resolve_column_index(col: &str, headers: &[String]) -> Option<usize> {
    // Try numeric index first
    if let Ok(idx) = col.parse::<usize>() {
        return Some(idx);
    }
    // Try header name
    headers.iter().position(|h| h == col)
}

fn select_columns(record: &csv::StringRecord, headers: &[String], columns: &[String]) -> Vec<(String, String)> {
    if columns.is_empty() {
        return headers
            .iter()
            .enumerate()
            .map(|(i, h)| (h.clone(), record.get(i).unwrap_or("").to_string()))
            .collect();
    }
    columns
        .iter()
        .filter_map(|col| {
            let idx = resolve_column_index(col, headers)?;
            let name = headers.get(idx).cloned().unwrap_or_else(|| format!("col_{idx}"));
            let val = record.get(idx).unwrap_or("").to_string();
            Some((name, val))
        })
        .collect()
}

fn execute_parse(
    content: &[u8],
    delimiter: u8,
    has_header: bool,
    input: &Value,
    scratch_dir: &Path,
) -> OopResult {
    let columns: Vec<String> = input
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .from_reader(content);

    let headers: Vec<String> = if has_header {
        rdr.headers()
            .map(|h| h.iter().map(String::from).collect())
            .unwrap_or_default()
    } else {
        // Generate col_0, col_1, ...
        if let Some(first) = rdr.records().next() {
            match first {
                Ok(r) => (0..r.len()).map(|i| format!("col_{i}")).collect(),
                Err(_) => vec![],
            }
        } else {
            vec![]
        }
    };

    // Re-read from beginning
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .from_reader(content);

    let mut rows: Vec<Value> = Vec::new();
    for result in rdr.records() {
        let record = match result {
            Ok(r) => r,
            Err(_) => continue,
        };
        let selected = select_columns(&record, &headers, &columns);
        let obj: serde_json::Map<String, Value> = selected
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        rows.push(Value::Object(obj));
    }

    let row_count = rows.len();
    let output_text = serde_json::to_string_pretty(&rows).unwrap_or_default();
    let out_path = scratch_dir.join("csv_parsed.json");

    if let Err(e) = std::fs::write(&out_path, &output_text) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write parsed output: {e}"),
            retryable: false,
        };
    }

    // Preview: first 10 rows
    let preview: Vec<&Value> = rows.iter().take(10).collect();

    OopResult::Ok {
        output: json!({
            "operation": "parse",
            "row_count": row_count,
            "column_count": headers.len(),
            "columns": headers,
            "output_size": output_text.len(),
            "preview": preview,
            "hint": "Full parsed JSON stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename: "csv_parsed.json".into(),
            path: out_path,
            mime_type: Some("application/json".into()),
            description: Some(format!("CSV parsed to JSON: {row_count} rows")),
        }],
    }
}

fn execute_filter(
    content: &[u8],
    delimiter: u8,
    has_header: bool,
    input: &Value,
    scratch_dir: &Path,
) -> OopResult {
    let filter_column = match input.get("filter_column").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return OopResult::Error {
                code: "missing_filter_column".into(),
                message: "filter_column is required for filter operation".into(),
                retryable: false,
            }
        }
    };
    let filter_pattern = match input.get("filter_pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return OopResult::Error {
                code: "missing_filter_pattern".into(),
                message: "filter_pattern is required for filter operation".into(),
                retryable: false,
            }
        }
    };

    let re = match regex::RegexBuilder::new(filter_pattern)
        .size_limit(1024 * 1024)
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return OopResult::Error {
                code: "invalid_regex".into(),
                message: format!("invalid filter_pattern regex: {e}"),
                retryable: false,
            }
        }
    };

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .from_reader(content);

    let headers: Vec<String> = if has_header {
        rdr.headers()
            .map(|h| h.iter().map(String::from).collect())
            .unwrap_or_default()
    } else {
        vec![]
    };

    let col_idx = resolve_column_index(filter_column, &headers);

    let mut wtr = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());

    // Write header if present
    if has_header && !headers.is_empty() {
        let _ = wtr.write_record(&headers);
    }

    let mut matched_count = 0usize;
    let mut total_count = 0usize;

    for result in rdr.records() {
        total_count += 1;
        let record = match result {
            Ok(r) => r,
            Err(_) => continue,
        };

        let should_include = match col_idx {
            Some(idx) => {
                let val = record.get(idx).unwrap_or("");
                re.is_match(val)
            }
            None => {
                // Search all columns
                record.iter().any(|val| re.is_match(val))
            }
        };

        if should_include {
            let _ = wtr.write_record(&record);
            matched_count += 1;
        }
    }

    let output_bytes = wtr.into_inner().unwrap_or_default();
    let out_path = scratch_dir.join("csv_filtered.csv");

    if let Err(e) = std::fs::write(&out_path, &output_bytes) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write filtered output: {e}"),
            retryable: false,
        };
    }

    OopResult::Ok {
        output: json!({
            "operation": "filter",
            "total_rows": total_count,
            "matched_rows": matched_count,
            "filter_column": filter_column,
            "filter_pattern": filter_pattern,
            "output_size": output_bytes.len(),
            "hint": "Filtered CSV stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename: "csv_filtered.csv".into(),
            path: out_path,
            mime_type: Some("text/csv".into()),
            description: Some(format!(
                "CSV filtered: {matched_count}/{total_count} rows matching '{filter_pattern}'"
            )),
        }],
    }
}

fn execute_stats(content: &[u8], delimiter: u8, has_header: bool) -> OopResult {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .from_reader(content);

    let headers: Vec<String> = if has_header {
        rdr.headers()
            .map(|h| h.iter().map(String::from).collect())
            .unwrap_or_default()
    } else {
        vec![]
    };

    let max_tracked_cols = 10;
    let max_unique_tracked = 1000;

    struct ColStats {
        count: usize,
        non_null: usize,
        min_len: usize,
        max_len: usize,
        uniques: HashMap<String, usize>,
        unique_overflow: bool,
    }

    let mut stats: Vec<ColStats> = Vec::new();
    let mut row_count = 0usize;

    for result in rdr.records() {
        let record = match result {
            Ok(r) => r,
            Err(_) => continue,
        };
        row_count += 1;

        // Initialize stats columns on first record
        if stats.is_empty() {
            let ncols = record.len().min(max_tracked_cols);
            for _ in 0..ncols {
                stats.push(ColStats {
                    count: 0,
                    non_null: 0,
                    min_len: usize::MAX,
                    max_len: 0,
                    uniques: HashMap::new(),
                    unique_overflow: false,
                });
            }
        }

        for (i, stat) in stats.iter_mut().enumerate() {
            let val = record.get(i).unwrap_or("");
            stat.count += 1;
            if !val.is_empty() {
                stat.non_null += 1;
                stat.min_len = stat.min_len.min(val.len());
                stat.max_len = stat.max_len.max(val.len());
                if !stat.unique_overflow {
                    if stat.uniques.len() < max_unique_tracked {
                        *stat.uniques.entry(val.to_string()).or_insert(0) += 1;
                    } else {
                        stat.unique_overflow = true;
                    }
                }
            }
        }
    }

    let col_stats: Vec<Value> = stats
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let name = headers.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));
            json!({
                "column": name,
                "count": s.count,
                "non_null": s.non_null,
                "unique_count": if s.unique_overflow { format!(">{max_unique_tracked}") } else { s.uniques.len().to_string() },
                "min_length": if s.non_null > 0 { s.min_len } else { 0 },
                "max_length": s.max_len,
            })
        })
        .collect();

    OopResult::Ok {
        output: json!({
            "operation": "stats",
            "row_count": row_count,
            "column_count": headers.len().max(stats.len()),
            "columns": col_stats,
        }),
        produced_files: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_column_index_by_name() {
        let headers = vec!["name".into(), "age".into(), "city".into()];
        assert_eq!(resolve_column_index("age", &headers), Some(1));
        assert_eq!(resolve_column_index("missing", &headers), None);
    }

    #[test]
    fn test_resolve_column_index_by_number() {
        let headers = vec!["name".into(), "age".into()];
        assert_eq!(resolve_column_index("0", &headers), Some(0));
        assert_eq!(resolve_column_index("1", &headers), Some(1));
    }
}
