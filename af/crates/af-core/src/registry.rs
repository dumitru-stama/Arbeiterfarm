use crate::error::RegistryError;
use crate::executor::ToolExecutor;
use crate::types::{ExecutorEntry, SpawnConfig, ToolSpec};
use std::collections::{BTreeMap, HashMap};

/// Pure data registry of tool specifications.
/// Keyed by (name, version) to support multiple versions.
pub struct ToolSpecRegistry {
    specs: HashMap<String, BTreeMap<u32, ToolSpec>>,
}

impl ToolSpecRegistry {
    pub fn new() -> Self {
        Self {
            specs: HashMap::new(),
        }
    }

    pub fn register(&mut self, spec: ToolSpec) -> Result<(), RegistryError> {
        let versions = self.specs.entry(spec.name.clone()).or_default();
        if versions.contains_key(&spec.version) {
            return Err(RegistryError::Duplicate {
                name: spec.name.clone(),
                version: spec.version,
            });
        }
        versions.insert(spec.version, spec);
        Ok(())
    }

    pub fn get_latest(&self, name: &str) -> Option<&ToolSpec> {
        self.specs.get(name)?.values().next_back()
    }

    pub fn get(&self, name: &str, version: u32) -> Option<&ToolSpec> {
        self.specs.get(name)?.get(&version)
    }

    pub fn list(&self) -> Vec<&str> {
        self.specs.keys().map(|s| s.as_str()).collect()
    }

    pub fn versions(&self, name: &str) -> Vec<u32> {
        self.specs
            .get(name)
            .map(|v| v.keys().copied().collect())
            .unwrap_or_default()
    }
}

/// Pure data registry of tool executors.
pub struct ToolExecutorRegistry {
    entries: HashMap<String, BTreeMap<u32, ExecutorEntry>>,
}

impl ToolExecutorRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn register(&mut self, executor: Box<dyn ToolExecutor>) -> Result<(), RegistryError> {
        let name = executor.tool_name().to_string();
        let version = executor.tool_version();
        let versions = self.entries.entry(name.clone()).or_default();
        if versions.contains_key(&version) {
            return Err(RegistryError::Duplicate { name, version });
        }
        versions.insert(version, ExecutorEntry::InProcess(executor));
        Ok(())
    }

    pub fn register_oop(&mut self, config: SpawnConfig) -> Result<(), RegistryError> {
        for (name, version) in &config.supported_tools {
            let versions = self.entries.entry(name.clone()).or_default();
            if versions.contains_key(version) {
                return Err(RegistryError::Duplicate {
                    name: name.clone(),
                    version: *version,
                });
            }
            versions.insert(*version, ExecutorEntry::OutOfProcess(config.clone()));
        }
        Ok(())
    }

    pub fn get(&self, name: &str, version: u32) -> Option<&ExecutorEntry> {
        self.entries.get(name)?.get(&version)
    }

    pub fn get_latest(&self, name: &str) -> Option<&ExecutorEntry> {
        self.entries.get(name)?.values().next_back()
    }

    pub fn list(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }
}

/// Validate both registries together at startup.
pub fn validate_registries(
    specs: &ToolSpecRegistry,
    executors: &ToolExecutorRegistry,
) -> Result<(), Vec<RegistryError>> {
    let mut errors = Vec::new();

    // Every executor must have a matching spec
    for name in executors.list() {
        if let Some(versions) = executors.entries.get(name) {
            for &version in versions.keys() {
                if specs.get(name, version).is_none() {
                    errors.push(RegistryError::ExecutorWithoutSpec {
                        name: name.to_string(),
                        version,
                    });
                }
            }
        }
    }

    // Every spec's latest version should have an executor.
    // Skip `internal.*` tools — they are conditionally wired (need DB + LLM router)
    // and may not have executors in CLI-only mode.
    // Skip `tools.discover` — intercepted in AgentRuntime, no executor needed.
    for name in specs.list() {
        if name.starts_with("internal.") || name == "tools.discover" {
            continue;
        }
        if let Some(spec) = specs.get_latest(name) {
            if executors.get(name, spec.version).is_none() {
                errors.push(RegistryError::SpecWithoutExecutor {
                    name: name.to_string(),
                    version: spec.version,
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
