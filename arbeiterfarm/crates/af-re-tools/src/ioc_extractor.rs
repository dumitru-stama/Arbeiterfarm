use af_plugin_api::{PluginDb, PluginDbError};
use regex::Regex;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

/// An extracted indicator of compromise.
#[derive(Debug, Clone)]
pub struct ExtractedIoc {
    pub ioc_type: String,
    pub value: String,
    pub context: Option<String>,
}

/// Regex-based IOC extraction from text.
pub struct IocExtractor {
    plugin_db: Arc<dyn PluginDb>,
}

impl IocExtractor {
    pub fn new(plugin_db: Arc<dyn PluginDb>) -> Self {
        Self { plugin_db }
    }

    /// Extract IOCs from arbitrary text (tool output, strings, etc.)
    pub fn extract(text: &str) -> Vec<ExtractedIoc> {
        let mut iocs = Vec::new();

        // IPv4 — exclude private/reserved ranges
        let ipv4_re = Regex::new(
            r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b",
        )
        .unwrap();
        for m in ipv4_re.find_iter(text) {
            let val = m.as_str();
            if !is_private_ipv4(val) {
                iocs.push(ExtractedIoc {
                    ioc_type: "ipv4".into(),
                    value: val.into(),
                    context: extract_context(text, m.start(), m.end()),
                });
            }
        }

        // IPv6 — simplified pattern for common forms
        let ipv6_re = Regex::new(
            r"(?i)\b(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}\b|(?i)\b(?:[0-9a-f]{1,4}:){1,7}:\b|(?i)\b::(?:[0-9a-f]{1,4}:){0,5}[0-9a-f]{1,4}\b",
        )
        .unwrap();
        for m in ipv6_re.find_iter(text) {
            let val = m.as_str();
            if !is_loopback_ipv6(val) {
                iocs.push(ExtractedIoc {
                    ioc_type: "ipv6".into(),
                    value: val.into(),
                    context: extract_context(text, m.start(), m.end()),
                });
            }
        }

        // SHA256 (must check before SHA1 and MD5 since longer hex strings match shorter patterns)
        let sha256_re = Regex::new(r"(?i)\b[a-f0-9]{64}\b").unwrap();
        for m in sha256_re.find_iter(text) {
            iocs.push(ExtractedIoc {
                ioc_type: "sha256".into(),
                value: m.as_str().to_lowercase(),
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // Collect SHA256 values to exclude from shorter hash matches
        let sha256_vals: Vec<String> = iocs
            .iter()
            .filter(|i| i.ioc_type == "sha256")
            .map(|i| i.value.clone())
            .collect();

        // SHA1
        let sha1_re = Regex::new(r"(?i)\b[a-f0-9]{40}\b").unwrap();
        for m in sha1_re.find_iter(text) {
            let val = m.as_str().to_lowercase();
            // Skip if this is a substring of a SHA256
            if sha256_vals.iter().any(|s| s.contains(&val)) {
                continue;
            }
            iocs.push(ExtractedIoc {
                ioc_type: "sha1".into(),
                value: val,
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // MD5
        let md5_re = Regex::new(r"(?i)\b[a-f0-9]{32}\b").unwrap();
        for m in md5_re.find_iter(text) {
            let val = m.as_str().to_lowercase();
            // Skip if substring of SHA1 or SHA256
            if sha256_vals.iter().any(|s| s.contains(&val)) {
                continue;
            }
            if iocs
                .iter()
                .any(|i| i.ioc_type == "sha1" && i.value.contains(&val))
            {
                continue;
            }
            iocs.push(ExtractedIoc {
                ioc_type: "md5".into(),
                value: val,
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // URLs
        let url_re = Regex::new(r#"https?://[^\s<>"\{\}|\\^`]+"#).unwrap();
        for m in url_re.find_iter(text) {
            iocs.push(ExtractedIoc {
                ioc_type: "url".into(),
                value: m.as_str().into(),
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // Domains — after URLs to avoid double-extraction
        let domain_re =
            Regex::new(r"\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}\b").unwrap();
        for m in domain_re.find_iter(text) {
            let val = m.as_str();
            if !is_safe_domain(val) {
                // Skip if already captured as part of a URL
                let already_in_url = iocs
                    .iter()
                    .any(|i| i.ioc_type == "url" && i.value.contains(val));
                if !already_in_url {
                    iocs.push(ExtractedIoc {
                        ioc_type: "domain".into(),
                        value: val.into(),
                        context: extract_context(text, m.start(), m.end()),
                    });
                }
            }
        }

        // Email addresses
        let email_re =
            Regex::new(r"\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b").unwrap();
        for m in email_re.find_iter(text) {
            iocs.push(ExtractedIoc {
                ioc_type: "email".into(),
                value: m.as_str().into(),
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // Named pipes
        let pipe_re = Regex::new(r"\\\\\.\\pipe\\[^\s]+").unwrap();
        for m in pipe_re.find_iter(text) {
            iocs.push(ExtractedIoc {
                ioc_type: "mutex".into(),
                value: m.as_str().into(),
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // Global/Local mutexes
        let mutex_re = Regex::new(r"(?:Global|Local)\\[^\s]+").unwrap();
        for m in mutex_re.find_iter(text) {
            iocs.push(ExtractedIoc {
                ioc_type: "mutex".into(),
                value: m.as_str().into(),
                context: extract_context(text, m.start(), m.end()),
            });
        }

        // Registry keys
        let reg_re = Regex::new(r"(?:HKLM|HKCU|HKEY_[A-Z_]+)\\[^\s]+").unwrap();
        for m in reg_re.find_iter(text) {
            iocs.push(ExtractedIoc {
                ioc_type: "registry_key".into(),
                value: m.as_str().into(),
                context: extract_context(text, m.start(), m.end()),
            });
        }

        iocs
    }

    /// Extract and store IOCs in re.iocs table.
    pub async fn extract_and_store(
        &self,
        text: &str,
        project_id: Uuid,
        source_tool_run: Option<Uuid>,
        user_id: Option<Uuid>,
    ) -> Result<Vec<Uuid>, PluginDbError> {
        let iocs = Self::extract(text);
        let mut ids = Vec::with_capacity(iocs.len());

        for ioc in &iocs {
            let tool_run_str = source_tool_run.map(|u| u.to_string());
            let params = vec![
                json!(project_id.to_string()),
                json!(ioc.ioc_type),
                json!(ioc.value),
                json!(tool_run_str),
                json!(ioc.context),
            ];

            let rows = self
                .plugin_db
                .query_json(
                    "INSERT INTO iocs (project_id, ioc_type, value, source_tool_run, context) \
                     VALUES ($1::uuid, $2, $3, $4::uuid, $5) RETURNING id",
                    params,
                    user_id,
                )
                .await?;

            if let Some(row) = rows.first() {
                if let Some(id_str) = row.get("id").and_then(|v| v.as_str()) {
                    if let Ok(uuid) = Uuid::parse_str(id_str) {
                        ids.push(uuid);
                    }
                }
            }
        }

        Ok(ids)
    }
}

/// Check if an IPv4 address is private/reserved.
fn is_private_ipv4(ip: &str) -> bool {
    ip.starts_with("10.")
        || ip.starts_with("192.168.")
        || ip.starts_with("127.")
        || ip.starts_with("0.")
        || ip.starts_with("169.254.")
        || ip == "255.255.255.255"
        || ip.starts_with("172.")
            && {
                let second: u8 = ip
                    .split('.')
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                (16..=31).contains(&second)
            }
}

fn is_loopback_ipv6(ip: &str) -> bool {
    let lower = ip.to_lowercase();
    lower == "::1" || lower.starts_with("fe80:") || lower.starts_with("fc") || lower.starts_with("fd")
}

/// Check if a domain is a known safe/false-positive value.
fn is_safe_domain(domain: &str) -> bool {
    let lower = domain.to_lowercase();
    let safe = [
        "localhost",
        "example.com",
        "example.org",
        "example.net",
        "test.com",
        "test.org",
    ];
    if safe.contains(&lower.as_str()) {
        return true;
    }
    // Common file extensions that look like domains
    let false_positive_tlds = [".exe", ".dll", ".sys", ".bin", ".dat", ".tmp", ".log", ".bak"];
    for ext in &false_positive_tlds {
        if lower.ends_with(ext) {
            return true;
        }
    }
    false
}

/// Extract ±50 chars of context around a match.
fn extract_context(text: &str, start: usize, end: usize) -> Option<String> {
    let ctx_start = start.saturating_sub(50);
    let ctx_end = (end + 50).min(text.len());
    // Find safe char boundaries
    let ctx_start = floor_char_boundary(text, ctx_start);
    let ctx_end = ceil_char_boundary(text, ctx_end);
    Some(text[ctx_start..ctx_end].to_string())
}

fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ipv4() {
        let text = "connecting to 8.8.8.8 on port 53";
        let iocs = IocExtractor::extract(text);
        assert!(iocs.iter().any(|i| i.ioc_type == "ipv4" && i.value == "8.8.8.8"));
    }

    #[test]
    fn test_exclude_private_ipv4() {
        let text = "local addr 192.168.1.1 and 10.0.0.1";
        let iocs = IocExtractor::extract(text);
        assert!(!iocs.iter().any(|i| i.ioc_type == "ipv4"));
    }

    #[test]
    fn test_extract_sha256() {
        let hash = "a".repeat(64);
        let text = format!("hash: {hash}");
        let iocs = IocExtractor::extract(&text);
        assert!(iocs.iter().any(|i| i.ioc_type == "sha256" && i.value == hash));
    }

    #[test]
    fn test_extract_url() {
        let text = "download from https://evil.com/malware.exe please";
        let iocs = IocExtractor::extract(text);
        assert!(iocs.iter().any(|i| i.ioc_type == "url" && i.value.contains("evil.com")));
    }

    #[test]
    fn test_extract_registry_key() {
        let text = r"writes to HKLM\Software\Microsoft\Windows\Run";
        let iocs = IocExtractor::extract(text);
        assert!(iocs.iter().any(|i| i.ioc_type == "registry_key"));
    }

    #[test]
    fn test_exclude_safe_domains() {
        let text = "test against example.com and file.exe";
        let iocs = IocExtractor::extract(text);
        assert!(!iocs.iter().any(|i| i.ioc_type == "domain"));
    }
}
