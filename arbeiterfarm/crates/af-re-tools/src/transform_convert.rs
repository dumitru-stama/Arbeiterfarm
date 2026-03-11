//! `transform.convert` — Convert between structured data formats.
//!
//! Supports: JSON, YAML, TOML, XML.

use af_builtin_tools::envelope::{OopArtifact, OopResult, ProducedFile};
use serde_json::{json, Value};
use std::path::Path;

pub fn execute(artifact: &OopArtifact, input: &Value, scratch_dir: &Path) -> OopResult {
    let from_format = match input.get("from_format").and_then(|v| v.as_str()) {
        Some(f) => f,
        None => {
            return OopResult::Error {
                code: "missing_from_format".into(),
                message: "from_format parameter is required".into(),
                retryable: false,
            }
        }
    };
    let to_format = match input.get("to_format").and_then(|v| v.as_str()) {
        Some(f) => f,
        None => {
            return OopResult::Error {
                code: "missing_to_format".into(),
                message: "to_format parameter is required".into(),
                retryable: false,
            }
        }
    };

    if from_format == to_format {
        return OopResult::Error {
            code: "same_format".into(),
            message: format!("from_format and to_format are both '{from_format}'"),
            retryable: false,
        };
    }

    let content = match std::fs::read_to_string(&artifact.storage_path) {
        Ok(c) => c,
        Err(e) => {
            return OopResult::Error {
                code: "read_error".into(),
                message: format!("failed to read artifact as UTF-8: {e}"),
                retryable: false,
            }
        }
    };

    let input_size = content.len();

    // Parse input
    let parsed: Value = match from_format {
        "json" => match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                return OopResult::Error {
                    code: "parse_error".into(),
                    message: format!("failed to parse JSON: {e}"),
                    retryable: false,
                }
            }
        },
        "yaml" => match serde_yml::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                return OopResult::Error {
                    code: "parse_error".into(),
                    message: format!("failed to parse YAML: {e}"),
                    retryable: false,
                }
            }
        },
        "toml" => match toml::from_str::<Value>(&content) {
            Ok(v) => v,
            Err(e) => {
                return OopResult::Error {
                    code: "parse_error".into(),
                    message: format!("failed to parse TOML: {e}"),
                    retryable: false,
                }
            }
        },
        "xml" => match quick_xml::de::from_str::<Value>(&content) {
            Ok(v) => v,
            Err(e) => {
                return OopResult::Error {
                    code: "parse_error".into(),
                    message: format!("failed to parse XML: {e}"),
                    retryable: false,
                }
            }
        },
        other => {
            return OopResult::Error {
                code: "unsupported_format".into(),
                message: format!("unsupported from_format: {other}"),
                retryable: false,
            }
        }
    };

    // Serialize output
    let (output_text, ext) = match to_format {
        "json" => (
            serde_json::to_string_pretty(&parsed).unwrap_or_default(),
            "json",
        ),
        "yaml" => match serde_yml::to_string(&parsed) {
            Ok(s) => (s, "yaml"),
            Err(e) => {
                return OopResult::Error {
                    code: "serialize_error".into(),
                    message: format!("failed to serialize to YAML: {e}"),
                    retryable: false,
                }
            }
        },
        "toml" => match toml::to_string_pretty(&parsed) {
            Ok(s) => (s, "toml"),
            Err(e) => {
                return OopResult::Error {
                    code: "serialize_error".into(),
                    message: format!("failed to serialize to TOML: {e}"),
                    retryable: false,
                }
            }
        },
        "xml" => match quick_xml::se::to_string(&parsed) {
            Ok(s) => (s, "xml"),
            Err(e) => {
                return OopResult::Error {
                    code: "serialize_error".into(),
                    message: format!("failed to serialize to XML: {e}"),
                    retryable: false,
                }
            }
        },
        other => {
            return OopResult::Error {
                code: "unsupported_format".into(),
                message: format!("unsupported to_format: {other}"),
                retryable: false,
            }
        }
    };

    let output_size = output_text.len();
    let filename = format!("converted.{ext}");
    let out_path = scratch_dir.join(&filename);

    if let Err(e) = std::fs::write(&out_path, &output_text) {
        return OopResult::Error {
            code: "write_error".into(),
            message: format!("failed to write converted output: {e}"),
            retryable: false,
        };
    }

    let mime = match ext {
        "json" => "application/json",
        "yaml" => "text/yaml",
        "toml" => "text/toml",
        "xml" => "application/xml",
        _ => "text/plain",
    };

    OopResult::Ok {
        output: json!({
            "from_format": from_format,
            "to_format": to_format,
            "input_size": input_size,
            "output_size": output_size,
            "hint": "Converted file stored as artifact. Use file.read_range to inspect.",
        }),
        produced_files: vec![ProducedFile {
            filename,
            path: out_path,
            mime_type: Some(mime.into()),
            description: Some(format!("Converted from {from_format} to {to_format}")),
        }],
    }
}
