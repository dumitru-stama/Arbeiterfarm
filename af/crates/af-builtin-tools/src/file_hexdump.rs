use crate::envelope::{OopArtifact, OopResult};
use serde_json::json;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

pub fn execute(artifact: &OopArtifact, input: &serde_json::Value) -> OopResult {
    let path = &artifact.storage_path;
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let length = input
        .get("length")
        .and_then(|v| v.as_u64())
        .unwrap_or(256) as usize;

    // Cap at 4KB
    let length = length.min(4096);

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

    // Build hex dump lines (16 bytes per line)
    let mut lines = Vec::new();
    for chunk_offset in (0..bytes_read).step_by(16) {
        let end = (chunk_offset + 16).min(bytes_read);
        let chunk = &buf[chunk_offset..end];

        let hex_part: String = chunk
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");

        let ascii_part: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();

        let addr = offset as usize + chunk_offset;
        lines.push(format!("{addr:08x}  {hex_part:<48}  |{ascii_part}|"));
    }

    OopResult::Ok {
        output: json!({
            "filename": artifact.filename,
            "offset": offset,
            "bytes_read": bytes_read,
            "hexdump": lines.join("\n"),
        }),
        produced_files: vec![],
    }
}
