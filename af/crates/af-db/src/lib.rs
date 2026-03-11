pub mod agents;
pub mod api_keys;
pub mod artifacts;
pub mod audit_log;
pub mod blobs;
pub mod email;
pub mod embed_queue;
pub mod embeddings;
pub mod llm_usage_log;
pub mod message_evidence;
pub mod messages;
pub mod notifications;
pub mod project_hooks;
pub mod project_members;
pub mod projects;
pub mod restricted_tools;
pub mod scoped;
pub mod scoped_plugin_db;
pub mod thread_export;
pub mod thread_memory;
pub mod threads;
pub mod tool_config;
pub mod tool_run_artifacts;
pub mod tool_run_events;
pub mod tool_runs;
pub mod url_ingest;
pub mod user_allowed_routes;
pub mod user_quotas;
pub mod users;
pub mod web_fetch;
pub mod workflows;
pub mod yara;

pub use scoped::begin_scoped;
pub use scoped_plugin_db::ScopedPluginDb;

use sqlx::postgres::PgPoolOptions;
pub use sqlx::PgPool;

/// Connect to Postgres and run migrations.
pub async fn init_db(database_url: &str) -> Result<PgPool, sqlx::Error> {
    init_db_with_pool_size(database_url, 10).await
}

/// Connect to Postgres with a configurable pool size and run migrations.
pub async fn init_db_with_pool_size(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await?;

    run_migrations(&pool).await?;

    Ok(pool)
}

/// Run a single migration inside an explicit transaction.
/// PostgreSQL supports DDL in transactions, so CREATE TABLE / ALTER TABLE / CREATE POLICY
/// all roll back cleanly on failure — no partial migration state.
async fn run_one_migration(pool: &PgPool, name: &str, sql: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(e) = sqlx::raw_sql(sql).execute(&mut *tx).await {
        eprintln!("[migrations] {name} FAILED: {e}");
        // tx drops → implicit rollback
        return Err(e);
    }
    tx.commit().await?;
    Ok(())
}

async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    run_one_migration(pool, "001_initial", include_str!("../migrations/001_initial.sql")).await?;
    run_one_migration(pool, "002_slice2", include_str!("../migrations/002_slice2.sql")).await?;
    run_one_migration(pool, "003_plugin_migrations", include_str!("../migrations/003_plugin_migrations.sql")).await?;
    run_one_migration(pool, "004_audit", include_str!("../migrations/004_audit.sql")).await?;
    run_one_migration(pool, "005_message_tool_fields", include_str!("../migrations/005_message_tool_fields.sql")).await?;
    run_one_migration(pool, "006_tenancy", include_str!("../migrations/006_tenancy.sql")).await?;
    run_one_migration(pool, "007_rbac", include_str!("../migrations/007_rbac.sql")).await?;
    run_one_migration(pool, "008_quotas", include_str!("../migrations/008_quotas.sql")).await?;
    run_one_migration(pool, "009_audit_immutable", include_str!("../migrations/009_audit_immutable.sql")).await?;
    run_one_migration(pool, "010_rls", include_str!("../migrations/010_rls.sql")).await?;
    run_one_migration(pool, "011_security_hardening", include_str!("../migrations/011_security_hardening.sql")).await?;
    run_one_migration(pool, "012_agents_and_workflows", include_str!("../migrations/012_agents_and_workflows.sql")).await?;
    run_one_migration(pool, "013_message_seq", include_str!("../migrations/013_message_seq.sql")).await?;
    run_one_migration(pool, "014_rate_limit", include_str!("../migrations/014_rate_limit.sql")).await?;
    run_one_migration(pool, "015_artifact_metadata", include_str!("../migrations/015_artifact_metadata.sql")).await?;
    run_one_migration(pool, "016_thread_lineage", include_str!("../migrations/016_thread_lineage.sql")).await?;
    run_one_migration(pool, "017_artifact_description", include_str!("../migrations/017_artifact_description.sql")).await?;
    run_one_migration(pool, "018_manager_role", include_str!("../migrations/018_manager_role.sql")).await?;
    run_one_migration(pool, "019_source_plugin", include_str!("../migrations/019_source_plugin.sql")).await?;
    run_one_migration(pool, "020_project_hooks", include_str!("../migrations/020_project_hooks.sql")).await?;
    run_one_migration(pool, "021_agent_timeout", include_str!("../migrations/021_agent_timeout.sql")).await?;
    run_one_migration(pool, "022_thread_type", include_str!("../migrations/022_thread_type.sql")).await?;
    run_one_migration(pool, "023_user_allowed_routes", include_str!("../migrations/023_user_allowed_routes.sql")).await?;
    run_one_migration(pool, "024_llm_usage_log", include_str!("../migrations/024_llm_usage_log.sql")).await?;
    run_one_migration(pool, "025_context_compaction", include_str!("../migrations/025_context_compaction.sql")).await?;
    run_one_migration(pool, "026_embeddings", include_str!("../migrations/026_embeddings.sql")).await?;
    run_one_migration(pool, "027_project_settings", include_str!("../migrations/027_project_settings.sql")).await?;
    run_one_migration(pool, "028_nda", include_str!("../migrations/028_nda.sql")).await?;
    run_one_migration(pool, "029_web_gateway", include_str!("../migrations/029_web_gateway.sql")).await?;
    run_one_migration(pool, "030_email", include_str!("../migrations/030_email.sql")).await?;
    run_one_migration(pool, "031_fix_rls_search_path", include_str!("../migrations/031_fix_rls_search_path.sql")).await?;
    run_one_migration(pool, "032_thread_memory", include_str!("../migrations/032_thread_memory.sql")).await?;
    run_one_migration(pool, "033_thread_target_artifact", include_str!("../migrations/033_thread_target_artifact.sql")).await?;
    run_one_migration(pool, "034_embed_queue", include_str!("../migrations/034_embed_queue.sql")).await?;
    run_one_migration(pool, "035_url_ingest_queue", include_str!("../migrations/035_url_ingest_queue.sql")).await?;
    run_one_migration(pool, "036_notifications", include_str!("../migrations/036_notifications.sql")).await?;
    Ok(())
}
