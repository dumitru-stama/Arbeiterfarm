use uuid::Uuid;

/// Resolve artifact ID paths from a tool's input schema.
///
/// Phase 1: Walk the schema, find all JSON paths that `$ref` to `#/$defs/ArtifactId`
/// or `#/$defs/ArtifactIds`.
///
/// Phase 2: Extract UUIDs from those paths in the actual input_json.

const MAX_DEPTH: usize = 16;

/// Walk a JSON Schema and return all JSON Pointer paths that reference ArtifactId/ArtifactIds.
pub fn resolve_schema_paths(schema: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    walk_schema(schema, "", &mut paths, 0);
    paths
}

fn walk_schema(schema: &serde_json::Value, current_path: &str, paths: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }

    // Check if this node is a $ref to ArtifactId or ArtifactIds
    if let Some(ref_val) = schema.get("$ref").and_then(|v| v.as_str()) {
        if ref_val == "#/$defs/ArtifactId" {
            paths.push(current_path.to_string());
            return;
        }
        if ref_val == "#/$defs/ArtifactIds" {
            paths.push(current_path.to_string());
            return;
        }
    }

    // If type is "object", walk properties
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (key, sub_schema) in props {
            let child_path = if current_path.is_empty() {
                format!("/{key}")
            } else {
                format!("{current_path}/{key}")
            };
            walk_schema(sub_schema, &child_path, paths, depth + 1);
        }
    }

    // If type is "array" with items, walk items
    if let Some(items) = schema.get("items") {
        walk_schema(items, current_path, paths, depth + 1);
    }
}

/// Extract artifact UUIDs from input_json using pre-computed schema paths.
pub fn extract_artifact_ids(
    input: &serde_json::Value,
    schema_paths: &[String],
) -> Result<Vec<Uuid>, String> {
    let mut ids = Vec::new();

    for path in schema_paths {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        extract_at_path(input, &parts, &mut ids)?;
    }

    Ok(ids)
}

fn extract_at_path(
    value: &serde_json::Value,
    path: &[&str],
    ids: &mut Vec<Uuid>,
) -> Result<(), String> {
    if path.is_empty() {
        // We're at the target — could be a single UUID or an array of UUIDs
        match value {
            serde_json::Value::String(s) => {
                let id = Uuid::parse_str(s)
                    .map_err(|_| format!("invalid UUID: {s}"))?;
                ids.push(id);
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    if let serde_json::Value::String(s) = item {
                        let id = Uuid::parse_str(s)
                            .map_err(|_| format!("invalid UUID: {s}"))?;
                        ids.push(id);
                    }
                }
            }
            serde_json::Value::Null => {
                // Optional field, not provided — skip
            }
            _ => {
                return Err(format!("expected string or array at path, got {}", value));
            }
        }
        return Ok(());
    }

    match value.get(path[0]) {
        Some(child) => extract_at_path(child, &path[1..], ids),
        None => Ok(()), // Missing optional field
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_resolve_simple_artifact_id() {
        let schema = json!({
            "type": "object",
            "properties": {
                "artifact_id": { "$ref": "#/$defs/ArtifactId" },
                "offset": { "type": "integer" }
            }
        });
        let paths = resolve_schema_paths(&schema);
        assert_eq!(paths, vec!["/artifact_id"]);
    }

    #[test]
    fn test_resolve_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "comparison": {
                    "type": "object",
                    "properties": {
                        "primary": { "$ref": "#/$defs/ArtifactId" },
                        "secondary": { "$ref": "#/$defs/ArtifactId" }
                    }
                }
            }
        });
        let mut paths = resolve_schema_paths(&schema);
        paths.sort();
        assert_eq!(
            paths,
            vec!["/comparison/primary", "/comparison/secondary"]
        );
    }

    #[test]
    fn test_resolve_artifact_ids_array() {
        let schema = json!({
            "type": "object",
            "properties": {
                "targets": { "$ref": "#/$defs/ArtifactIds" }
            }
        });
        let paths = resolve_schema_paths(&schema);
        assert_eq!(paths, vec!["/targets"]);
    }

    #[test]
    fn test_extract_single() {
        let id = Uuid::new_v4();
        let input = json!({ "artifact_id": id.to_string() });
        let paths = vec!["/artifact_id".to_string()];
        let extracted = extract_artifact_ids(&input, &paths).unwrap();
        assert_eq!(extracted, vec![id]);
    }

    #[test]
    fn test_extract_array() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let input = json!({ "targets": [id1.to_string(), id2.to_string()] });
        let paths = vec!["/targets".to_string()];
        let extracted = extract_artifact_ids(&input, &paths).unwrap();
        assert_eq!(extracted, vec![id1, id2]);
    }
}
