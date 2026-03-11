use af_core::{
    OutputRedirectPolicy, SandboxProfile, SpawnConfig, ToolExecutorRegistry, ToolPolicy,
    ToolSpec, ToolSpecRegistry,
};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Parsed result from a TOML file: a ToolSpec + SpawnConfig ready for registration.
#[derive(Debug)]
pub struct LocalToolDef {
    pub spec: ToolSpec,
    pub spawn_config: SpawnConfig,
    pub source_file: PathBuf,
}

/// Errors that can occur when loading a local tool TOML file.
#[derive(Debug)]
pub enum LocalToolError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    Validation(PathBuf, String),
}

impl std::fmt::Display for LocalToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "local tool {}: I/O error: {e}", path.display()),
            Self::Parse(path, e) => write!(f, "local tool {}: parse error: {e}", path.display()),
            Self::Validation(path, msg) => {
                write!(f, "local tool {}: {msg}", path.display())
            }
        }
    }
}

/// TOML schema for a local tool definition.
#[derive(Debug, Deserialize)]
struct LocalToolToml {
    tool: ToolSection,
}

#[derive(Debug, Deserialize)]
struct ToolSection {
    name: String,
    version: u32,
    binary: String,
    protocol: String,
    description: String,
    usage: Option<String>,
    good_for: Option<Vec<String>>,
    output_redirect: Option<String>,
    input_schema: Option<toml::Value>,
    policy: Option<PolicySection>,
    context_extra: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
struct PolicySection {
    sandbox: Option<String>,
    timeout_ms: Option<u64>,
    max_input_bytes: Option<u64>,
    max_output_bytes: Option<u64>,
    max_produced_artifacts: Option<u32>,
    uds_bind_mounts: Option<Vec<String>>,
    writable_bind_mounts: Option<Vec<String>>,
    extra_ro_bind_mounts: Option<Vec<String>>,
}

/// Scan a directory for `*.toml` files and attempt to load each as a local tool.
/// Returns one Result per file found.
pub fn load_local_tools(dir: &Path) -> Vec<Result<LocalToolDef, LocalToolError>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return vec![];
            }
            return vec![Err(LocalToolError::Io(dir.to_path_buf(), e))];
        }
    };

    let mut results = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                results.push(Err(LocalToolError::Io(dir.to_path_buf(), e)));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            results.push(load_single_tool(&path));
        }
    }
    results
}

/// Default tools directory: `~/.af/tools/`, overridable via `AF_TOOLS_DIR`.
pub fn default_tools_dir() -> PathBuf {
    std::env::var("AF_TOOLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".af")
                .join("tools")
        })
}

/// Load and register all local TOML tools into the given registries.
///
/// Pass `None` for `tools_dir` to use the default (`~/.af/tools/` or `AF_TOOLS_DIR`).
/// Returns the list of successfully registered tool names.
///
/// Warnings for invalid files are printed to stderr. Valid tools still load
/// even if some files fail.
pub fn register_local_tools(
    specs: &mut ToolSpecRegistry,
    executors: &mut ToolExecutorRegistry,
    tools_dir: Option<&Path>,
) -> Vec<String> {
    let dir = match tools_dir {
        Some(d) => d.to_path_buf(),
        None => default_tools_dir(),
    };

    let mut registered = Vec::new();

    for result in load_local_tools(&dir) {
        match result {
            Ok(def) => {
                let name = def.spec.name.clone();
                if let Err(e) = specs.register(def.spec) {
                    eprintln!("[af] WARNING: local tool '{name}': {e}");
                    continue;
                }
                if let Err(e) = executors.register_oop(def.spawn_config) {
                    eprintln!("[af] WARNING: local tool '{name}': {e}");
                    continue;
                }
                eprintln!(
                    "[af] Local tool '{name}' loaded from {}",
                    def.source_file.display()
                );
                registered.push(name);
            }
            Err(e) => eprintln!("[af] WARNING: {e}"),
        }
    }

    if !registered.is_empty() {
        eprintln!(
            "[af] {} local tool(s) loaded from {}",
            registered.len(),
            dir.display()
        );
    }

    registered
}

