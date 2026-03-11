//! `transform.unpack` — Extract archive contents.
//!
//! Supports: ZIP, tar, tar.gz, tar.bz2, 7z.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

const DEFAULT_MAX_FILES: usize = 100;
const MAX_MAX_FILES: usize = 500;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let password = input.get("password").and_then(|v| v.as_str());
    let max_files = input
        .get("max_files")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_MAX_FILES as u64)
        .min(MAX_MAX_FILES as u64) as usize;
    let max_total_bytes = input
        .get("max_total_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_MAX_TOTAL_BYTES);

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

    let out_dir = scratch_dir.join("unpacked");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        return OopResult::Error {
            code: "mkdir_error".into(),
            message: format!("failed to create output directory: {e}"),
            retryable: false,
        };
    }

    // Detect format from magic bytes + extension
    let format = detect_format(&data, &artifact.filename);

    let result = match format.as_str() {
        "zip" => extract_zip(&artifact.storage_path, password, &out_dir, max_files, max_total_bytes),
        "tar.gz" => extract_tar_gz(&data, &out_dir, max_files, max_total_bytes),
        "tar.bz2" => extract_tar_bz2(&data, &out_dir, max_files, max_total_bytes),
        "7z" => extract_7z(&artifact.storage_path, &out_dir, max_files, max_total_bytes),
        "tar" => extract_tar(&data, &out_dir, max_files, max_total_bytes),
        _ => Err(format!(
            "unsupported or unrecognized archive format (detected: {format})"
        )),
    };

    match result {
        Ok((files, total_bytes)) => {
            let file_count = files.len();

            // Build file listing (first 50)
            let listing: Vec<Value> = files
                .iter()
                .take(50)
                .map(|f| {
                    let size = std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0);
                    json!({
                        "name": f.filename,
                        "size": size,
                    })
                })
                .collect();

            OopResult::Ok {
                output: json!({
                    "archive_type": format,
                    "file_count": file_count,
                    "total_bytes": total_bytes,
                    "files": listing,
                    "truncated": file_count > 50,
                    "hint": "Extracted files stored as artifacts. Use file.info and file.read_range to inspect.",
                }),
                produced_files: files,
            }
        }
        Err(e) => OopResult::Error {
            code: "unpack_error".into(),
            message: e,
            retryable: false,
        },
    }
}

fn detect_format(data: &[u8], filename: &str) -> String {
    if data.len() >= 4 {
        // ZIP magic
        if data[0] == 0x50 && data[1] == 0x4B && data[2] == 0x03 && data[3] == 0x04 {
            return "zip".into();
        }
        // gzip magic
        if data[0] == 0x1F && data[1] == 0x8B {
            return "tar.gz".into();
        }
        // bzip2 magic
        if data[0] == 0x42 && data[1] == 0x5A && data[2] == 0x68 {
            return "tar.bz2".into();
        }
    }
    if data.len() >= 6 {
        // 7z magic: 37 7A BC AF 27 1C
        if data[0] == 0x37
            && data[1] == 0x7A
            && data[2] == 0xBC
            && data[3] == 0xAF
            && data[4] == 0x27
            && data[5] == 0x1C
        {
            return "7z".into();
        }
    }

    // Fallback to extension
    let lower = filename.to_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        "tar.gz".into()
    } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
        "tar.bz2".into()
    } else if lower.ends_with(".tar") {
        "tar".into()
    } else if lower.ends_with(".zip") {
        "zip".into()
    } else if lower.ends_with(".7z") {
        "7z".into()
    } else {
        "unknown".into()
    }
}

/// Validate path component: reject `..`, absolute paths, and null bytes.
fn validate_entry_name(name: &str) -> Result<String, String> {
    if name.contains('\0') {
        return Err("entry name contains null byte".into());
    }
    let path = std::path::Path::new(name);
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(format!("path traversal in entry: {name}"))
            }
            std::path::Component::RootDir => {
                return Err(format!("absolute path in entry: {name}"))
            }
            _ => {}
        }
    }
    // Use just the filename for flat extraction
    let sanitized = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");
    Ok(sanitized.to_string())
}

