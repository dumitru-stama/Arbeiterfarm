//! `transform.decode` — Decode/decompress a single blob.
//!
//! Supports: base64, base64url, hex, url, xor, gzip, zlib, bzip2.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

/// Max decompressed output size: 256 MB (prevents decompression bombs).
const MAX_DECOMPRESS_SIZE: u64 = 256 * 1024 * 1024;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let encoding = match input.get("encoding").and_then(|v| v.as_str()) {
        Some(e) => e,
        None => {
            return OopResult::Error {
                code: "missing_encoding".into(),
                message: "encoding parameter is required".into(),
                retryable: false,
            }
        }
    };

    let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let length = input.get("length").and_then(|v| v.as_u64()).map(|l| l as usize);

    // Read source data
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

    if offset >= raw.len() {
        return OopResult::Error {
            code: "invalid_offset".into(),
            message: format!("offset {} exceeds file size {}", offset, raw.len()),
            retryable: false,
        };
    }

    let data = match length {
        Some(len) => &raw[offset..raw.len().min(offset + len)],
        None => &raw[offset..],
    };

    let input_size = data.len();

    let decoded = match encoding {
        "base64" => {
            use base64::Engine;
            // Strip whitespace before decoding
            let clean: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(&clean)
                .map_err(|e| format!("base64 decode error: {e}"))
        }
        "base64url" => {
            use base64::Engine;
            let clean: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&clean)
                .map_err(|e| format!("base64url decode error: {e}"))
        }
        "hex" => {
            let hex_str: String = data
                .iter()
                .filter(|b| !b.is_ascii_whitespace())
                .map(|&b| b as char)
                .collect();
            if hex_str.len() % 2 != 0 {
                Err("hex string has odd length".into())
            } else {
                let mut out = Vec::with_capacity(hex_str.len() / 2);
                for chunk in hex_str.as_bytes().chunks(2) {
                    let s = std::str::from_utf8(chunk).unwrap_or("00");
                    match u8::from_str_radix(s, 16) {
                        Ok(b) => out.push(b),
                        Err(e) => return OopResult::Error {
                            code: "hex_error".into(),
                            message: format!("invalid hex at '{}': {e}", s),
                            retryable: false,
                        },
                    }
                }
                Ok(out)
            }
        }
        "url" => {
            let s = String::from_utf8_lossy(data);
            Ok(percent_decode(&s))
        }
        "xor" => {
            let key_hex = match input.get("key").and_then(|v| v.as_str()) {
                Some(k) => k,
                None => {
                    return OopResult::Error {
                        code: "missing_key".into(),
                        message: "key parameter is required for XOR encoding".into(),
                        retryable: false,
                    }
                }
            };
            let key = parse_hex_key(key_hex);
            match key {
                Ok(k) => {
                    if k.is_empty() {
                        Err("XOR key cannot be empty".into())
                    } else if k.len() > 256 {
                        Err("XOR key exceeds 256 bytes".into())
                    } else {
                        Ok(xor_decode(data, &k))
                    }
                }
                Err(e) => Err(e),
            }
        }
        "gzip" => {
            let decoder = flate2::read::GzDecoder::new(data);
            decompress_with_limit(decoder, "gzip")
        }
        "zlib" => {
            let decoder = flate2::read::ZlibDecoder::new(data);
            decompress_with_limit(decoder, "zlib")
        }
        "bzip2" => {
            let decoder = bzip2::read::BzDecoder::new(data);
            decompress_with_limit(decoder, "bzip2")
        }
        other => {
            return OopResult::Error {
                code: "unsupported_encoding".into(),
                message: format!("unsupported encoding: {other}"),
                retryable: false,
            }
        }
    };

    let decoded = match decoded {
        Ok(d) => d,
        Err(e) => {
            return OopResult::Error {
                code: "decode_error".into(),
                message: e,
                retryable: false,
            }
        }
    };

    let output_size = decoded.len();

    // Write output
    let out_path = scratch_dir.join("decoded.bin");
    if let Err(e) = std::fs::write(&out_path, &decoded) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write decoded output: {e}"),
            retryable: false,
        };
    }

    // Build text preview
    let preview = build_preview(&decoded, 200);

    OopResult::Ok {
        output: json!({
            "encoding": encoding,
            "input_size": input_size,
            "output_size": output_size,
            "preview": preview,
            "hint": "Full decoded data stored as artifact. Use file.read_range or file.hexdump to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename: "decoded.bin".into(),
            path: out_path,
            mime_type: Some("application/octet-stream".into()),
            description: Some(format!("Decoded from {encoding}: {input_size} → {output_size} bytes")),
        }],
    }
}