fn load_single_tool(path: &Path) -> Result<LocalToolDef, LocalToolError> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| LocalToolError::Io(path.to_path_buf(), e))?;
    let toml_doc: LocalToolToml =
        toml::from_str(&contents).map_err(|e| LocalToolError::Parse(path.to_path_buf(), e))?;
    let tool = toml_doc.tool;

    // --- Validation ---
    validate_name(&tool.name, path)?;

    if tool.version < 1 {
        return Err(LocalToolError::Validation(
            path.to_path_buf(),
            "version must be >= 1".into(),
        ));
    }

    let binary_path = PathBuf::from(&tool.binary);
    if !binary_path.is_absolute() {
        return Err(LocalToolError::Validation(
            path.to_path_buf(),
            format!("binary path must be absolute: {}", tool.binary),
        ));
    }
    if !binary_path.exists() {
        return Err(LocalToolError::Validation(
            path.to_path_buf(),
            format!("binary not found: {}", tool.binary),
        ));
    }

    let protocol = match tool.protocol.as_str() {
        "simple" | "oop" => tool.protocol.as_str(),
        other => {
            return Err(LocalToolError::Validation(
                path.to_path_buf(),
                format!("protocol must be \"simple\" or \"oop\", got \"{other}\""),
            ))
        }
    };

    // --- Build description ---
    let description = build_description(&tool.description, tool.usage.as_deref(), &tool.good_for);

    // --- Parse input_schema ---
    let mut input_schema = match &tool.input_schema {
        Some(v) => toml_to_json(v),
        None => json!({"type": "object"}),
    };

    // Auto-inject $defs.ArtifactId if schema references it
    inject_artifact_defs(&mut input_schema);

    // --- Parse policy ---
    let policy = build_policy(tool.policy.as_ref(), path)?;

    // --- Parse output_redirect ---
    let output_redirect = match tool.output_redirect.as_deref() {
        None | Some("Allowed") => OutputRedirectPolicy::Allowed,
        Some("Forbidden") => OutputRedirectPolicy::Forbidden,
        Some(other) => {
            return Err(LocalToolError::Validation(
                path.to_path_buf(),
                format!("output_redirect must be \"Allowed\" or \"Forbidden\", got \"{other}\""),
            ))
        }
    };

    // --- Build ToolSpec ---
    let spec = ToolSpec {
        name: tool.name.clone(),
        version: tool.version,
        deprecated: false,
        description,
        input_schema: input_schema.clone(),
        policy: policy.clone(),
        output_redirect,
    };

    // --- Build SpawnConfig ---
    let context_extra = match protocol {
        "simple" => {
            // Pre-compute artifact schema paths for runtime replacement
            let artifact_paths = af_core::resolve_schema_paths(&input_schema);
            let mut extra = json!({
                "protocol": "simple",
                "artifact_schema_paths": artifact_paths,
            });
            // Merge user-provided context_extra
            if let Some(user_extra) = &tool.context_extra {
                let user_json = toml_to_json(user_extra);
                if let (Some(base), Some(user)) =
                    (extra.as_object_mut(), user_json.as_object())
                {
                    for (k, v) in user {
                        base.insert(k.clone(), v.clone());
                    }
                }
            }
            extra
        }
        "oop" => match &tool.context_extra {
            Some(v) => toml_to_json(v),
            None => serde_json::Value::Null,
        },
        _ => unreachable!(),
    };

    let spawn_config = SpawnConfig {
        binary_path,
        protocol_version: 1,
        supported_tools: vec![(tool.name.clone(), tool.version)],
        context_extra,
    };

    Ok(LocalToolDef {
        spec,
        spawn_config,
        source_file: path.to_path_buf(),
    })
}

