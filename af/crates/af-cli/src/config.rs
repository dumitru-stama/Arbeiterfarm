use serde::Deserialize;
use std::path::PathBuf;

/// Top-level configuration loaded from `~/.af/config.toml`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ArbeiterfarmConfig {
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub server: ServerConfig,
    pub compaction: CompactionConfig,
}

impl Default for ArbeiterfarmConfig {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            storage: StorageConfig::default(),
            server: ServerConfig::default(),
            compaction: CompactionConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
    pub pool_size: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://af:af@localhost/af".to_string(),
            pool_size: 10,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub storage_root: String,
    pub scratch_root: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_root: "/tmp/af/storage".to_string(),
            scratch_root: "/tmp/af/scratch".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub cors_origin: Option<String>,
    pub rate_limit: u32,
    pub upload_max_bytes: u64,
    pub max_stream_duration_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".to_string(),
            cors_origin: None,
            rate_limit: 60,
            upload_max_bytes: 100 * 1024 * 1024, // 100 MB
            max_stream_duration_secs: 1800,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// Fraction of context_window at which to trigger compaction (e.g. 0.85).
    pub threshold: f32,
    /// LLM route to use for summarization (e.g. "openai:gpt-4o-mini").
    /// None = use the agent's own backend.
    pub summarization_route: Option<String>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: 0.85,
            summarization_route: None,
        }
    }
}

/// Resolve the config file path: `AF_CONFIG_PATH` env or `~/.af/config.toml`.
pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("AF_CONFIG_PATH") {
        return PathBuf::from(path);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".af").join("config.toml");
    }
    PathBuf::from(".af/config.toml")
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# Arbeiterfarm configuration file
# Environment variables override values set here.

# [database]
# url = "postgres://af:af@localhost/af"
# pool_size = 10

# [storage]
# storage_root = "/tmp/af/storage"
# scratch_root = "/tmp/af/scratch"

# [server]
# bind_addr = "127.0.0.1:8080"
# cors_origin = "*"
# rate_limit = 60
# upload_max_bytes = 104857600
# max_stream_duration_secs = 1800

# [compaction]
# threshold = 0.85
# summarization_route = "openai:gpt-4o-mini"

# Local model cards directory (TOML files with context_window, pricing, etc.)
# models_dir = "~/.af/models"
"#;

/// Load config from `config_path()`. If the file doesn't exist, write a default
/// template with all options commented out. On parse errors, warn and return defaults.
pub fn load_or_create_default() -> ArbeiterfarmConfig {
    let path = config_path();

    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<ArbeiterfarmConfig>(&contents) {
                Ok(config) => return config,
                Err(e) => {
                    eprintln!(
                        "[af] WARNING: failed to parse {}: {e} — using defaults",
                        path.display()
                    );
                    return ArbeiterfarmConfig::default();
                }
            },
            Err(e) => {
                eprintln!(
                    "[af] WARNING: failed to read {}: {e} — using defaults",
                    path.display()
                );
                return ArbeiterfarmConfig::default();
            }
        }
    }

    // File doesn't exist — try to create it with the default template
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE) {
        Ok(()) => {
            // Set restrictive permissions (0600) since config may contain DB credentials
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            eprintln!("[af] Created default config: {}", path.display());
        }
        Err(e) => {
            eprintln!(
                "[af] WARNING: could not write default config to {}: {e}",
                path.display()
            );
        }
    }

    ArbeiterfarmConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_uses_defaults() {
        let config: ArbeiterfarmConfig = toml::from_str("").unwrap();
        assert_eq!(config.database.url, "postgres://af:af@localhost/af");
        assert_eq!(config.database.pool_size, 10);
        assert_eq!(config.storage.storage_root, "/tmp/af/storage");
        assert_eq!(config.compaction.threshold, 0.85);
        assert!(config.compaction.summarization_route.is_none());
    }

    #[test]
    fn test_partial_config() {
        let config: ArbeiterfarmConfig = toml::from_str(
            r#"
            [compaction]
            summarization_route = "openai:gpt-4o-mini"
            "#,
        )
        .unwrap();
        assert_eq!(config.database.url, "postgres://af:af@localhost/af");
        assert_eq!(
            config.compaction.summarization_route.as_deref(),
            Some("openai:gpt-4o-mini")
        );
        assert_eq!(config.compaction.threshold, 0.85);
    }

    #[test]
    fn test_full_config() {
        let config: ArbeiterfarmConfig = toml::from_str(
            r#"
            [database]
            url = "postgres://test:test@db/test"
            pool_size = 20

            [storage]
            storage_root = "/data/storage"
            scratch_root = "/data/scratch"

            [server]
            bind_addr = "0.0.0.0:9090"
            cors_origin = "*"
            rate_limit = 120
            upload_max_bytes = 52428800
            max_stream_duration_secs = 3600

            [compaction]
            threshold = 0.90
            summarization_route = "anthropic:claude-haiku-4-5-20251001"
            "#,
        )
        .unwrap();
        assert_eq!(config.database.url, "postgres://test:test@db/test");
        assert_eq!(config.database.pool_size, 20);
        assert_eq!(config.storage.storage_root, "/data/storage");
        assert_eq!(config.server.bind_addr, "0.0.0.0:9090");
        assert_eq!(config.server.rate_limit, 120);
        assert_eq!(config.compaction.threshold, 0.90);
        assert_eq!(
            config.compaction.summarization_route.as_deref(),
            Some("anthropic:claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn test_default_template_is_valid() {
        // The default template has everything commented out — should parse as empty/defaults
        let config: ArbeiterfarmConfig = toml::from_str(DEFAULT_CONFIG_TEMPLATE).unwrap();
        assert_eq!(config.database.url, "postgres://af:af@localhost/af");
    }
}