fn extract_zip(
    path: &Path,
    password: Option<&str>,
    out_dir: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Result<(Vec<ProducedFile>, u64), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip open: {e}"))?;

    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;

    for i in 0..archive.len() {
        if files.len() >= max_files {
            break;
        }

        let mut entry = if let Some(pw) = password {
            archive
                .by_index_decrypt(i, pw.as_bytes())
                .map_err(|e| format!("zip entry {i}: {e}"))?
        } else {
            archive
                .by_index(i)
                .map_err(|e| format!("zip entry {i}: {e}"))?
        };

        if entry.is_dir() {
            continue;
        }

        let name = entry.name().to_string();
        let safe_name = match validate_entry_name(&name) {
            Ok(n) => n,
            Err(_) => continue,
        };

        // Check decompressed size before reading
        let declared_size = entry.size();
        if total_bytes + declared_size > max_total_bytes {
            break;
        }

        let out_name = deduplicate_name(&files, &safe_name);
        let out_path = out_dir.join(&out_name);

        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("zip read {name}: {e}"))?;

        if total_bytes + buf.len() as u64 > max_total_bytes {
            break;
        }

        std::fs::write(&out_path, &buf).map_err(|e| format!("write {out_name}: {e}"))?;
        total_bytes += buf.len() as u64;

        files.push(ProducedFile {
            filename: out_name,
            path: out_path,
            mime_type: None,
            description: Some(format!("Extracted from ZIP: {name}")),
        });
    }

    Ok((files, total_bytes))
}

fn extract_tar(
    data: &[u8],
    out_dir: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Result<(Vec<ProducedFile>, u64), String> {
    extract_tar_from_reader(data, out_dir, max_files, max_total_bytes)
}

fn extract_tar_gz(
    data: &[u8],
    out_dir: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Result<(Vec<ProducedFile>, u64), String> {
    let decoder = flate2::read::GzDecoder::new(data);
    let decompressed = read_with_limit(decoder, max_total_bytes)?;
    extract_tar_from_reader(&decompressed, out_dir, max_files, max_total_bytes)
}

fn extract_tar_bz2(
    data: &[u8],
    out_dir: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Result<(Vec<ProducedFile>, u64), String> {
    let decoder = bzip2::read::BzDecoder::new(data);
    let decompressed = read_with_limit(decoder, max_total_bytes)?;
    extract_tar_from_reader(&decompressed, out_dir, max_files, max_total_bytes)
}

fn read_with_limit<R: Read>(mut reader: R, limit: u64) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut limited = reader.by_ref().take(limit + 1);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| format!("decompress: {e}"))?;
    if buf.len() as u64 > limit {
        return Err(format!(
            "decompressed size exceeds limit ({} bytes)",
            limit
        ));
    }
    Ok(buf)
}

fn extract_tar_from_reader(
    data: &[u8],
    out_dir: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Result<(Vec<ProducedFile>, u64), String> {
    let mut archive = tar::Archive::new(data);
    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;

    let entries = archive.entries().map_err(|e| format!("tar entries: {e}"))?;

    for entry_result in entries {
        if files.len() >= max_files {
            break;
        }

        let mut entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[transform.unpack] skipping bad tar entry: {e}");
                continue;
            }
        };

        if entry.header().entry_type().is_dir() {
            continue;
        }

        let name = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unnamed".into());

        let safe_name = match validate_entry_name(&name) {
            Ok(n) => n,
            Err(_) => continue,
        };

        let entry_size = entry.header().size().unwrap_or(0);
        if total_bytes + entry_size > max_total_bytes {
            break;
        }

        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() {
            continue;
        }

        if total_bytes + buf.len() as u64 > max_total_bytes {
            break;
        }

        let out_name = deduplicate_name(&files, &safe_name);
        let out_path = out_dir.join(&out_name);

        std::fs::write(&out_path, &buf).map_err(|e| format!("write {out_name}: {e}"))?;
        total_bytes += buf.len() as u64;

        files.push(ProducedFile {
            filename: out_name,
            path: out_path,
            mime_type: None,
            description: Some(format!("Extracted from tar: {name}")),
        });
    }

    Ok((files, total_bytes))
}

