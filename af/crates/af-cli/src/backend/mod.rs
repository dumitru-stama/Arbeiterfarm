pub mod direct;
pub mod remote;

use async_trait::async_trait;
use af_db::{
    agents::AgentRow,
    api_keys::ApiKeyRow,
    artifacts::ArtifactRow,
    audit_log::AuditLogRow,
    email::{EmailCredentialRow, EmailRecipientRuleRow, EmailScheduledRow, EmailTonePresetRow},
    messages::MessageRow,
    project_hooks::ProjectHookRow,
    project_members::ProjectMemberWithName,
    projects::ProjectRow,
    restricted_tools::{RestrictedToolRow, UserToolGrantRow},
    threads::ThreadRow,
    users::UserRow,
    web_fetch::{CountryBlockRow, WebFetchRuleRow},
    workflows::WorkflowRow,
    yara::{YaraRuleRow, YaraScanResultRow},
};
use uuid::Uuid;

#[async_trait]
pub trait Backend: Send + Sync {
    /// True for DirectDb, false for RemoteApi — used to decide
    /// whether local-only operations (hook firing, key generation) apply.
    fn is_local(&self) -> bool;

    // --- Projects ---
    async fn create_project(&self, name: &str) -> anyhow::Result<ProjectRow>;
    async fn list_projects(&self) -> anyhow::Result<Vec<ProjectRow>>;

    async fn delete_project(&self, id: Uuid) -> anyhow::Result<bool>;

    // --- Members ---
    async fn list_members(&self, project_id: Uuid) -> anyhow::Result<Vec<ProjectMemberWithName>>;
    async fn add_member(&self, project_id: Uuid, user_id: Uuid, role: &str)
        -> anyhow::Result<()>;
    async fn remove_member(&self, project_id: Uuid, user_id: Uuid) -> anyhow::Result<()>;

    // --- Artifacts ---
    async fn upload_artifact(
        &self,
        project_id: Uuid,
        filename: &str,
        data: &[u8],
    ) -> anyhow::Result<ArtifactRow>;
    async fn list_artifacts(&self, project_id: Uuid) -> anyhow::Result<Vec<ArtifactRow>>;
    async fn get_artifact(&self, id: Uuid) -> anyhow::Result<Option<ArtifactRow>>;
    async fn update_artifact_description(
        &self,
        id: Uuid,
        desc: &str,
    ) -> anyhow::Result<Option<ArtifactRow>>;
    async fn delete_artifact(&self, id: Uuid) -> anyhow::Result<bool>;
    async fn delete_generated_artifacts(&self, project_id: Uuid) -> anyhow::Result<u64>;

    // --- Conversations ---
    async fn list_threads(&self, project_id: Uuid) -> anyhow::Result<Vec<ThreadRow>>;
    async fn get_thread_messages(&self, thread_id: Uuid) -> anyhow::Result<Vec<MessageRow>>;
    async fn export_thread(&self, thread_id: Uuid, format: &str) -> anyhow::Result<String>;
    async fn delete_thread(&self, id: Uuid) -> anyhow::Result<bool>;
    /// Insert a user message without triggering the LLM.
    async fn queue_message(&self, thread_id: Uuid, content: &str) -> anyhow::Result<MessageRow>;

