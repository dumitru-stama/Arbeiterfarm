//! `doc.ingest` — All-in-one: parse document → chunk → prepare for embedding.
//!
//! Produces two artifacts: `parsed_text.txt` and `chunks.json`.
//! The agent then calls `embed.batch` with the chunk data to store embeddings.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::path::Path;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let data = match std::fs::read(&artifact.storage_path) {
        Ok(d) => d,
        Err(e) => {
            return OopResult::Error {
                code: "read_error".into(),
                message: format!("failed to read artifact: {e}"),
                retryable: false,
            }
        }
    };

    let format_override = input.get("format").and_then(|v| v.as_str());
    let pages = input.get("pages").and_then(|v| v.as_str());

    // Step 1: Parse — extract text
    let detected = format_override
        .map(|f| f.to_string())
        .unwrap_or_else(|| crate::doc_parse::detect_format(&data, &artifact.filename));
    let (text, format) = if detected == "xlsx" {
        match crate::doc_parse::execute_inner_xlsx(&artifact.storage_path) {
            Ok(r) => r,
            Err(e) => {
                return OopResult::Error {
                    code: "parse_error".into(),
                    message: e,
                    retryable: false,
                }
            }
        }
    } else {
        match crate::doc_parse::execute_inner(&data, &artifact.filename, format_override, pages) {
            Ok(r) => r,
            Err(e) => {
                return OopResult::Error {
                    code: "parse_error".into(),
                    message: e,
                    retryable: false,
                }
            }
        }
    };

    let char_count = text.chars().count();
    let word_count = text.split_whitespace().count();

    // Write parsed text
    let text_path = scratch_dir.join("parsed_text.txt");
    if let Err(e) = std::fs::write(&text_path, &text) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write parsed text: {e}"),
            retryable: false,
        };
    }

    // Step 2: Chunk
    let chunk_size = input
        .get("chunk_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000)
        .max(100)
        .min(10000) as usize;

    let max_overlap = chunk_size / 2;
    let chunk_overlap = input
        .get("chunk_overlap")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
        .min(max_overlap as u64) as usize;

    let default_prefix = Path::new(&artifact.filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("chunk")
        .to_string();
    let label_prefix = input
        .get("label_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or(&default_prefix);

    let chunks = crate::doc_chunk::chunk_text(&text, chunk_size, chunk_overlap, label_prefix);
    let chunk_count = chunks.len();
    let total_chars: usize = chunks.iter().map(|c| c.length).sum();
    let avg_size = if chunk_count > 0 {
        total_chars / chunk_count
    } else {
        0
    };

    let chunks_json =
        serde_json::to_string_pretty(&chunks).unwrap_or_else(|_| "[]".to_string());

    let chunks_path = scratch_dir.join("chunks.json");
    if let Err(e) = std::fs::write(&chunks_path, &chunks_json) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write chunks: {e}"),
            retryable: false,
        };
    }

    // Build labels list for summary (first 20)
    let labels: Vec<&str> = chunks.iter().take(20).map(|c| c.label.as_str()).collect();

    OopResult::Ok {
        output: json!({
            "format": format,
            "char_count": char_count,
            "word_count": word_count,
            "chunk_count": chunk_count,
            "avg_chunk_size": avg_size,
            "label_prefix": label_prefix,
            "labels_preview": labels,
            "hint": "Two artifacts produced: parsed_text.txt (full text) and chunks.json (chunk array). \
                     Chunks have been auto-enqueued for background embedding. Use embed.search to query \
                     after the next tick cycle, or call embed.batch directly for immediate embedding.",
        }),
        produced_files: vec![
            ProducedFile {
                filename: "parsed_text.txt".into(),
                path: text_path,
                mime_type: Some("text/plain".into()),
                description: Some(format!(
                    "Extracted text from {format}: {char_count} chars, {word_count} words"
                )),
            },
            ProducedFile {
                filename: "chunks.json".into(),
                path: chunks_path,
                mime_type: Some("application/json".into()),
                description: Some(format!(
                    "{chunk_count} chunks (avg {avg_size} chars), prefix: {label_prefix}"
                )),
            },
        ],
    }
}
