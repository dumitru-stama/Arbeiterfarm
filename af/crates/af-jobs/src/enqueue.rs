use af_core::{ToolRequest, ToolSpecRegistry};
use sqlx::PgPool;
use uuid::Uuid;

/// Enqueue a tool run: validate tool exists, check quotas, create queued tool_run row, link input artifacts.
///
/// The quota check + enqueue are wrapped in a transaction with a per-user advisory lock
/// to prevent TOCTOU races where concurrent requests bypass the concurrent run limit.
pub async fn enqueue(
    pool: &PgPool,
    specs: &ToolSpecRegistry,
    request: &ToolRequest,
    actor_subject: Option<&str>,
) -> Result<Uuid, EnqueueError> {
    let spec = specs
        .get_latest(&request.tool_name)
        .ok_or_else(|| EnqueueError::ToolNotFound(request.tool_name.clone()))?;

    // Enforce max_input_bytes: check serialized size of input JSON
    let input_bytes = serde_json::to_string(&request.input_json)
        .map_err(|e| EnqueueError::InvalidInput(format!("failed to serialize input: {e}")))?;
    if input_bytes.len() as u64 > spec.policy.max_input_bytes {
        return Err(EnqueueError::InvalidInput(format!(
            "input size {} bytes exceeds max {} bytes",
            input_bytes.len(),
            spec.policy.max_input_bytes
        )));
    }

    // Enforce max_input_depth: reject deeply nested JSON
    let depth = json_depth_bounded(&request.input_json, spec.policy.max_input_depth + 1);
    if depth > spec.policy.max_input_depth {
        return Err(EnqueueError::InvalidInput(format!(
            "input depth {} exceeds max {}",
            depth, spec.policy.max_input_depth
        )));
    }

    // Resolve artifact IDs from input schema (pre-tx validation — read-only)
    let schema_paths = af_core::resolve_schema_paths(&spec.input_schema);
    eprintln!("[enqueue-debug] tool={} input_json={}", request.tool_name,
        serde_json::to_string(&request.input_json).unwrap_or_else(|_| "<serialize error>".into()));
    eprintln!("[enqueue-debug] tool={} schema_paths={:?}", request.tool_name, schema_paths);
    let artifact_ids = af_core::extract_artifact_ids(&request.input_json, &schema_paths)
        .map_err(|e| {
            eprintln!("[enqueue-debug] tool={} extract_artifact_ids FAILED: {}", request.tool_name, e);
            EnqueueError::InvalidInput(e)
        })?;
    eprintln!("[enqueue-debug] tool={} artifact_ids={:?}", request.tool_name, artifact_ids);

    // Verify all input artifacts exist and belong to this project.
    // Uses a single query + project_id filter. Same error for missing vs wrong-project
    // to prevent cross-tenant artifact enumeration.
    if !artifact_ids.is_empty() {
        let artifact_rows = af_db::artifacts::get_artifacts_by_ids(pool, &artifact_ids)
            .await
            .map_err(|e| EnqueueError::Db(e.to_string()))?;

        let valid: std::collections::HashSet<Uuid> = artifact_rows
            .iter()
            .filter(|r| r.project_id == request.project_id)
            .map(|r| r.id)
            .collect();

        let invalid: Vec<_> = artifact_ids
            .iter()
            .filter(|id| !valid.contains(id))
            .collect();
        if !invalid.is_empty() {
            return Err(EnqueueError::InvalidInput(format!(
                "artifact(s) not accessible in this project: {}",
                invalid
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }

    // Wrap quota check + enqueue in a transaction with per-user advisory lock
    // to prevent TOCTOU race on concurrent run quota.
    let mut tx = pool.begin().await.map_err(|e| EnqueueError::Db(e.to_string()))?;

    if let Some(uid) = request.actor_user_id {
        // Advisory lock keyed on user ID — serializes same-user enqueues without
        // blocking other users. Released automatically on commit/rollback.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text))")
            .bind(uid)
            .execute(&mut *tx)
            .await
            .map_err(|e| EnqueueError::Db(e.to_string()))?;

        let active = af_db::user_quotas::count_active_runs(&mut *tx, uid)
            .await
            .map_err(|e| EnqueueError::Db(e.to_string()))?;
        let quota = af_db::user_quotas::get_quota(&mut *tx, uid)
            .await
            .map_err(|e| EnqueueError::Db(e.to_string()))?;
        if let Some(q) = quota {
            if active >= q.max_concurrent_runs as i64 {
                return Err(EnqueueError::QuotaExceeded(format!(
                    "concurrent run limit ({}) reached",
                    q.max_concurrent_runs
                )));
            }
        }
        // Record tool run usage
        let _ = af_db::user_quotas::record_tool_run(&mut *tx, uid).await;
    }

    // Enqueue the tool run (inside the transaction)
    let row = af_db::tool_runs::enqueue(
        &mut *tx,
        request.project_id,
        &request.tool_name,
        spec.version as i32,
        &request.input_json,
        request.thread_id,
        request.parent_message_id,
        actor_subject,
        request.actor_user_id,
    )
    .await
    .map_err(|e| EnqueueError::Db(e.to_string()))?;

    // Link input artifacts (inside the transaction)
    for aid in &artifact_ids {
        af_db::tool_run_artifacts::link_artifact(&mut *tx, row.id, *aid, "input")
            .await
            .map_err(|e| EnqueueError::Db(e.to_string()))?;
    }

    tx.commit().await.map_err(|e| EnqueueError::Db(e.to_string()))?;

    Ok(row.id)
}

/// Compute the nesting depth of a JSON value. Objects and arrays add 1.
/// Uses an explicit stack to avoid blowing the call stack on pathological input.
/// Bails out early once `bail_at` depth is reached (returns `bail_at`).
fn json_depth_bounded(val: &serde_json::Value, bail_at: u32) -> u32 {
    // Stack of (value, current_depth)
    let mut stack: Vec<(&serde_json::Value, u32)> = vec![(val, 0)];
    let mut max_depth: u32 = 0;

    while let Some((v, depth)) = stack.pop() {
        let child_depth = match v {
            serde_json::Value::Object(map) => {
                let d = depth + 1;
                if d >= bail_at {
                    return bail_at;
                }
                for child in map.values() {
                    stack.push((child, d));
                }
                d
            }
            serde_json::Value::Array(arr) => {
                let d = depth + 1;
                if d >= bail_at {
                    return bail_at;
                }
                for child in arr.iter() {
                    stack.push((child, d));
                }
                d
            }
            _ => depth,
        };
        if child_depth > max_depth {
            max_depth = child_depth;
        }
    }

    max_depth
}

#[derive(Debug, thiserror::Error)]
pub enum EnqueueError {
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("database error: {0}")]
    Db(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
}
