use async_trait::async_trait;
use af_core::CoreConfig;
use af_db::{
    agents::AgentRow,
    api_keys::ApiKeyRow,
    artifacts::ArtifactRow,
    audit_log::AuditLogRow,
    messages::MessageRow,
    project_hooks::ProjectHookRow,
    project_members::ProjectMemberWithName,
    projects::ProjectRow,
    thread_export::{ExportFormat, run_thread_export},
    threads::ThreadRow,
    users::UserRow,
    workflows::WorkflowRow,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::Backend;

pub struct DirectDb {
    pool: PgPool,
    core_config: CoreConfig,
}

impl DirectDb {
    pub fn new(pool: PgPool, core_config: CoreConfig) -> Self {
        Self { pool, core_config }
    }

    /// Expose the pool for local-only operations (hook firing, etc).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl Backend for DirectDb {
    fn is_local(&self) -> bool {
        true
    }

    // --- Projects ---

    async fn create_project(&self, name: &str) -> anyhow::Result<ProjectRow> {
        Ok(af_db::projects::create_project(&self.pool, name).await?)
    }

    async fn list_projects(&self) -> anyhow::Result<Vec<ProjectRow>> {
        Ok(af_db::projects::list_projects(&self.pool).await?)
    }

    async fn delete_project(&self, id: Uuid) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let ok = af_db::projects::delete_project(&mut *tx, id).await?;
        tx.commit().await?;
        Ok(ok)
    }

    // --- Members ---

    async fn list_members(&self, project_id: Uuid) -> anyhow::Result<Vec<ProjectMemberWithName>> {
        Ok(af_db::project_members::list_members_with_names(&self.pool, project_id).await?)
    }

    async fn add_member(
        &self,
        project_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> anyhow::Result<()> {
        Ok(af_db::project_members::add_member(&self.pool, project_id, user_id, role).await?)
    }

    async fn remove_member(&self, project_id: Uuid, user_id: Uuid) -> anyhow::Result<()> {
        Ok(af_db::project_members::remove_member(&self.pool, project_id, user_id).await?)
    }

    // --- Artifacts ---

    async fn upload_artifact(
        &self,
        project_id: Uuid,
        filename: &str,
        data: &[u8],
    ) -> anyhow::Result<ArtifactRow> {
        let artifact_id = af_storage::artifact_store::ingest_artifact(
            &self.pool,
            &self.core_config.storage_root,
            project_id,
            filename,
            data,
            None,
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Return the full row
        af_db::artifacts::get_artifact(&self.pool, artifact_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("artifact vanished after creation"))
    }

    async fn list_artifacts(&self, project_id: Uuid) -> anyhow::Result<Vec<ArtifactRow>> {
        Ok(af_db::artifacts::list_artifacts(&self.pool, project_id).await?)
    }

    async fn get_artifact(&self, id: Uuid) -> anyhow::Result<Option<ArtifactRow>> {
        Ok(af_db::artifacts::get_artifact(&self.pool, id).await?)
    }

    async fn update_artifact_description(
        &self,
        id: Uuid,
        desc: &str,
    ) -> anyhow::Result<Option<ArtifactRow>> {
        Ok(af_db::artifacts::update_artifact_description(&self.pool, id, desc).await?)
    }

    async fn delete_artifact(&self, id: Uuid) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let ok = af_db::artifacts::delete_artifact(&mut *tx, id).await?;
        tx.commit().await?;
        Ok(ok)
    }

    async fn delete_generated_artifacts(&self, project_id: Uuid) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
        let count = af_db::artifacts::delete_generated_artifacts(&mut *tx, project_id).await?;
        tx.commit().await?;
        Ok(count)
    }

    // --- Conversations ---

    async fn list_threads(&self, project_id: Uuid) -> anyhow::Result<Vec<ThreadRow>> {
        Ok(af_db::threads::list_threads(&self.pool, project_id).await?)
    }

    async fn get_thread_messages(&self, thread_id: Uuid) -> anyhow::Result<Vec<MessageRow>> {
        Ok(af_db::messages::get_thread_messages(&self.pool, thread_id).await?)
    }

    async fn export_thread(&self, thread_id: Uuid, format: &str) -> anyhow::Result<String> {
        let export_format = match format.to_lowercase().as_str() {
            "json" => ExportFormat::Json,
            _ => ExportFormat::Markdown,
        };
        let mut conn = self.pool.acquire().await?;
        run_thread_export(&mut *conn, thread_id, export_format).await
    }

    async fn delete_thread(&self, id: Uuid) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let ok = af_db::threads::delete_thread(&mut *tx, id).await?;
        tx.commit().await?;
        Ok(ok)
    }

    async fn queue_message(&self, thread_id: Uuid, content: &str) -> anyhow::Result<MessageRow> {
        Ok(af_db::messages::insert_message(&self.pool, thread_id, "user", Some(content), None).await?)
    }

    // --- Agents ---

    async fn list_agents(&self) -> anyhow::Result<Vec<AgentRow>> {
        Ok(af_db::agents::list(&self.pool).await?)
    }

    async fn get_agent(&self, name: &str) -> anyhow::Result<Option<AgentRow>> {
        Ok(af_db::agents::get(&self.pool, name).await?)
    }

    async fn upsert_agent(
        &self,
        name: &str,
        prompt: &str,
        tools: &serde_json::Value,
        route: &str,
        metadata: &serde_json::Value,
        is_builtin: bool,
        source: Option<&str>,
        timeout_secs: Option<i32>,
    ) -> anyhow::Result<AgentRow> {
        Ok(
            af_db::agents::upsert(
                &self.pool,
                name,
                prompt,
                tools,
                route,
                metadata,
                is_builtin,
                source,
                timeout_secs,
            )
            .await?,
        )
    }

    async fn delete_agent(&self, name: &str) -> anyhow::Result<bool> {
        Ok(af_db::agents::delete(&self.pool, name).await?)
    }

    // --- Workflows ---

    async fn list_workflows(&self) -> anyhow::Result<Vec<WorkflowRow>> {
        Ok(af_db::workflows::list(&self.pool).await?)
    }

    async fn get_workflow(&self, name: &str) -> anyhow::Result<Option<WorkflowRow>> {
        Ok(af_db::workflows::get(&self.pool, name).await?)
    }

    // --- Hooks ---

    async fn list_hooks(&self, project_id: Uuid) -> anyhow::Result<Vec<ProjectHookRow>> {
        Ok(af_db::project_hooks::list_by_project(&self.pool, project_id).await?)
    }

    async fn create_hook(
        &self,
        project_id: Uuid,
        name: &str,
        event: &str,
        workflow: Option<&str>,
        agent: Option<&str>,
        prompt: &str,
        route: Option<&str>,
        interval: Option<i32>,
    ) -> anyhow::Result<ProjectHookRow> {
        Ok(
            af_db::project_hooks::create(
                &self.pool,
                project_id,
                name,
                event,
                workflow,
                agent,
                prompt,
                route,
                interval,
            )
            .await?,
        )
    }

    async fn get_hook(&self, id: Uuid) -> anyhow::Result<Option<ProjectHookRow>> {
        Ok(af_db::project_hooks::get(&self.pool, id).await?)
    }

    async fn update_hook(
        &self,
        id: Uuid,
        enabled: Option<bool>,
        prompt: Option<&str>,
        route: Option<Option<&str>>,
        interval: Option<i32>,
    ) -> anyhow::Result<Option<ProjectHookRow>> {
        Ok(af_db::project_hooks::update(&self.pool, id, enabled, prompt, route, interval)
            .await?)
    }

    async fn delete_hook(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::project_hooks::delete(&self.pool, id).await?)
    }

    // --- Audit ---

    async fn list_audit(
        &self,
        limit: i64,
        event_type: Option<&str>,
    ) -> anyhow::Result<Vec<AuditLogRow>> {
        Ok(af_db::audit_log::list(&self.pool, limit, event_type).await?)
    }

    // --- Users ---

    async fn create_user(
        &self,
        subject: &str,
        display: Option<&str>,
        email: Option<&str>,
        roles: &[String],
    ) -> anyhow::Result<UserRow> {
        Ok(af_db::users::create_user(&self.pool, subject, display, email, roles).await?)
    }

    async fn list_users(&self) -> anyhow::Result<Vec<UserRow>> {
        Ok(af_db::users::list_users(&self.pool).await?)
    }

    async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRow>> {
        Ok(af_db::users::get_by_id(&self.pool, id).await?)
    }

    // --- API Keys ---

    async fn create_api_key(
        &self,
        user_id: Uuid,
        name: &str,
    ) -> anyhow::Result<(String, ApiKeyRow)> {
        // Verify user exists
        af_db::users::get_by_id(&self.pool, user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("user not found: {user_id}"))?;

        let (raw_key, key_hash, key_prefix) = af_auth::generate_key();
        let row = af_db::api_keys::create_key(
            &self.pool,
            user_id,
            &key_hash,
            &key_prefix,
            name,
            &["all".to_string()],
        )
        .await?;

        Ok((raw_key, row))
    }

    async fn list_api_keys(&self, user_id: Uuid) -> anyhow::Result<Vec<ApiKeyRow>> {
        Ok(af_db::api_keys::list_for_user(&self.pool, user_id).await?)
    }

    async fn revoke_api_key(&self, key_id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::api_keys::delete(&self.pool, key_id).await?)
    }

    // --- Project Settings ---

    async fn get_project_settings(&self, project_id: Uuid) -> anyhow::Result<serde_json::Value> {
        let row = af_db::projects::get_project(&self.pool, project_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;
        Ok(row.settings)
    }

    async fn update_project_settings(
        &self,
        project_id: Uuid,
        settings: &serde_json::Value,
    ) -> anyhow::Result<ProjectRow> {
        let row = af_db::projects::update_settings(&self.pool, project_id, settings)
            .await?
            .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;
        Ok(row)
    }

    async fn set_nda(&self, project_id: Uuid, nda: bool) -> anyhow::Result<(ProjectRow, bool)> {
        let mut tx = self.pool.begin().await?;
        let (row, old_nda) = af_db::projects::set_nda(&mut tx, project_id, nda, None)
            .await?
            .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;
        tx.commit().await?;
        Ok((row, old_nda))
    }

    // --- User Allowed Routes ---

    async fn list_user_routes(&self, user_id: Uuid) -> anyhow::Result<Vec<String>> {
        let rows = af_db::user_allowed_routes::list_routes(&self.pool, user_id).await?;
        Ok(rows.into_iter().map(|r| r.route).collect())
    }

    async fn add_user_route(&self, user_id: Uuid, route: &str) -> anyhow::Result<()> {
        af_db::user_allowed_routes::add_route(&self.pool, user_id, route).await?;
        Ok(())
    }

    async fn remove_user_route(&self, user_id: Uuid, route: &str) -> anyhow::Result<bool> {
        Ok(af_db::user_allowed_routes::remove_route(&self.pool, user_id, route).await?)
    }

    async fn clear_user_routes(&self, user_id: Uuid) -> anyhow::Result<u64> {
        Ok(af_db::user_allowed_routes::remove_all_routes(&self.pool, user_id).await?)
    }

    // --- Web Fetch Rules ---

    async fn list_web_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<af_db::web_fetch::WebFetchRuleRow>> {
        Ok(af_db::web_fetch::list_rules(&self.pool, None, project_id).await?)
    }

    async fn add_web_rule(
        &self,
        scope: &str,
        project_id: Option<Uuid>,
        rule_type: &str,
        pattern_type: &str,
        pattern: &str,
        description: Option<&str>,
    ) -> anyhow::Result<af_db::web_fetch::WebFetchRuleRow> {
        Ok(af_db::web_fetch::add_rule(
            &self.pool, scope, project_id, rule_type, pattern_type, pattern, description, None,
        ).await?)
    }

    async fn remove_web_rule(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::web_fetch::remove_rule(&self.pool, id).await?)
    }

    async fn list_country_blocks(&self) -> anyhow::Result<Vec<af_db::web_fetch::CountryBlockRow>> {
        Ok(af_db::web_fetch::list_country_blocks(&self.pool).await?)
    }

    async fn add_country_block(&self, code: &str, name: Option<&str>) -> anyhow::Result<af_db::web_fetch::CountryBlockRow> {
        Ok(af_db::web_fetch::add_country_block(&self.pool, code, name, None).await?)
    }

    async fn remove_country_block(&self, code: &str) -> anyhow::Result<bool> {
        Ok(af_db::web_fetch::remove_country_block(&self.pool, code).await?)
    }

    // --- Restricted Tools ---

    async fn list_restricted_tools(&self) -> anyhow::Result<Vec<af_db::restricted_tools::RestrictedToolRow>> {
        Ok(af_db::restricted_tools::list_restricted(&self.pool).await?)
    }

    async fn add_restricted_tool(&self, pattern: &str, description: &str) -> anyhow::Result<af_db::restricted_tools::RestrictedToolRow> {
        Ok(af_db::restricted_tools::add_restricted(&self.pool, pattern, description).await?)
    }

    async fn remove_restricted_tool(&self, pattern: &str) -> anyhow::Result<bool> {
        Ok(af_db::restricted_tools::remove_restricted(&self.pool, pattern).await?)
    }

    async fn list_user_grants(&self, user_id: Uuid) -> anyhow::Result<Vec<af_db::restricted_tools::UserToolGrantRow>> {
        Ok(af_db::restricted_tools::list_user_grants(&self.pool, user_id).await?)
    }

    async fn add_user_grant(&self, user_id: Uuid, pattern: &str) -> anyhow::Result<af_db::restricted_tools::UserToolGrantRow> {
        Ok(af_db::restricted_tools::add_user_grant(&self.pool, user_id, pattern).await?)
    }

    async fn remove_user_grant(&self, user_id: Uuid, pattern: &str) -> anyhow::Result<bool> {
        Ok(af_db::restricted_tools::remove_user_grant(&self.pool, user_id, pattern).await?)
    }

    // --- Email Recipient Rules ---

    async fn list_email_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<af_db::email::EmailRecipientRuleRow>> {
        Ok(af_db::email::list_recipient_rules(&self.pool, None, project_id).await?)
    }

    async fn add_email_rule(
        &self,
        scope: &str,
        project_id: Option<Uuid>,
        rule_type: &str,
        pattern_type: &str,
        pattern: &str,
        description: Option<&str>,
    ) -> anyhow::Result<af_db::email::EmailRecipientRuleRow> {
        Ok(af_db::email::add_recipient_rule(
            &self.pool, scope, project_id, rule_type, pattern_type, pattern, description, None,
        ).await?)
    }

    async fn remove_email_rule(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::email::remove_recipient_rule(&self.pool, id).await?)
    }

    // --- Email Credentials ---

    async fn upsert_email_credential(
        &self,
        user_id: Uuid,
        provider: &str,
        email_address: &str,
        credentials_json: &serde_json::Value,
        is_default: bool,
    ) -> anyhow::Result<af_db::email::EmailCredentialRow> {
        Ok(af_db::email::upsert_credential(&self.pool, user_id, provider, email_address, credentials_json, is_default).await?)
    }

    async fn list_email_credentials(&self, user_id: Uuid) -> anyhow::Result<Vec<af_db::email::EmailCredentialRow>> {
        Ok(af_db::email::list_credentials(&self.pool, user_id).await?)
    }

    async fn delete_email_credential(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::email::delete_credential(&self.pool, id).await?)
    }

    // --- Email Tone Presets ---

    async fn list_email_tone_presets(&self) -> anyhow::Result<Vec<af_db::email::EmailTonePresetRow>> {
        Ok(af_db::email::list_tone_presets(&self.pool).await?)
    }

    async fn upsert_email_tone_preset(
        &self,
        name: &str,
        description: Option<&str>,
        system_instruction: &str,
    ) -> anyhow::Result<af_db::email::EmailTonePresetRow> {
        Ok(af_db::email::upsert_tone_preset(&self.pool, name, description, system_instruction, false, None).await?)
    }

    async fn delete_email_tone_preset(&self, name: &str) -> anyhow::Result<bool> {
        Ok(af_db::email::delete_tone_preset(&self.pool, name).await?)
    }

    // --- Email Scheduling ---

    async fn list_scheduled_emails(
        &self,
        project_id: Option<Uuid>,
        status: Option<&str>,
    ) -> anyhow::Result<Vec<af_db::email::EmailScheduledRow>> {
        Ok(af_db::email::list_scheduled(&self.pool, project_id, status).await?)
    }

    async fn cancel_scheduled_email(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::email::cancel_scheduled(&self.pool, id).await?)
    }

    // --- YARA Rules ---

    async fn list_yara_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<af_db::yara::YaraRuleRow>> {
        Ok(af_db::yara::list_rules(&self.pool, project_id).await?)
    }

    async fn get_yara_rule(&self, id: Uuid) -> anyhow::Result<Option<af_db::yara::YaraRuleRow>> {
        Ok(af_db::yara::get_rule(&self.pool, id).await?)
    }

    async fn remove_yara_rule(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(af_db::yara::delete_rule(&self.pool, id).await?)
    }

    async fn list_yara_scan_results(
        &self,
        artifact_id: Option<Uuid>,
        rule_name: Option<&str>,
    ) -> anyhow::Result<Vec<af_db::yara::YaraScanResultRow>> {
        Ok(af_db::yara::list_scan_results(&self.pool, artifact_id, rule_name).await?)
    }
}
