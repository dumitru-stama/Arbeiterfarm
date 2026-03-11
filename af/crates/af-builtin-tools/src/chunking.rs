//! Shared text chunking algorithm for embedding pipelines.
//!
//! Chunks at paragraph boundaries (\n\n), falls back to line breaks (\n),
//! sentence boundaries (. ), word boundaries ( ), then hard cut.

use serde::{Deserialize, Serialize};

const MAX_CHUNKS: usize = 10_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub index: usize,
    pub offset: usize,
    pub length: usize,
    pub text: String,
    pub label: String,
}

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

/// Split text into overlapping chunks suitable for embedding.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize, prefix: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let text_len = text.len();

    if text_len == 0 || chunk_size == 0 {
        return chunks;
    }

    let mut start = 0;

    while start < text_len && chunks.len() < MAX_CHUNKS {
        let end = floor_char_boundary(text, (start + chunk_size).min(text_len));

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

        let next_start = if overlap > 0 && boundary > overlap {
            floor_char_boundary(text, boundary - overlap)
        } else {
            boundary
        };

        if next_start <= start {
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

    if let Some(pos) = region.rfind("\n\n") {
        let boundary = start + pos + 2;
        if boundary > start {
            return boundary;
        }
    }

    if let Some(pos) = region.rfind('\n') {
        let boundary = start + pos + 1;
        if boundary > start {
            return boundary;
        }
    }

    if let Some(pos) = region.rfind(". ") {
        let boundary = start + pos + 2;
        if boundary > start {
            return boundary;
        }
    }

    if let Some(pos) = region.rfind(' ') {
        let boundary = start + pos + 1;
        if boundary > start {
            return boundary;
        }
    }

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
        assert!(chunks[0].text.contains("First paragraph"));
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
}