/// Decompress with a size limit to prevent decompression bombs.
fn decompress_with_limit<R: Read>(reader: R, codec: &str) -> Result<Vec<u8>, String> {
    let mut limited = reader.take(MAX_DECOMPRESS_SIZE + 1);
    let mut buf = Vec::new();
    limited
        .read_to_end(&mut buf)
        .map_err(|e| format!("{codec} decompress error: {e}"))?;
    if buf.len() as u64 > MAX_DECOMPRESS_SIZE {
        return Err(format!(
            "{codec} decompressed size exceeds limit ({} MB)",
            MAX_DECOMPRESS_SIZE / (1024 * 1024)
        ));
    }
    Ok(buf)
}

fn build_preview(data: &[u8], max_bytes: usize) -> String {
    let slice = &data[..data.len().min(max_bytes)];
    if slice.iter().all(|&b| b == b'\n' || b == b'\r' || b == b'\t' || (0x20..=0x7e).contains(&b)) {
        let s = String::from_utf8_lossy(slice);
        if data.len() > max_bytes {
            format!("{}...", s)
        } else {
            s.into_owned()
        }
    } else {
        // Hex preview
        let hex: Vec<String> = slice.iter().take(64).map(|b| format!("{b:02x}")).collect();
        if data.len() > 64 {
            format!("{}...", hex.join(" "))
        } else {
            hex.join(" ")
        }
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00"),
                16,
            ) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    out
}

fn parse_hex_key(hex: &str) -> Result<Vec<u8>, String> {
    let clean: String = hex.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if clean.len() % 2 != 0 {
        return Err("XOR key hex has odd length".into());
    }
    let mut key = Vec::with_capacity(clean.len() / 2);
    for chunk in clean.as_bytes().chunks(2) {
        let s = std::str::from_utf8(chunk).unwrap_or("00");
        match u8::from_str_radix(s, 16) {
            Ok(b) => key.push(b),
            Err(e) => return Err(format!("invalid hex in key at '{}': {e}", s)),
        }
    }
    Ok(key)
}

fn xor_decode(data: &[u8], key: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, &b)| b ^ key[i % key.len()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("hello%20world"), b"hello world");
        assert_eq!(percent_decode("a+b"), b"a b");
        assert_eq!(percent_decode("%41%42%43"), b"ABC");
    }

    #[test]
    fn test_xor_decode() {
        let data = vec![0x41 ^ 0xFF, 0x42 ^ 0xFF];
        let result = xor_decode(&data, &[0xFF]);
        assert_eq!(result, vec![0x41, 0x42]);
    }

    #[test]
    fn test_xor_decode_multi_byte_key() {
        let key = vec![0x12, 0x34];
        let data = vec![0x41 ^ 0x12, 0x42 ^ 0x34, 0x43 ^ 0x12];
        let result = xor_decode(&data, &key);
        assert_eq!(result, vec![0x41, 0x42, 0x43]);
    }

    #[test]
    fn test_parse_hex_key() {
        assert_eq!(parse_hex_key("FF").unwrap(), vec![0xFF]);
        assert_eq!(parse_hex_key("12 34").unwrap(), vec![0x12, 0x34]);
        assert!(parse_hex_key("F").is_err());
        assert!(parse_hex_key("GG").is_err());
    }

    #[test]
    fn test_build_preview_text() {
        let data = b"Hello, world!";
        let preview = build_preview(data, 200);
        assert_eq!(preview, "Hello, world!");
    }

    #[test]
    fn test_build_preview_binary() {
        let data = vec![0x00, 0x01, 0x02, 0xFF];
        let preview = build_preview(&data, 200);
        assert_eq!(preview, "00 01 02 ff");
    }
}
