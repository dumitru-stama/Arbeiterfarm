use af_llm::model_catalog::ModelSpec;
use std::path::{Path, PathBuf};

/// TOML file wrapper: `[model]` section.
#[derive(serde::Deserialize)]
struct TomlModelFile {
    model: TomlModelDef,
}

/// Deserialization target for a single model card.
#[derive(serde::Deserialize)]
struct TomlModelDef {
    name: String,
    context_window: u32,
    max_output_tokens: u32,
    #[serde(default)]
    cost_per_mtok_input: f64,
    #[serde(default)]
    cost_per_mtok_output: f64,
    #[serde(default)]
    cost_per_mtok_cached_input: Option<f64>,
    #[serde(default)]
    cost_per_mtok_cache_creation: Option<f64>,
    #[serde(default)]
    supports_vision: bool,
    #[serde(default)]
    knowledge_cutoff: Option<String>,
    /// Fixed temperature (shorthand for `temperature_range = [t, t]`).
    #[serde(default)]
    temperature: Option<f32>,
    /// Allowed temperature range `[min, max]`. Overrides `temperature` if both are set.
    #[serde(default)]
    temperature_range: Option<[f32; 2]>,
    /// Whether the model supports native tool/function calling.
    /// Defaults to None (use backend default). Set to `false` for models like gemma3.
    #[serde(default)]
    supports_tool_calls: Option<bool>,
}

/// Successfully parsed local model definition.
#[derive(Debug)]
pub struct LocalModelDef {
    pub name: String,
    pub spec: ModelSpec,
    pub source_file: PathBuf,
}

/// Errors that can occur when loading a local model TOML file.
#[derive(Debug)]
pub enum LocalModelError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    Validation(PathBuf, String),
}

impl std::fmt::Display for LocalModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "local model {}: I/O error: {e}", path.display()),
            Self::Parse(path, e) => write!(f, "local model {}: parse error: {e}", path.display()),
            Self::Validation(path, msg) => {
                write!(f, "local model {}: {msg}", path.display())
            }
        }
    }
}

/// Default models directory: `~/.af/models/`, overridable via `AF_MODELS_DIR`.
pub fn default_models_dir() -> PathBuf {
    std::env::var("AF_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".af")
                .join("models")
        })
}

/// Scan a directory for `*.toml` files and attempt to load each as a model card.
/// Returns one Result per file found.
pub fn load_local_models(dir: &Path) -> Vec<Result<LocalModelDef, LocalModelError>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return vec![];
            }
            return vec![Err(LocalModelError::Io(dir.to_path_buf(), e))];
        }
    };

    let mut results = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                results.push(Err(LocalModelError::Io(dir.to_path_buf(), e)));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            results.push(load_single_model(&path));
        }
    }
    results
}

fn load_single_model(path: &Path) -> Result<LocalModelDef, LocalModelError> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| LocalModelError::Io(path.to_path_buf(), e))?;
    let file: TomlModelFile =
        toml::from_str(&contents).map_err(|e| LocalModelError::Parse(path.to_path_buf(), e))?;
    let def = file.model;

    // Validation
    if def.name.is_empty() {
        return Err(LocalModelError::Validation(
            path.to_path_buf(),
            "model name cannot be empty".into(),
        ));
    }
    if def.context_window == 0 {
        return Err(LocalModelError::Validation(
            path.to_path_buf(),
            "context_window must be > 0".into(),
        ));
    }
    if def.max_output_tokens == 0 {
        return Err(LocalModelError::Validation(
            path.to_path_buf(),
            "max_output_tokens must be > 0".into(),
        ));
    }

    // Leak knowledge_cutoff string to get &'static str (small one-time leak at startup)
    let knowledge_cutoff: Option<&'static str> = def
        .knowledge_cutoff
        .map(|s| &*Box::leak(s.into_boxed_str()));

    // temperature_range takes precedence over temperature (fixed value)
    let temperature_range = if let Some([min, max]) = def.temperature_range {
        Some((min, max))
    } else {
        def.temperature.map(|t| (t, t))
    };

    let spec = ModelSpec {
        context_window: def.context_window,
        max_output_tokens: def.max_output_tokens,
        cost_per_mtok_input: def.cost_per_mtok_input,
        cost_per_mtok_output: def.cost_per_mtok_output,
        cost_per_mtok_cached_input: def.cost_per_mtok_cached_input,
        cost_per_mtok_cache_creation: def.cost_per_mtok_cache_creation,
        supports_vision: def.supports_vision,
        knowledge_cutoff,
        temperature_range,
        supports_tool_calls: def.supports_tool_calls,
    };

    Ok(LocalModelDef {
        name: def.name,
        spec,
        source_file: path.to_path_buf(),
    })
}

