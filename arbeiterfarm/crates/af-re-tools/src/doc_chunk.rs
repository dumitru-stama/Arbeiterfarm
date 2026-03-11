//! `doc.chunk` — Split text into overlapping chunks suitable for embedding.
//!
//! Chunks at paragraph boundaries (\n\n), falls back to line breaks (\n),
//! sentence boundaries (. ), word boundaries ( ), then hard cut.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const MAX_CHUNKS: usize = 10_000;

/// Snap a byte position down to the nearest char boundary (never exceeds `pos`).
fn floor_char_boundary(text: &str, mut pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub index: usize,
    pub offset: usize,
    pub length: usize,
    pub text: String,
    pub label: String,
}

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

    let text = String::from_utf8_lossy(&data);

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

    let chunks = chunk_text(&text, chunk_size, chunk_overlap, label_prefix);

    let chunk_count = chunks.len();
    let total_chars: usize = chunks.iter().map(|c| c.length).sum();
    let avg_size = if chunk_count > 0 {
        total_chars / chunk_count
    } else {
        0
    };

    let chunks_json =
        serde_json::to_string_pretty(&chunks).unwrap_or_else(|_| "[]".to_string());

    let out_path = scratch_dir.join("chunks.json");
    if let Err(e) = std::fs::write(&out_path, &chunks_json) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write chunks: {e}"),
            retryable: false,
        };
    }

    OopResult::Ok {
        output: json!({
            "chunk_count": chunk_count,
            "avg_chunk_size": avg_size,
            "total_chars": total_chars,
            "label_prefix": label_prefix,
            "hint": "Chunks stored as artifact and auto-enqueued for background embedding. \
                     Use embed.search to query after the next tick cycle, or call embed.batch directly for immediate embedding.",
        }),
        produced_files: vec![ProducedFile {
            filename: "chunks.json".into(),
            path: out_path,
            mime_type: Some("application/json".into()),
            description: Some(format!(
                "{chunk_count} chunks (avg {avg_size} chars), prefix: {label_prefix}"
            )),
        }],
    }
}

/// Core chunking algorithm. Exported for use by doc_ingest.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize, prefix: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let text_len = text.len();

    if text_len == 0 {
        return chunks;
    }

    let mut start = 0;

    while start < text_len && chunks.len() < MAX_CHUNKS {
        // Snap end to a valid char boundary (don't exceed chunk_size bytes)
        let end = floor_char_boundary(text, (start + chunk_size).min(text_len));

        // If we're at the end of the text, just take everything remaining
        if end >= text_len {
            let chunk_text = &text[start..text_len];
            if !chunk_text.trim().is_empty() {
                chunks.push(Chunk {
                    index: chunks.len(),
                    offset: start,
                    length: text_len - start,
                    text: chunk_text.to_string(),
                    label: format!("{}_chunk_{}", prefix, chunks.len()),
                });
            }
            break;
        }

        // Search backwards from `end` for the best boundary
        let boundary = find_boundary(text, start, end);

        let chunk_text = &text[start..boundary];
        if !chunk_text.trim().is_empty() {
            chunks.push(Chunk {
                index: chunks.len(),
                offset: start,
                length: boundary - start,
                text: chunk_text.to_string(),
                label: format!("{}_chunk_{}", prefix, chunks.len()),
            });
        }

        // Advance with overlap (but never backwards), snapping to char boundary
        let next_start = if overlap > 0 && boundary > overlap {
            floor_char_boundary(text, boundary - overlap)
        } else {
            boundary
        };

        if next_start <= start {
            // Avoid infinite loop — force advance
            start = boundary;
        } else {
            start = next_start;
        }
    }

    chunks
}

/// Find the best boundary position by searching backwards from `end`.
/// Priority: paragraph (\n\n) > line (\n) > sentence (. ) > word ( ) > hard cut.
fn find_boundary(text: &str, start: usize, end: usize) -> usize {
    let region = &text[start..end];

    // Try paragraph boundary (\n\n)
    if let Some(pos) = region.rfind("\n\n") {
        let boundary = start + pos + 2; // Include the \n\n
        if boundary > start {
            return boundary;
        }
    }

    // Try line boundary (\n)
    if let Some(pos) = region.rfind('\n') {
        let boundary = start + pos + 1;
        if boundary > start {
            return boundary;
        }
    }

    // Try sentence boundary (. )
    if let Some(pos) = region.rfind(". ") {
        let boundary = start + pos + 2;
        if boundary > start {
            return boundary;
        }
    }

    // Try word boundary ( )
    if let Some(pos) = region.rfind(' ') {
        let boundary = start + pos + 1;
        if boundary > start {
            return boundary;
        }
    }

    // Hard cut
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_empty() {
        let chunks = chunk_text("", 100, 20, "test");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_small_text() {
        let chunks = chunk_text("Hello, world!", 1000, 200, "test");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello, world!");
        assert_eq!(chunks[0].label, "test_chunk_0");
    }

    #[test]
    fn test_chunk_paragraph_boundaries() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_text(text, 30, 0, "doc");
        assert!(chunks.len() >= 2);
        // Should split at paragraph boundaries
        assert!(chunks[0].text.contains("First paragraph"));
    }

    #[test]
    fn test_chunk_overlap() {
        let text = "AAAA.\n\nBBBB.\n\nCCCC.\n\nDDDD.";
        let chunks = chunk_text(text, 10, 5, "test");
        // With overlap, some text should appear in multiple chunks
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_chunk_labels() {
        let text = "A paragraph.\n\nAnother paragraph.\n\nYet another.";
        let chunks = chunk_text(text, 20, 0, "report");
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
            assert_eq!(chunk.label, format!("report_chunk_{i}"));
        }
    }

    #[test]
    fn test_chunk_max_limit() {
        // Create text that would generate more than MAX_CHUNKS
        let text = "a\n\n".repeat(MAX_CHUNKS + 100);
        let chunks = chunk_text(&text, 4, 0, "test");
        assert!(chunks.len() <= MAX_CHUNKS);
    }

    #[test]
    fn test_chunk_multibyte_utf8() {
        // Each char is 3 bytes (U+4E16 etc). chunk_size=10 bytes can land mid-char.
        let text = "世界你好世界你好世界你好";
        let chunks = chunk_text(text, 10, 3, "utf8");
        assert!(!chunks.is_empty());
        // Verify all chunks are valid UTF-8 (would panic during slicing if not)
        for chunk in &chunks {
            assert!(!chunk.text.is_empty());
            // Double-check round-trip
            let _: Vec<char> = chunk.text.chars().collect();
        }
    }

    #[test]
    fn test_chunk_no_infinite_loop() {
        // Single word longer than chunk_size — should hard cut
        let text = "a".repeat(200);
        let chunks = chunk_text(&text, 50, 10, "test");
        assert!(!chunks.is_empty());
        // Should terminate without hanging
    }
}