/// Validate tool name: dotted lowercase, e.g. "custom.hash", "my.tool.v2"
fn validate_name(name: &str, path: &Path) -> Result<(), LocalToolError> {
    // Must have at least one dot
    if !name.contains('.') {
        return Err(LocalToolError::Validation(
            path.to_path_buf(),
            format!("tool name must contain at least one dot (e.g. \"custom.hash\"): \"{name}\""),
        ));
    }

    // Each segment: [a-z][a-z0-9_]*
    for segment in name.split('.') {
        if segment.is_empty() {
            return Err(LocalToolError::Validation(
                path.to_path_buf(),
                format!("tool name has empty segment: \"{name}\""),
            ));
        }
        let first = segment.chars().next().unwrap();
        if !first.is_ascii_lowercase() {
            return Err(LocalToolError::Validation(
                path.to_path_buf(),
                format!(
                    "each segment must start with a-z: \"{name}\" (segment \"{segment}\")"
                ),
            ));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(LocalToolError::Validation(
                path.to_path_buf(),
                format!(
                    "segments must contain only a-z, 0-9, _ : \"{name}\" (segment \"{segment}\")"
                ),
            ));
        }
    }
    Ok(())
}

/// Merge description + usage + good_for into a single description string.
fn build_description(
    description: &str,
    usage: Option<&str>,
    good_for: &Option<Vec<String>>,
) -> String {
    let mut result = description.to_string();
    if let Some(usage) = usage {
        result.push_str("\n\nUsage: ");
        result.push_str(usage);
    }
    if let Some(items) = good_for {
        if !items.is_empty() {
            result.push_str("\n\nGood for: ");
            result.push_str(&items.join(", "));
        }
    }
    result
}

/// If the input_schema contains `$ref: "#/$defs/ArtifactId"` references but
/// no `$defs.ArtifactId` definition, inject the standard definition.
fn inject_artifact_defs(schema: &mut serde_json::Value) {
    let has_ref = schema_references_artifact(schema);
    if !has_ref {
        return;
    }

    let defs = schema
        .as_object_mut()
        .unwrap()
        .entry("$defs")
        .or_insert_with(|| json!({}));

    if let Some(obj) = defs.as_object_mut() {
        obj.entry("ArtifactId").or_insert_with(|| {
            json!({
                "type": "string",
                "format": "uuid",
                "description": "UUID of an artifact in the current project"
            })
        });
        obj.entry("ArtifactIds").or_insert_with(|| {
            json!({
                "type": "array",
                "items": {
                    "type": "string",
                    "format": "uuid"
                },
                "description": "Array of artifact UUIDs"
            })
        });
    }
}

fn schema_references_artifact(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => {
            s == "#/$defs/ArtifactId" || s == "#/$defs/ArtifactIds"
        }
        serde_json::Value::Object(map) => {
            // Check $ref directly
            if let Some(ref_val) = map.get("$ref").and_then(|v| v.as_str()) {
                if ref_val == "#/$defs/ArtifactId" || ref_val == "#/$defs/ArtifactIds" {
                    return true;
                }
            }
            map.values().any(schema_references_artifact)
        }
        serde_json::Value::Array(arr) => arr.iter().any(schema_references_artifact),
        _ => false,
    }
}

fn parse_sandbox_profile(s: &str, path: &Path) -> Result<SandboxProfile, LocalToolError> {
    match s {
        "Trusted" => Ok(SandboxProfile::Trusted),
        "NoNetReadOnly" => Ok(SandboxProfile::NoNetReadOnly),
        "NoNetReadOnlyTmpfs" => Ok(SandboxProfile::NoNetReadOnlyTmpfs),
        "PrivateLoopback" => Ok(SandboxProfile::PrivateLoopback),
        "NetEgressAllowlist" => Ok(SandboxProfile::NetEgressAllowlist),
        other => Err(LocalToolError::Validation(
            path.to_path_buf(),
            format!("unknown sandbox profile: \"{other}\""),
        )),
    }
}

