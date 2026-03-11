use af_plugin_api::EvidenceResolver;

/// Resolves RE-specific evidence references (e.g. evidence:re:ioc:<uuid>).
pub struct ReEvidenceResolver;

impl EvidenceResolver for ReEvidenceResolver {
    fn namespace(&self) -> &str {
        "re"
    }

    fn resolve(&self, kind: &str, id: &str) -> Option<String> {
        match kind {
            "ioc" => {
                if uuid::Uuid::parse_str(id).is_ok() {
                    Some(format!("IOC record {id}"))
                } else {
                    None
                }
            }
            "family" => {
                if uuid::Uuid::parse_str(id).is_ok() {
                    Some(format!("Family tag {id}"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn existence_query(&self, kind: &str) -> Option<&str> {
        match kind {
            "ioc" => Some("SELECT 1 FROM re.iocs WHERE id = $1::uuid AND project_id = $2::uuid"),
            "family" => Some("SELECT 1 FROM re.artifact_families WHERE id = $1::uuid AND project_id = $2::uuid"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_valid_ioc_uuid() {
        let r = ReEvidenceResolver;
        let id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        assert!(r.resolve("ioc", id).is_some());
    }

    #[test]
    fn test_resolve_invalid_ioc_uuid() {
        let r = ReEvidenceResolver;
        assert!(r.resolve("ioc", "not-a-uuid").is_none());
    }

    #[test]
    fn test_resolve_unknown_kind() {
        let r = ReEvidenceResolver;
        assert!(r.resolve("bogus", "a1b2c3d4-e5f6-7890-abcd-ef1234567890").is_none());
    }

    #[test]
    fn test_existence_query_schema_qualified_and_project_scoped() {
        let r = ReEvidenceResolver;
        let sql = r.existence_query("ioc").expect("ioc should have existence query");
        // Must use schema-qualified table name (not bare `iocs`)
        assert!(sql.contains("re.iocs"), "query must be schema-qualified: {sql}");
        // Must bind both record id ($1) and project_id ($2)
        assert!(sql.contains("$1"), "query must bind record id: {sql}");
        assert!(sql.contains("$2"), "query must bind project_id: {sql}");
        assert!(sql.contains("project_id"), "query must filter by project_id: {sql}");
    }

    #[test]
    fn test_resolve_valid_family_uuid() {
        let r = ReEvidenceResolver;
        let id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        let result = r.resolve("family", id);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Family tag"));
    }

    #[test]
    fn test_resolve_invalid_family_uuid() {
        let r = ReEvidenceResolver;
        assert!(r.resolve("family", "not-a-uuid").is_none());
    }

    #[test]
    fn test_existence_query_family() {
        let r = ReEvidenceResolver;
        let sql = r.existence_query("family").expect("family should have existence query");
        assert!(sql.contains("re.artifact_families"), "query must be schema-qualified: {sql}");
        assert!(sql.contains("$1"), "query must bind record id: {sql}");
        assert!(sql.contains("$2"), "query must bind project_id: {sql}");
        assert!(sql.contains("project_id"), "query must filter by project_id: {sql}");
    }

    #[test]
    fn test_existence_query_none_for_unknown_kind() {
        let r = ReEvidenceResolver;
        assert!(r.existence_query("bogus").is_none());
    }
}
