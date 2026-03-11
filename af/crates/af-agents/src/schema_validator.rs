use af_core::ToolSpec;
use std::collections::HashMap;
use std::sync::Mutex;

/// Cache of compiled JSON Schema validators keyed by (tool_name, tool_version).
pub struct SchemaValidatorCache {
    cache: Mutex<HashMap<(String, u32), Option<jsonschema::Validator>>>,
}

impl SchemaValidatorCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Validate tool input against the spec's input_schema.
    /// Returns Ok(()) if valid, Err(Vec<String>) with error messages if invalid.
    pub fn validate(
        &self,
        spec: &ToolSpec,
        input: &serde_json::Value,
    ) -> Result<(), Vec<String>> {
        let key = (spec.name.clone(), spec.version);

        let mut cache = self.cache.lock().unwrap();
        let validator = cache
            .entry(key)
            .or_insert_with(|| jsonschema::validator_for(&spec.input_schema).ok());

        let Some(validator) = validator else {
            // Schema couldn't be compiled — skip validation
            return Ok(());
        };

        // iter() returns an iterator of ValidationError.
        // Include the instance path so the model knows WHICH field failed.
        let errors: Vec<String> = validator
            .iter_errors(input)
            .map(|e| {
                let path = e.instance_path.to_string();
                if path.is_empty() || path == "/" {
                    format!("{}", e)
                } else {
                    // path is like "/line_count" — strip leading slash for readability
                    let field = path.trim_start_matches('/');
                    format!("'{}': {}", field, e)
                }
            })
            .collect();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
