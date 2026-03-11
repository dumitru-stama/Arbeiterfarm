use af_plugin_api::{PluginDb, PluginDbError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// A single rename entry from the database.
#[derive(Debug, Clone)]
pub struct RenameEntry {
    pub old_name: String,
    pub new_name: String,
    pub address: Option<String>,
}

/// A cross-project rename suggestion, deduplicated across projects.
#[derive(Debug, Clone)]
pub struct CrossRenameEntry {
    pub old_name: String,
    pub new_name: String,
    pub address: Option<String>,
    /// Comma-separated names of projects that have this rename.
    pub project_names: String,
    /// Number of distinct projects with this rename.
    pub source_count: i64,
}

/// Bulk upsert renames for a binary in a project.
/// Uses INSERT ... ON CONFLICT UPDATE for last-write-wins semantics.
/// Sends a single SQL statement using jsonb_array_elements for efficiency.
pub async fn upsert_renames(
    db: &Arc<dyn PluginDb>,
    project_id: Uuid,
    sha256: &str,
    renames: &[(String, String, Option<String>)], // (old_name, new_name, address)
    user_id: Option<Uuid>,
) -> Result<u64, PluginDbError> {
    let renames_json: Vec<Value> = renames
        .iter()
        .map(|(old, new, addr)| {
            let mut obj = json!({ "old_name": old, "new_name": new });
            if let Some(a) = addr {
                obj["address"] = json!(a);
            }
            obj
        })
        .collect();

    let user_val = match user_id {
        Some(u) => json!(u.to_string()),
        None => Value::Null,
    };

    db.execute_json(
        "INSERT INTO ghidra_function_renames (project_id, sha256, old_name, new_name, address, renamed_by) \
         SELECT $1::uuid, $2, r->>'old_name', r->>'new_name', NULLIF(r->>'address', ''), $4::uuid \
         FROM jsonb_array_elements($3::jsonb) AS r \
         ON CONFLICT (project_id, sha256, old_name) \
         DO UPDATE SET new_name = EXCLUDED.new_name, \
                       address = COALESCE(EXCLUDED.address, ghidra_function_renames.address), \
                       renamed_by = EXCLUDED.renamed_by, \
                       updated_at = now()",
        vec![
            json!(project_id.to_string()),
            json!(sha256),
            json!(renames_json),
            user_val,
        ],
        user_id,
    )
    .await
}

/// Get all renames for a binary in a specific project.
/// Returns a HashMap<old_name, new_name> for efficient lookup.
pub async fn get_renames(
    db: &Arc<dyn PluginDb>,
    project_id: Uuid,
    sha256: &str,
) -> Result<HashMap<String, String>, PluginDbError> {
    let rows = db
        .query_json(
            "SELECT old_name, new_name FROM ghidra_function_renames \
             WHERE project_id = $1::uuid AND sha256 = $2 \
             ORDER BY updated_at DESC",
            vec![json!(project_id.to_string()), json!(sha256)],
            None,
        )
        .await?;

    let mut map = HashMap::new();
    for row in rows {
        if let (Some(old), Some(new)) = (
            row.get("old_name").and_then(|v| v.as_str()),
            row.get("new_name").and_then(|v| v.as_str()),
        ) {
            map.insert(old.to_string(), new.to_string());
        }
    }
    Ok(map)
}

/// Get all renames for a binary as RenameEntry structs.
pub async fn get_rename_entries(
    db: &Arc<dyn PluginDb>,
    project_id: Uuid,
    sha256: &str,
) -> Result<Vec<RenameEntry>, PluginDbError> {
    let rows = db
        .query_json(
            "SELECT old_name, new_name, address FROM ghidra_function_renames \
             WHERE project_id = $1::uuid AND sha256 = $2 \
             ORDER BY old_name",
            vec![json!(project_id.to_string()), json!(sha256)],
            None,
        )
        .await?;

    Ok(rows
        .iter()
        .filter_map(|row| {
            let old_name = row.get("old_name")?.as_str()?.to_string();
            let new_name = row.get("new_name")?.as_str()?.to_string();
            let address = row
                .get("address")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(RenameEntry {
                old_name,
                new_name,
                address,
            })
        })
        .collect())
}

/// Get renames from other shareable projects for a binary (cross-project suggestions).
/// Uses af_shareable_projects() to respect NDA boundaries.
/// Results are deduplicated: same (old_name, new_name) from multiple projects is aggregated.
pub async fn get_cross_project_renames(
    db: &Arc<dyn PluginDb>,
    sha256: &str,
    exclude_project_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Vec<CrossRenameEntry>, PluginDbError> {
    let rows = db
        .query_json(
            "SELECT r.old_name, r.new_name, r.address, \
                    string_agg(DISTINCT p.name, ', ' ORDER BY p.name) as project_names, \
                    COUNT(DISTINCT r.project_id) as source_count \
             FROM ghidra_function_renames r \
             JOIN projects p ON p.id = r.project_id \
             WHERE r.sha256 = $1 \
               AND r.project_id <> $2::uuid \
               AND r.project_id IN (SELECT af_shareable_projects()) \
             GROUP BY r.old_name, r.new_name, r.address \
             ORDER BY r.old_name",
            vec![json!(sha256), json!(exclude_project_id.to_string())],
            user_id,
        )
        .await?;

    Ok(rows
        .iter()
        .filter_map(|row| {
            let old_name = row.get("old_name")?.as_str()?.to_string();
            let new_name = row.get("new_name")?.as_str()?.to_string();
            let address = row
                .get("address")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let project_names = row
                .get("project_names")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let source_count = row
                .get("source_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            Some(CrossRenameEntry {
                old_name,
                new_name,
                address,
                project_names,
                source_count,
            })
        })
        .collect())
}

/// Import renames from another project into the current project.
/// Only imports from shareable projects.
pub async fn import_renames(
    db: &Arc<dyn PluginDb>,
    target_project_id: Uuid,
    source_project_id: Uuid,
    sha256: &str,
    user_id: Option<Uuid>,
) -> Result<u64, PluginDbError> {
    let user_val = match user_id {
        Some(u) => json!(u.to_string()),
        None => Value::Null,
    };
    db.execute_json(
        "INSERT INTO ghidra_function_renames (project_id, sha256, old_name, new_name, address, renamed_by) \
         SELECT $1::uuid, sha256, old_name, new_name, address, $4::uuid \
         FROM ghidra_function_renames \
         WHERE project_id = $2::uuid AND sha256 = $3 \
           AND project_id IN (SELECT af_shareable_projects()) \
         ON CONFLICT (project_id, sha256, old_name) DO NOTHING",
        vec![
            json!(target_project_id.to_string()),
            json!(source_project_id.to_string()),
            json!(sha256),
            user_val,
        ],
        user_id,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_rename_entry_struct() {
        let entry = RenameEntry {
            old_name: "FUN_00401000".to_string(),
            new_name: "parse_header".to_string(),
            address: Some("0x00401000".to_string()),
        };
        assert_eq!(entry.old_name, "FUN_00401000");
        assert_eq!(entry.new_name, "parse_header");
        assert_eq!(entry.address.as_deref(), Some("0x00401000"));
    }

    #[test]
    fn test_cross_rename_entry_struct() {
        let entry = CrossRenameEntry {
            old_name: "FUN_00401000".to_string(),
            new_name: "parse_header".to_string(),
            address: None,
            project_names: "project-a, project-b".to_string(),
            source_count: 2,
        };
        assert_eq!(entry.old_name, "FUN_00401000");
        assert_eq!(entry.project_names, "project-a, project-b");
        assert_eq!(entry.source_count, 2);
    }

    #[test]
    fn test_renames_as_hashmap() {
        let mut map = HashMap::new();
        map.insert("FUN_00401000".to_string(), "parse_header".to_string());
        map.insert("FUN_00402000".to_string(), "decode_payload".to_string());
        assert_eq!(map.get("FUN_00401000"), Some(&"parse_header".to_string()));
        assert_eq!(map.get("FUN_00402000"), Some(&"decode_payload".to_string()));
        assert_eq!(map.get("FUN_00403000"), None);
    }
}