fn extract_7z(
    path: &Path,
    out_dir: &Path,
    max_files: usize,
    max_total_bytes: u64,
) -> Result<(Vec<ProducedFile>, u64), String> {
    // NOTE: sevenz-rust extracts everything before we can check limits.
    // We pre-check the archive's declared sizes to catch obvious bombs,
    // then enforce limits on the actually extracted files.
    let file = std::fs::File::open(path).map_err(|e| format!("7z open: {e}"))?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let archive =
        sevenz_rust::Archive::read(&mut std::io::BufReader::new(&file), file_len, &[])
            .map_err(|e| format!("7z read: {e}"))?;

    // Pre-flight: check declared sizes to reject obvious bombs
    let mut declared_total: u64 = 0;
    let mut declared_files: usize = 0;
    for entry in archive.files.iter() {
        if !entry.is_directory() {
            declared_files += 1;
            declared_total = declared_total.saturating_add(entry.size());
        }
    }
    if declared_files > max_files {
        return Err(format!(
            "7z archive contains {declared_files} files, exceeds limit of {max_files}"
        ));
    }
    if declared_total > max_total_bytes {
        return Err(format!(
            "7z archive declares {} bytes total, exceeds limit of {} bytes",
            declared_total, max_total_bytes
        ));
    }

    sevenz_rust::decompress_file(path, out_dir).map_err(|e| format!("7z extract: {e}"))?;

    // Collect extracted files (still enforce limits on actual sizes)
    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;
    collect_files_recursive(
        out_dir,
        out_dir,
        &mut files,
        &mut total_bytes,
        max_files,
        max_total_bytes,
    );

    Ok((files, total_bytes))
}

fn collect_files_recursive(
    base: &Path,
    dir: &Path,
    files: &mut Vec<ProducedFile>,
    total_bytes: &mut u64,
    max_files: usize,
    max_total_bytes: u64,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if files.len() >= max_files || *total_bytes > max_total_bytes {
            break;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(base, &path, files, total_bytes, max_files, max_total_bytes);
        } else if path.is_file() {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if *total_bytes + size > max_total_bytes {
                break;
            }
            *total_bytes += size;

            let rel = path.strip_prefix(base).unwrap_or(&path);
            let filename = rel
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string();
            let out_name = deduplicate_name(files, &filename);
            let rel_display = rel.display().to_string();

            // If dedup changed the name, rename the file
            if out_name != filename {
                let new_path = path.parent().unwrap_or(base).join(&out_name);
                let _ = std::fs::rename(&path, &new_path);
                files.push(ProducedFile {
                    filename: out_name,
                    path: new_path,
                    mime_type: None,
                    description: Some(format!("Extracted from 7z: {rel_display}")),
                });
            } else {
                files.push(ProducedFile {
                    filename: out_name,
                    path,
                    mime_type: None,
                    description: Some(format!("Extracted from 7z: {rel_display}")),
                });
            }
        }
    }
}

/// Ensure unique filenames among produced files.
fn deduplicate_name(existing: &[ProducedFile], name: &str) -> String {
    if !existing.iter().any(|f| f.filename == name) {
        return name.to_string();
    }
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    for i in 1..10000 {
        let candidate = format!("{stem}_{i}{ext}");
        if !existing.iter().any(|f| f.filename == candidate) {
            return candidate;
        }
    }
    format!("{stem}_dup{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_zip() {
        let data = vec![0x50, 0x4B, 0x03, 0x04, 0x00, 0x00];
        assert_eq!(detect_format(&data, "test.bin"), "zip");
    }

    #[test]
    fn test_detect_format_gzip() {
        let data = vec![0x1F, 0x8B, 0x08, 0x00];
        assert_eq!(detect_format(&data, "test.bin"), "tar.gz");
    }

    #[test]
    fn test_detect_format_7z() {
        let data = vec![0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
        assert_eq!(detect_format(&data, "test.bin"), "7z");
    }

    #[test]
    fn test_detect_format_by_extension() {
        let data = vec![0x00; 10];
        assert_eq!(detect_format(&data, "archive.tar"), "tar");
        assert_eq!(detect_format(&data, "archive.tar.gz"), "tar.gz");
        assert_eq!(detect_format(&data, "archive.tgz"), "tar.gz");
    }

    #[test]
    fn test_validate_entry_name_ok() {
        assert!(validate_entry_name("file.txt").is_ok());
        assert!(validate_entry_name("dir/file.txt").is_ok());
    }

    #[test]
    fn test_validate_entry_name_traversal() {
        assert!(validate_entry_name("../etc/passwd").is_err());
        assert!(validate_entry_name("foo/../../bar").is_err());
    }

    #[test]
    fn test_validate_entry_name_absolute() {
        assert!(validate_entry_name("/etc/passwd").is_err());
    }

    #[test]
    fn test_deduplicate_name() {
        let existing = vec![ProducedFile {
            filename: "test.txt".into(),
            path: "/tmp/test.txt".into(),
            mime_type: None,
            description: None,
        }];
        assert_eq!(deduplicate_name(&existing, "test.txt"), "test_1.txt");
        assert_eq!(deduplicate_name(&existing, "other.txt"), "other.txt");
    }
}