fn build_policy(
    section: Option<&PolicySection>,
    path: &Path,
) -> Result<ToolPolicy, LocalToolError> {
    let mut policy = ToolPolicy::default();

    if let Some(s) = section {
        if let Some(ref sandbox) = s.sandbox {
            policy.sandbox = parse_sandbox_profile(sandbox, path)?;
        }
        if let Some(timeout) = s.timeout_ms {
            policy.timeout_ms = timeout;
        }
        if let Some(v) = s.max_input_bytes {
            policy.max_input_bytes = v;
        }
        if let Some(v) = s.max_output_bytes {
            policy.max_output_bytes = v;
        }
        if let Some(v) = s.max_produced_artifacts {
            policy.max_produced_artifacts = v;
        }
        if let Some(ref paths) = s.uds_bind_mounts {
            policy.uds_bind_mounts = paths.iter().map(PathBuf::from).collect();
        }
        if let Some(ref paths) = s.writable_bind_mounts {
            policy.writable_bind_mounts = paths.iter().map(PathBuf::from).collect();
        }
        if let Some(ref paths) = s.extra_ro_bind_mounts {
            policy.extra_ro_bind_mounts = paths.iter().map(PathBuf::from).collect();
        }
    }

    Ok(policy)
}

/// Convert a TOML value to a serde_json::Value.
fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => json!(*i),
        toml::Value::Float(f) => json!(*f),
        toml::Value::Boolean(b) => json!(*b),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => {
            let map: serde_json::Map<String, serde_json::Value> = table
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

/// Check if a tool name belongs to a locally-loaded tool.
/// Returns true if the spec's name matches any loaded local tool.
pub fn is_local_tool(name: &str, local_names: &[String]) -> bool {
    local_names.contains(&name.to_string())
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
    fn test_validate_name_valid() {
        let p = PathBuf::from("/test");
        assert!(validate_name("custom.hash", &p).is_ok());
        assert!(validate_name("my.tool.v2", &p).is_ok());
        assert!(validate_name("re.deep_scan", &p).is_ok());
        assert!(validate_name("a.b", &p).is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        let p = PathBuf::from("/test");
        // No dot
        assert!(validate_name("nodot", &p).is_err());
        // Uppercase
        assert!(validate_name("Custom.hash", &p).is_err());
        // Empty segment
        assert!(validate_name("custom..hash", &p).is_err());
        // Starts with digit
        assert!(validate_name("custom.1hash", &p).is_err());
        // Special chars
        assert!(validate_name("custom.ha-sh", &p).is_err());
    }

    #[test]
    fn test_build_description_basic() {
        let d = build_description("Hello", None, &None);
        assert_eq!(d, "Hello");
    }

    #[test]
    fn test_build_description_full() {
        let d = build_description(
            "Compute hashes",
            Some("Pass an artifact_id"),
            &Some(vec!["File ID".into(), "Tracking".into()]),
        );
        assert_eq!(
            d,
            "Compute hashes\n\nUsage: Pass an artifact_id\n\nGood for: File ID, Tracking"
        );
    }

    #[test]
    fn test_inject_artifact_defs_with_ref() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            }
        });
        inject_artifact_defs(&mut schema);
        assert!(schema["$defs"]["ArtifactId"].is_object());
        assert_eq!(schema["$defs"]["ArtifactId"]["type"], "string");
    }

    #[test]
    fn test_inject_artifact_defs_no_ref() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        inject_artifact_defs(&mut schema);
        assert!(schema.get("$defs").is_none());
    }

    #[test]
    fn test_inject_artifact_defs_preserves_existing() {
        let mut schema = json!({
            "type": "object",
            "$defs": {
                "ArtifactId": {
                    "type": "string",
                    "format": "uuid",
                    "description": "custom"
                }
            },
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" }
            }
        });
        inject_artifact_defs(&mut schema);
        // Should not overwrite the custom description
        assert_eq!(schema["$defs"]["ArtifactId"]["description"], "custom");
    }

    #[test]
    fn test_toml_to_json() {
        let toml_str = concat!(
            "type = \"object\"\n",
            "required = [\"artifact_id\"]\n",
            "[properties.artifact_id]\n",
            "\"$ref\" = \"#/$defs/ArtifactId\"\n",
        );
        let toml_val: toml::Value = toml::from_str(toml_str).unwrap();
        let json_val = toml_to_json(&toml_val);
        assert_eq!(json_val["type"], "object");
        assert_eq!(
            json_val["properties"]["artifact_id"]["$ref"],
            "#/$defs/ArtifactId"
        );
    }

    #[test]
    fn test_load_nonexistent_dir() {
        let results = load_local_tools(Path::new("/nonexistent/dir/for/af"));
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_empty_dir() {
        let dir = TempDir::new().unwrap();
        let results = load_local_tools(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_invalid_toml() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "bad.toml", "not valid {{ toml");
        let results = load_local_tools(dir.path());
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalToolError::Parse(_, _)));
    }

    #[test]
    fn test_load_valid_simple_tool() {
        let dir = TempDir::new().unwrap();

        // Create a fake binary
        let bin_path = dir.path().join("my-tool");
        std::fs::write(&bin_path, "#!/bin/sh\necho '{}'").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let toml_content = format!(
            r##"
[tool]
name = "custom.hash"
version = 1
binary = "{}"
protocol = "simple"
description = "Compute hashes"
usage = "Pass an artifact_id"
good_for = ["File ID", "Tracking"]

[tool.input_schema]
type = "object"
required = ["artifact_id"]

[tool.input_schema.properties.artifact_id]
"$ref" = "#/$defs/ArtifactId"

[tool.policy]
sandbox = "NoNetReadOnly"
timeout_ms = 30000
"##,
            bin_path.display()
        );

        write_toml(dir.path(), "hash.toml", &toml_content);
        let results = load_local_tools(dir.path());
        assert_eq!(results.len(), 1);

        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.spec.name, "custom.hash");
        assert_eq!(def.spec.version, 1);
        assert!(def.spec.description.contains("Compute hashes"));
        assert!(def.spec.description.contains("Usage: Pass an artifact_id"));
        assert!(def.spec.description.contains("Good for: File ID, Tracking"));
        assert_eq!(def.spec.policy.timeout_ms, 30000);

        // Should have auto-injected $defs
        assert!(def.spec.input_schema["$defs"]["ArtifactId"].is_object());

        // SpawnConfig should have simple protocol marker
        assert_eq!(def.spawn_config.context_extra["protocol"], "simple");
        assert!(def.spawn_config.context_extra["artifact_schema_paths"].is_array());
        assert_eq!(
            def.spawn_config.context_extra["artifact_schema_paths"][0],
            "/artifact_id"
        );
        assert_eq!(def.spawn_config.supported_tools, vec![("custom.hash".into(), 1)]);
    }

    #[test]
    fn test_load_oop_tool() {
        let dir = TempDir::new().unwrap();

        let bin_path = dir.path().join("oop-tool");
        std::fs::write(&bin_path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let toml_content = format!(
            r#"
[tool]
name = "custom.oop"
version = 1
binary = "{}"
protocol = "oop"
description = "OOP tool"

[tool.context_extra]
my_key = "my_value"
"#,
            bin_path.display()
        );

        write_toml(dir.path(), "oop.toml", &toml_content);
        let results = load_local_tools(dir.path());
        let def = results.into_iter().next().unwrap().unwrap();
        assert_eq!(def.spawn_config.context_extra["my_key"], "my_value");
        // Should NOT have protocol marker
        assert!(def.spawn_config.context_extra.get("protocol").is_none());
    }

    #[test]
    fn test_validation_errors() {
        let dir = TempDir::new().unwrap();

        // Missing binary
        let toml_content = r#"
[tool]
name = "custom.test"
version = 1
binary = "/nonexistent/bin/test"
protocol = "simple"
description = "test"
"#;
        write_toml(dir.path(), "missing_bin.toml", toml_content);
        let results = load_local_tools(dir.path());
        let err = results[0].as_ref().unwrap_err();
        assert!(matches!(err, LocalToolError::Validation(_, _)));
    }

    #[test]
    fn test_skips_non_toml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a tool").unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();
        let results = load_local_tools(dir.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_sandbox_profiles() {
        let p = PathBuf::from("/test");
        assert!(matches!(
            parse_sandbox_profile("Trusted", &p).unwrap(),
            SandboxProfile::Trusted
        ));
        assert!(matches!(
            parse_sandbox_profile("NoNetReadOnly", &p).unwrap(),
            SandboxProfile::NoNetReadOnly
        ));
        assert!(matches!(
            parse_sandbox_profile("PrivateLoopback", &p).unwrap(),
            SandboxProfile::PrivateLoopback
        ));
        assert!(parse_sandbox_profile("invalid", &p).is_err());
    }
}
