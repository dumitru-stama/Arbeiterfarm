use crate::envelope::{OopArtifact, OopResult};
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

pub fn execute(artifact: &OopArtifact, input: &serde_json::Value) -> OopResult {
    let path = &artifact.storage_path;

    // Line-based reading
    if let Some(line_start) = input.get("line_start").and_then(|v| v.as_u64()) {
        let line_count = input
            .get("line_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(20) as usize;
        let line_start = line_start as usize;

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

        let reader = BufReader::new(file);
        let mut lines = Vec::new();
        let mut line_num = 0usize;

        for line_result in reader.lines() {
            line_num += 1;
            if line_num < line_start {
                continue;
            }
            if lines.len() >= line_count {
                break;
            }
            match line_result {
                Ok(line) => lines.push(json!({
                    "line_number": line_num,
                    "content": line,
                })),
                Err(e) => lines.push(json!({
                    "line_number": line_num,
                    "error": format!("non-utf8: {e}"),
                })),
            }
        }

        return OopResult::Ok {
            output: json!({
                "filename": artifact.filename,
                "mode": "lines",
                "line_start": line_start,
                "lines_returned": lines.len(),
                "total_lines_scanned": line_num,
                "lines": lines,
            }),
            produced_files: vec![],
        };
    }

    // Byte-based reading
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let length = input
        .get("length")
        .and_then(|v| v.as_u64())
        .unwrap_or(4096) as usize;

    // Cap at 64KB
    let length = length.min(65536);

    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to open file: {e}"),
                retryable: false,
            };
        }
    };

    if let Err(e) = file.seek(SeekFrom::Start(offset)) {
        return OopResult::Error {
            code: "io_error".into(),
            message: format!("failed to seek: {e}"),
            retryable: false,
        };
    }

    let mut buf = vec![0u8; length];
    let bytes_read = match file.read(&mut buf) {
        Ok(n) => n,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to read: {e}"),
                retryable: false,
            };
        }
    };
    buf.truncate(bytes_read);

    // Try UTF-8, fall back to lossy
    let content = String::from_utf8(buf.clone())
        .unwrap_or_else(|_| String::from_utf8_lossy(&buf).to_string());

    OopResult::Ok {
        output: json!({
            "filename": artifact.filename,
            "mode": "bytes",
            "offset": offset,
            "bytes_read": bytes_read,
            "content": content,
            "is_utf8": std::str::from_utf8(&buf[..bytes_read]).is_ok(),
        }),
        produced_files: vec![],
    }
}
