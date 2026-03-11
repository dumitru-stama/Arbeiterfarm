use crate::envelope::{OopArtifact, OopResult};
use md5::{Digest as Md5Digest, Md5};
use sha2::Sha256;
use serde_json::json;
use std::fs;
use std::io::Read;

/// Detect file type from magic bytes.
fn detect_magic(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "unknown";
    }
    match &data[..4] {
        [0x7f, b'E', b'L', b'F'] => "ELF",
        [b'M', b'Z', ..] => "PE/MZ",
        [0x89, b'P', b'N', b'G'] => "PNG",
        [0xff, 0xd8, 0xff, ..] => "JPEG",
        [b'G', b'I', b'F', b'8'] => "GIF",
        [b'P', b'K', 0x03, 0x04] => "ZIP/JAR/APK",
        [0x1f, 0x8b, ..] => "gzip",
        [b'%', b'P', b'D', b'F'] => "PDF",
        [0xca, 0xfe, 0xba, 0xbe] => "Mach-O (fat)",
        [0xfe, 0xed, 0xfa, 0xce] => "Mach-O (32-bit)",
        [0xfe, 0xed, 0xfa, 0xcf] => "Mach-O (64-bit)",
        [0xce, 0xfa, 0xed, 0xfe] => "Mach-O (32-bit, reversed)",
        [0xcf, 0xfa, 0xed, 0xfe] => "Mach-O (64-bit, reversed)",
        _ => {
            // Check if it looks like UTF-8 text
            if std::str::from_utf8(data).is_ok() {
                "text"
            } else {
                "binary"
            }
        }
    }
}

pub fn execute(artifact: &OopArtifact) -> OopResult {
    let path = &artifact.storage_path;

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

    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to read metadata: {e}"),
                retryable: false,
            };
        }
    };

    let size = metadata.len();

    // Read first 4096 bytes for magic detection
    let mut header = vec![0u8; 4096.min(size as usize)];
    if let Err(e) = file.read_exact(&mut header) {
        return OopResult::Error {
            code: "io_error".into(),
            message: format!("failed to read header: {e}"),
            retryable: false,
        };
    }
    let magic = detect_magic(&header);

    // Compute hashes by re-reading the file
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return OopResult::Error {
                code: "io_error".into(),
                message: format!("failed to read file for hashing: {e}"),
                retryable: false,
            };
        }
    };

    let md5_hash = {
        let mut hasher = Md5::new();
        hasher.update(&data);
        hex::encode(hasher.finalize())
    };

    let sha256_hash = {
        let mut hasher = Sha256::new();
        hasher.update(&data);
        hex::encode(hasher.finalize())
    };

    OopResult::Ok {
        output: json!({
            "filename": artifact.filename,
            "size_bytes": size,
            "magic_type": magic,
            "md5": md5_hash,
            "sha256": sha256_hash,
            "mime_type": artifact.mime_type,
        }),
        produced_files: vec![],
    }
}