/// Load and register all local TOML model cards into the model catalog.
///
/// Pass `None` for `dir` to use the default (`~/.af/models/` or `AF_MODELS_DIR`).
/// Returns the list of successfully registered model names.
pub fn register_local_models(dir: Option<&Path>) -> Vec<String> {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => default_models_dir(),
    };

    let mut registered = Vec::new();
    let mut specs = Vec::new();

    for result in load_local_models(&dir) {
        match result {
            Ok(def) => {
                eprintln!(
                    "[af] Local model '{}' loaded from {} (context={}K, max_output={})",
                    def.name,
                    def.source_file.display(),
                    def.spec.context_window / 1024,
                    def.spec.max_output_tokens,
                );
                registered.push(def.name.clone());
                specs.push((def.name, def.spec));
            }
            Err(e) => eprintln!("[af] WARNING: {e}"),
        }
    }

    if !specs.is_empty() {
        af_llm::model_catalog::init_local_models(specs);
        eprintln!(
            "[af] {} local model(s) loaded from {}",
            registered.len(),
            dir.display()
        );
    }

    registered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_toml(dir: &Path, filename: &str, contents: &str) -> PathBuf {
        let path = dir.join(filename);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_load_nonexistent_dir() {
        let results = load_local_models(Path::new("/nonexistent/dir/for/af/models"));
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_empty_dir() {
        let dir = TempDir::new().unwrap();
        let results = load_local_models(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_valid_model() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[model]
name = "deepseek-r1"
context_window = 65536
max_output_tokens = 8192
cost_per_mtok_input = 0.55
cost_per_mtok_output = 2.19
supports_vision = false
knowledge_cutoff = "2025-01"
"#;
        write_toml(dir.path(), "deepseek-r1.toml", toml_content);
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 1);

        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.name, "deepseek-r1");
        assert_eq!(def.spec.context_window, 65_536);
        assert_eq!(def.spec.max_output_tokens, 8_192);
        assert_eq!(def.spec.cost_per_mtok_input, 0.55);
        assert_eq!(def.spec.cost_per_mtok_output, 2.19);
        assert!(!def.spec.supports_vision);
        assert_eq!(def.spec.knowledge_cutoff, Some("2025-01"));
    }

    #[test]
    fn test_load_minimal_model() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[model]
name = "local-llama"
context_window = 4096
max_output_tokens = 2048
"#;
        write_toml(dir.path(), "llama.toml", toml_content);
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 1);

        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.name, "local-llama");
        assert_eq!(def.spec.context_window, 4096);
        assert_eq!(def.spec.max_output_tokens, 2048);
        assert_eq!(def.spec.cost_per_mtok_input, 0.0);
        assert_eq!(def.spec.cost_per_mtok_output, 0.0);
        assert_eq!(def.spec.cost_per_mtok_cached_input, None);
        assert_eq!(def.spec.cost_per_mtok_cache_creation, None);
        assert!(!def.spec.supports_vision);
        assert_eq!(def.spec.knowledge_cutoff, None);
    }

    #[test]
    fn test_load_invalid_empty_name() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[model]
name = ""
context_window = 4096
max_output_tokens = 2048
"#;
        write_toml(dir.path(), "bad.toml", toml_content);
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalModelError::Validation(_, _)));
    }

    #[test]
    fn test_load_invalid_zero_context() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[model]
name = "bad-model"
context_window = 0
max_output_tokens = 2048
"#;
        write_toml(dir.path(), "bad.toml", toml_content);
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalModelError::Validation(_, _)));
    }

    #[test]
    fn test_load_invalid_zero_output() {
        let dir = TempDir::new().unwrap();
        let toml_content = r#"
[model]
name = "bad-model"
context_window = 4096
max_output_tokens = 0
"#;
        write_toml(dir.path(), "bad.toml", toml_content);
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    #[test]
    fn test_load_invalid_toml() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "bad.toml", "not valid {{ toml");
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalModelError::Parse(_, _)));
    }

    #[test]
    fn test_skips_non_toml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a model").unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();
        let results = load_local_models(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_multiple_models() {
        let dir = TempDir::new().unwrap();
        write_toml(
            dir.path(),
            "model-a.toml",
            r#"
[model]
name = "model-a"
context_window = 8192
max_output_tokens = 4096
"#,
        );
        write_toml(
            dir.path(),
            "model-b.toml",
            r#"
[model]
name = "model-b"
context_window = 32768
max_output_tokens = 8192
supports_vision = true
"#,
        );
        let results = load_local_models(dir.path());
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));

        let names: Vec<String> = results
            .into_iter()
            .map(|r| r.unwrap().name)
            .collect();
        assert!(names.contains(&"model-a".to_string()));
        assert!(names.contains(&"model-b".to_string()));
    }

    #[test]
    fn test_display_errors() {
        let io_err = LocalModelError::Io(
            PathBuf::from("/tmp/test.toml"),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        let s = format!("{io_err}");
        assert!(s.contains("I/O error"));

        let val_err = LocalModelError::Validation(
            PathBuf::from("/tmp/test.toml"),
            "name cannot be empty".into(),
        );
        let s = format!("{val_err}");
        assert!(s.contains("name cannot be empty"));
    }
}
