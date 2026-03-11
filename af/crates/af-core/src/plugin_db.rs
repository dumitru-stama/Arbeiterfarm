use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginDbError {
    #[error("query error: {0}")]
    Query(String),
    #[error("migration error: {0}")]
    Migration(String),
}

/// A plugin migration step.
pub struct Migration {
    pub version: u32,
    pub name: String,
    pub sql: String,
}

/// Capability-scoped DB access for plugins.
/// Trait defined in af-core (pure — no sqlx dependency).
///
/// When `user_id` is provided, queries run inside a scoped transaction with
/// `SET LOCAL ROLE af_api` and `af.current_user_id` for RLS enforcement.
/// Pass `None` for shared/content-addressed tables that don't need tenant isolation.
#[async_trait]
pub trait PluginDb: Send + Sync {
    async fn query_json(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
        user_id: Option<uuid::Uuid>,
    ) -> Result<Vec<serde_json::Value>, PluginDbError>;

    async fn execute_json(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
        user_id: Option<uuid::Uuid>,
    ) -> Result<u64, PluginDbError>;

    async fn migrate(&self, migrations: &[Migration]) -> Result<(), PluginDbError>;

    fn schema(&self) -> &str;

    /// Write an entry to the shared `audit_log` table.
    ///
    /// This is a controlled escape hatch: plugins cannot write to `public.audit_log`
    /// through `query_json`/`execute_json` (search_path blocks it), but they can
    /// use this method to log operations. The event_type is automatically prefixed
    /// with the plugin schema name (e.g. `"re:ioc_extracted"`).
    async fn audit_log(
        &self,
        event_type: &str,
        actor_user_id: Option<uuid::Uuid>,
        detail: Option<&serde_json::Value>,
    ) -> Result<(), PluginDbError>;
}