    // --- Agents ---
    async fn list_agents(&self) -> anyhow::Result<Vec<AgentRow>>;
    async fn get_agent(&self, name: &str) -> anyhow::Result<Option<AgentRow>>;
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
    ) -> anyhow::Result<AgentRow>;
    async fn delete_agent(&self, name: &str) -> anyhow::Result<bool>;

    // --- Workflows ---
    async fn list_workflows(&self) -> anyhow::Result<Vec<WorkflowRow>>;
    async fn get_workflow(&self, name: &str) -> anyhow::Result<Option<WorkflowRow>>;

    // --- Hooks ---
    async fn list_hooks(&self, project_id: Uuid) -> anyhow::Result<Vec<ProjectHookRow>>;
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
    ) -> anyhow::Result<ProjectHookRow>;
    async fn get_hook(&self, id: Uuid) -> anyhow::Result<Option<ProjectHookRow>>;
    async fn update_hook(
        &self,
        id: Uuid,
        enabled: Option<bool>,
        prompt: Option<&str>,
        route: Option<Option<&str>>,
        interval: Option<i32>,
    ) -> anyhow::Result<Option<ProjectHookRow>>;
    async fn delete_hook(&self, id: Uuid) -> anyhow::Result<bool>;

    // --- Audit ---
    async fn list_audit(
        &self,
        limit: i64,
        event_type: Option<&str>,
    ) -> anyhow::Result<Vec<AuditLogRow>>;

    // --- Users ---
    async fn create_user(
        &self,
        subject: &str,
        display: Option<&str>,
        email: Option<&str>,
        roles: &[String],
    ) -> anyhow::Result<UserRow>;
    async fn list_users(&self) -> anyhow::Result<Vec<UserRow>>;
    async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRow>>;

    // --- API Keys ---
    /// Returns (raw_key, ApiKeyRow). raw_key is only shown once.
    async fn create_api_key(
        &self,
        user_id: Uuid,
        name: &str,
    ) -> anyhow::Result<(String, ApiKeyRow)>;
    async fn list_api_keys(&self, user_id: Uuid) -> anyhow::Result<Vec<ApiKeyRow>>;
    async fn revoke_api_key(&self, key_id: Uuid) -> anyhow::Result<bool>;

    // --- Project Settings ---
    async fn get_project_settings(&self, project_id: Uuid) -> anyhow::Result<serde_json::Value>;
    async fn update_project_settings(
        &self,
        project_id: Uuid,
        settings: &serde_json::Value,
    ) -> anyhow::Result<ProjectRow>;
    /// Returns `(updated_row, old_nda)` so callers can detect transitions.
    async fn set_nda(&self, project_id: Uuid, nda: bool) -> anyhow::Result<(ProjectRow, bool)>;

    // --- User Allowed Routes ---
    async fn list_user_routes(&self, user_id: Uuid) -> anyhow::Result<Vec<String>>;
    async fn add_user_route(&self, user_id: Uuid, route: &str) -> anyhow::Result<()>;
    async fn remove_user_route(&self, user_id: Uuid, route: &str) -> anyhow::Result<bool>;
    async fn clear_user_routes(&self, user_id: Uuid) -> anyhow::Result<u64>;

    // --- Web Fetch Rules ---
    async fn list_web_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<WebFetchRuleRow>>;
    async fn add_web_rule(
        &self,
        scope: &str,
        project_id: Option<Uuid>,
        rule_type: &str,
        pattern_type: &str,
        pattern: &str,
        description: Option<&str>,
    ) -> anyhow::Result<WebFetchRuleRow>;
    async fn remove_web_rule(&self, id: Uuid) -> anyhow::Result<bool>;
    async fn list_country_blocks(&self) -> anyhow::Result<Vec<CountryBlockRow>>;
    async fn add_country_block(&self, code: &str, name: Option<&str>) -> anyhow::Result<CountryBlockRow>;
    async fn remove_country_block(&self, code: &str) -> anyhow::Result<bool>;

    // --- Restricted Tools ---
    async fn list_restricted_tools(&self) -> anyhow::Result<Vec<RestrictedToolRow>>;
    async fn add_restricted_tool(&self, pattern: &str, description: &str) -> anyhow::Result<RestrictedToolRow>;
    async fn remove_restricted_tool(&self, pattern: &str) -> anyhow::Result<bool>;
    async fn list_user_grants(&self, user_id: Uuid) -> anyhow::Result<Vec<UserToolGrantRow>>;
    async fn add_user_grant(&self, user_id: Uuid, pattern: &str) -> anyhow::Result<UserToolGrantRow>;
    async fn remove_user_grant(&self, user_id: Uuid, pattern: &str) -> anyhow::Result<bool>;

    // --- Email Recipient Rules ---
    async fn list_email_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<EmailRecipientRuleRow>>;
    async fn add_email_rule(
        &self,
        scope: &str,
        project_id: Option<Uuid>,
        rule_type: &str,
        pattern_type: &str,
        pattern: &str,
        description: Option<&str>,
    ) -> anyhow::Result<EmailRecipientRuleRow>;
    async fn remove_email_rule(&self, id: Uuid) -> anyhow::Result<bool>;

    // --- Email Credentials ---
    async fn upsert_email_credential(
        &self,
        user_id: Uuid,
        provider: &str,
        email_address: &str,
        credentials_json: &serde_json::Value,
        is_default: bool,
    ) -> anyhow::Result<EmailCredentialRow>;
    async fn list_email_credentials(&self, user_id: Uuid) -> anyhow::Result<Vec<EmailCredentialRow>>;
    async fn delete_email_credential(&self, id: Uuid) -> anyhow::Result<bool>;

    // --- Email Tone Presets ---
    async fn list_email_tone_presets(&self) -> anyhow::Result<Vec<EmailTonePresetRow>>;
    async fn upsert_email_tone_preset(
        &self,
        name: &str,
        description: Option<&str>,
        system_instruction: &str,
    ) -> anyhow::Result<EmailTonePresetRow>;
    async fn delete_email_tone_preset(&self, name: &str) -> anyhow::Result<bool>;

    // --- Email Scheduling ---
    async fn list_scheduled_emails(
        &self,
        project_id: Option<Uuid>,
        status: Option<&str>,
    ) -> anyhow::Result<Vec<EmailScheduledRow>>;
    async fn cancel_scheduled_email(&self, id: Uuid) -> anyhow::Result<bool>;

    // --- YARA Rules ---
    async fn list_yara_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<YaraRuleRow>>;
    async fn get_yara_rule(&self, id: Uuid) -> anyhow::Result<Option<YaraRuleRow>>;
    async fn remove_yara_rule(&self, id: Uuid) -> anyhow::Result<bool>;
    async fn list_yara_scan_results(
        &self,
        artifact_id: Option<Uuid>,
        rule_name: Option<&str>,
    ) -> anyhow::Result<Vec<YaraScanResultRow>>;
}
