use regex::Regex;
use std::sync::LazyLock;

/// Regex-based redaction layer for stripping sensitive data before sending to external LLMs.
pub struct RedactionLayer {
    patterns: Vec<RedactionPattern>,
}

struct RedactionPattern {
    regex: Regex,
    replacement: &'static str,
}

static DEFAULT_PATTERNS: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| {
    vec![
        // API keys / secrets
        (r"sk-[a-zA-Z0-9_-]{20,}", "[REDACTED_KEY]"),
        (r"Bearer\s+[a-zA-Z0-9_\-\.]{20,}", "Bearer [REDACTED]"),
        (r"(?i)password\s*=\s*\S+", "password=[REDACTED]"),
        (r"(?i)api[_-]?key\s*[:=]\s*\S+", "api_key=[REDACTED]"),
        // AWS access key IDs
        (r"AKIA[0-9A-Z]{16}", "[REDACTED_AWS_KEY]"),
        // Database connection strings
        (r"(?i)(?:postgres|mysql|mongodb|redis)://\S+", "[REDACTED_DB_URL]"),
        // JWT tokens
        (r"eyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,}", "[REDACTED_JWT]"),
        // Private keys
        (r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----", "[REDACTED_PRIVATE_KEY]"),
        // GitHub tokens
        (r"gh[ps]_[a-zA-Z0-9]{36}", "[REDACTED_GH_TOKEN]"),
        // IP addresses (IPv4)
        (
            r"\b(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\b",
            "[REDACTED_IP]",
        ),
        // Home directory paths
        (r"/home/[a-zA-Z0-9_.-]+", "/home/[REDACTED]"),
        (r"/Users/[a-zA-Z0-9_.-]+", "/Users/[REDACTED]"),
    ]
});

impl Default for RedactionLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl RedactionLayer {
    pub fn new() -> Self {
        let patterns = DEFAULT_PATTERNS
            .iter()
            .filter_map(|(pattern, replacement)| {
                Regex::new(pattern).ok().map(|regex| RedactionPattern {
                    regex,
                    replacement,
                })
            })
            .collect();

        Self { patterns }
    }

    /// Redact sensitive patterns from text.
    pub fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for pattern in &self.patterns {
            result = pattern
                .regex
                .replace_all(&result, pattern.replacement)
                .to_string();
        }
        result
    }

    /// Recursively redact all string values in a JSON value.
    /// Walks objects and arrays, applying `redact()` to every string leaf.
    pub fn redact_json_values(&self, value: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::String(s) => Value::String(self.redact(s)),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.redact_json_values(v)))
                    .collect(),
            ),
            Value::Array(arr) => {
                Value::Array(arr.iter().map(|v| self.redact_json_values(v)).collect())
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_api_key() {
        let layer = RedactionLayer::new();
        let input = "Using key sk-ant-api03-xyzabc1234567890abcdef";
        let result = layer.redact(input);
        assert!(result.contains("[REDACTED_KEY]"));
        assert!(!result.contains("sk-ant-"));
    }

    #[test]
    fn test_redact_ip() {
        let layer = RedactionLayer::new();
        let input = "Connecting to 192.168.1.100 on port 8080";
        let result = layer.redact(input);
        assert!(result.contains("[REDACTED_IP]"));
        assert!(!result.contains("192.168.1.100"));
    }

    #[test]
    fn test_redact_home_path() {
        let layer = RedactionLayer::new();
        let input = "File at /home/user/secret/data.txt";
        let result = layer.redact(input);
        assert!(result.contains("/home/[REDACTED]"));
        assert!(!result.contains("/home/user"));
    }

    #[test]
    fn test_no_redaction_for_safe_text() {
        let layer = RedactionLayer::new();
        let input = "This is a normal message about file analysis.";
        assert_eq!(layer.redact(input), input);
    }

    #[test]
    fn test_redact_aws_key() {
        let layer = RedactionLayer::new();
        let input = "Key: AKIAIOSFODNN7EXAMPLE";
        let result = layer.redact(input);
        assert!(result.contains("[REDACTED_AWS_KEY]"));
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_redact_db_url() {
        let layer = RedactionLayer::new();
        let input = "Connecting to postgres://user:pass@localhost/mydb";
        let result = layer.redact(input);
        assert!(result.contains("[REDACTED_DB_URL]"));
        assert!(!result.contains("user:pass"));
    }

    #[test]
    fn test_redact_jwt() {
        let layer = RedactionLayer::new();
        let input = "Token: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let result = layer.redact(input);
        assert!(result.contains("[REDACTED_JWT]"));
        assert!(!result.contains("eyJhbGci"));
    }

    #[test]
    fn test_redact_github_token() {
        let layer = RedactionLayer::new();
        let input = "export GH_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let result = layer.redact(input);
        assert!(result.contains("[REDACTED_GH_TOKEN]"));
        assert!(!result.contains("ghp_ABCDEF"));
    }

    #[test]
    fn test_redact_json_values_nested() {
        let layer = RedactionLayer::new();
        let input = serde_json::json!({
            "url": "postgres://user:pass@localhost/db",
            "nested": {
                "key": "sk-ant-api03-xyzabc1234567890abcdef",
                "safe": "hello world"
            },
            "list": ["192.168.1.1", "normal text"],
            "number": 42
        });
        let result = layer.redact_json_values(&input);
        assert!(result["url"].as_str().unwrap().contains("[REDACTED_DB_URL]"));
        assert!(result["nested"]["key"].as_str().unwrap().contains("[REDACTED_KEY]"));
        assert_eq!(result["nested"]["safe"].as_str().unwrap(), "hello world");
        assert!(result["list"][0].as_str().unwrap().contains("[REDACTED_IP]"));
        assert_eq!(result["list"][1].as_str().unwrap(), "normal text");
        assert_eq!(result["number"].as_i64().unwrap(), 42);
    }

    #[test]
    fn test_redact_json_values_empty() {
        let layer = RedactionLayer::new();
        let input = serde_json::json!({});
        let result = layer.redact_json_values(&input);
        assert_eq!(result, input);
    }
}
