use af_plugin_api::{PluginDb, PluginDbError};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// VT response cache backed by `re.vt_cache` table via PluginDb.
pub struct VtCache {
    plugin_db: Arc<dyn PluginDb>,
    ttl: Duration,
}

impl VtCache {
    pub fn new(plugin_db: Arc<dyn PluginDb>, ttl: Duration) -> Self {
        Self { plugin_db, ttl }
    }

    /// Get cached response if fresh (within TTL).
    pub async fn get(&self, sha256: &str) -> Result<Option<Value>, PluginDbError> {
        let ttl_secs = self.ttl.as_secs() as i64;
        let rows = self
            .plugin_db
            .query_json(
                "SELECT response FROM vt_cache \
                 WHERE sha256 = $1 \
                 AND fetched_at > now() - make_interval(secs => $2::double precision)",
                vec![Value::String(sha256.to_string()), Value::from(ttl_secs)],
                None, // VT cache is content-addressed, no tenant isolation needed
            )
            .await?;

        if let Some(row) = rows.into_iter().next() {
            // row is a JSON object with "response" key
            if let Value::Object(map) = row {
                return Ok(map.get("response").cloned());
            }
        }
        Ok(None)
    }

    /// Store or update cached response.
    pub async fn put(
        &self,
        sha256: &str,
        response: &Value,
        positives: Option<i64>,
        total: Option<i64>,
    ) -> Result<(), PluginDbError> {
        self.plugin_db
            .execute_json(
                "INSERT INTO vt_cache (sha256, response, positives, total, fetched_at) \
                 VALUES ($1, $2, $3, $4, now()) \
                 ON CONFLICT (sha256) DO UPDATE SET \
                   response = EXCLUDED.response, \
                   positives = EXCLUDED.positives, \
                   total = EXCLUDED.total, \
                   fetched_at = now()",
                vec![
                    Value::String(sha256.to_string()),
                    response.clone(),
                    positives.map(Value::from).unwrap_or(Value::Null),
                    total.map(Value::from).unwrap_or(Value::Null),
                ],
                None, // VT cache is content-addressed, no tenant isolation needed
            )
            .await?;
        Ok(())
    }

    /// Evict expired entries. Returns number of rows deleted.
    pub async fn evict_expired(&self) -> Result<u64, PluginDbError> {
        let ttl_secs = self.ttl.as_secs() as i64;
        self.plugin_db
            .execute_json(
                "DELETE FROM vt_cache \
                 WHERE fetched_at <= now() - make_interval(secs => $1::double precision)",
                vec![Value::from(ttl_secs)],
                None, // VT cache is content-addressed, no tenant isolation needed
            )
            .await
    }
}
