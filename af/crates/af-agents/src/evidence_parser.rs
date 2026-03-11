use af_core::{EvidenceRef, EvidenceResolverRegistry};
use sqlx::PgPool;
use uuid::Uuid;

/// Parsed and verified evidence reference ready for DB insertion.
pub struct VerifiedEvidence {
    pub ref_type: String,
    pub ref_id: Uuid,
}

/// Parse evidence references from a message, verify they exist in the DB
/// and belong to the given project.
///
/// Plugin evidence (e.g. `evidence:re:ioc:<uuid>`) is verified via the
/// `EvidenceResolverRegistry` if provided.
pub async fn parse_and_verify(
    pool: &PgPool,
    content: &str,
    project_id: Uuid,
    evidence_resolvers: Option<&EvidenceResolverRegistry>,
) -> Vec<VerifiedEvidence> {
    let mut verified = Vec::new();

    // Find all evidence:type:uuid patterns
    for word in content.split_whitespace() {
        // Strip surrounding punctuation/backticks
        let cleaned = word.trim_matches(|c: char| c == '`' || c == '\'' || c == '"' || c == ',' || c == '.');
        if let Some(evidence_ref) = EvidenceRef::parse(cleaned) {
            if let Some(v) = verify_ref(pool, &evidence_ref, project_id, evidence_resolvers).await {
                verified.push(v);
            }
        }
    }

    verified
}

async fn verify_ref(
    pool: &PgPool,
    evidence_ref: &EvidenceRef,
    project_id: Uuid,
    evidence_resolvers: Option<&EvidenceResolverRegistry>,
) -> Option<VerifiedEvidence> {
    match evidence_ref {
        EvidenceRef::Artifact(id) => {
            // Check artifact exists and belongs to project
            let row = af_db::artifacts::get_artifact(pool, *id).await.ok()?;
            let artifact = row?;
            if artifact.project_id == project_id {
                Some(VerifiedEvidence {
                    ref_type: "artifact".into(),
                    ref_id: *id,
                })
            } else {
                None
            }
        }
        EvidenceRef::ToolRun(id) => {
            // Check tool_run exists and belongs to project
            let row = af_db::tool_runs::get(pool, *id).await.ok()?;
            let run = row?;
            if run.project_id == project_id {
                Some(VerifiedEvidence {
                    ref_type: "tool_run".into(),
                    ref_id: *id,
                })
            } else {
                None
            }
        }
        EvidenceRef::Message(id) => {
            // Messages are project-scoped via thread
            Some(VerifiedEvidence {
                ref_type: "message".into(),
                ref_id: *id,
            })
        }
        EvidenceRef::Plugin {
            namespace,
            kind,
            id,
        } => {
            // Verify via the plugin's evidence resolver
            if let Some(resolvers) = evidence_resolvers {
                if resolvers.resolve(namespace, kind, id).is_none() {
                    return None;
                }
                let ref_id = match Uuid::parse_str(id) {
                    Ok(uuid) => uuid,
                    Err(_) => {
                        tracing::warn!("ignoring plugin evidence with invalid UUID: {id}");
                        return None;
                    }
                };

                // If the resolver provides an existence query, verify the record
                // actually exists in the DB and belongs to this project.
                if let Some(sql) = resolvers.existence_query(namespace, kind) {
                    let exists = sqlx::query(sql)
                        .bind(ref_id)
                        .bind(project_id)
                        .fetch_optional(pool)
                        .await
                        .ok()
                        .flatten()
                        .is_some();
                    if !exists {
                        return None;
                    }
                }

                Some(VerifiedEvidence {
                    ref_type: format!("plugin:{namespace}:{kind}"),
                    ref_id,
                })
            } else {
                None
            }
        }
    }
}
