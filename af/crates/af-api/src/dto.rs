use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// --- Projects ---

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub settings: serde_json::Value,
    pub nda: bool,
}

impl From<af_db::projects::ProjectRow> for ProjectResponse {
    fn from(row: af_db::projects::ProjectRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            owner_id: row.owner_id,
            created_at: row.created_at,
            settings: row.settings,
            nda: row.nda,
        }
    }
}

// --- Artifacts ---

#[derive(Debug, Serialize)]
pub struct ArtifactResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub sha256: String,
    pub filename: String,
    pub mime_type: Option<String>,
    pub source_tool_run_id: Option<Uuid>,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<af_db::artifacts::ArtifactRow> for ArtifactResponse {
    fn from(row: af_db::artifacts::ArtifactRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            sha256: row.sha256,
            filename: row.filename,
            mime_type: row.mime_type,
            source_tool_run_id: row.source_tool_run_id,
            description: row.description,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateArtifactDescriptionRequest {
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct UploadArtifactResponse {
    pub id: Uuid,
    pub sha256: String,
    pub filename: String,
    pub created_at: DateTime<Utc>,
}

// --- Threads ---

#[derive(Debug, Deserialize)]
pub struct CreateThreadRequest {
    pub agent_name: String,
    pub title: Option<String>,
    /// Target artifact for this thread (set when launching analysis from a specific sample).
    pub target_artifact_id: Option<uuid::Uuid>,
    #[serde(default = "default_thread_type")]
    pub thread_type: String,
}

fn default_thread_type() -> String {
    "agent".to_string()
}

#[derive(Debug, Serialize)]
pub struct ThreadResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub agent_name: String,
    pub title: Option<String>,
    pub parent_thread_id: Option<Uuid>,
    pub thread_type: String,
    pub target_artifact_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl From<af_db::threads::ThreadRow> for ThreadResponse {
    fn from(row: af_db::threads::ThreadRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            agent_name: row.agent_name,
            title: row.title,
            parent_thread_id: row.parent_thread_id,
            thread_type: row.thread_type,
            target_artifact_id: row.target_artifact_id,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StartThinkingRequest {
    pub goal: String,
    pub agent_name: Option<String>,
    pub route: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunThinkingRequest {
    pub goal: String,
    pub agent_name: Option<String>,
    pub route: Option<String>,
}

// --- Messages ---

#[derive(Debug, Deserialize)]
pub struct QueueMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct QueueMessageResponse {
    pub id: Uuid,
    pub seq: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub agent_name: Option<String>,
    /// Optional LLM route override (e.g. "backend:openai:gpt-4o-mini").
    /// If set, overrides the agent's default_route for this message only.
    pub route: Option<String>,
    /// Optional system prompt override. When set, replaces the agent's default
    /// system prompt (artifact context and tool sections are still appended).
    pub system_prompt_override: Option<String>,
    /// Optional multi-modal content parts (text + images).
    /// When present, backends use these instead of `content` for the LLM request.
    #[serde(default)]
    pub content_parts: Option<Vec<af_core::ContentPart>>,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: Uuid,
    pub role: String,
    pub content: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub agent_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<af_db::messages::MessageRow> for MessageResponse {
    fn from(row: af_db::messages::MessageRow) -> Self {
        Self {
            id: row.id,
            role: row.role,
            content: row.content,
            tool_call_id: row.tool_call_id,
            tool_name: row.tool_name,
            agent_name: row.agent_name,
            created_at: row.created_at,
        }
    }
}

// --- Tools ---

#[derive(Debug, Deserialize)]
pub struct RunToolRequest {
    pub project_id: Uuid,
    pub input: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ToolRunResponse {
    pub output: serde_json::Value,
    pub produced_artifacts: Vec<Uuid>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ToolSpecResponse {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub source: Option<String>,
}

// --- Members ---

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    /// UUID or "@all" for public access
    pub user_id: String,
    /// "manager", "collaborator", or "viewer"
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct ProjectMemberResponse {
    /// UUID or "@all" for sentinel
    pub user_id: String,
    pub role: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<af_db::project_members::ProjectMemberWithName> for ProjectMemberResponse {
    fn from(row: af_db::project_members::ProjectMemberWithName) -> Self {
        let user_id = if row.user_id == af_db::project_members::ALL_USERS_SENTINEL {
            "@all".to_string()
        } else {
            row.user_id.to_string()
        };
        Self {
            user_id,
            role: row.role,
            display_name: row.display_name,
            created_at: row.created_at,
        }
    }
}

// --- Audit ---

#[derive(Debug, Serialize)]
pub struct AuditLogResponse {
    pub id: Uuid,
    pub event_type: String,
    pub actor_subject: Option<String>,
    pub actor_user_id: Option<Uuid>,
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl From<af_db::audit_log::AuditLogRow> for AuditLogResponse {
    fn from(row: af_db::audit_log::AuditLogRow) -> Self {
        Self {
            id: row.id,
            event_type: row.event_type,
            actor_subject: row.actor_subject,
            actor_user_id: row.actor_user_id,
            detail: row.detail,
            created_at: row.created_at,
        }
    }
}

// --- Quotas ---

#[derive(Debug, Serialize)]
pub struct QuotaResponse {
    pub max_storage_bytes: i64,
    pub max_projects: i32,
    pub max_concurrent_runs: i32,
    pub max_llm_tokens_per_day: i64,
    pub max_upload_bytes: i64,
    pub max_vt_lookups_per_day: i32,
}

impl From<af_db::user_quotas::UserQuotaRow> for QuotaResponse {
    fn from(row: af_db::user_quotas::UserQuotaRow) -> Self {
        Self {
            max_storage_bytes: row.max_storage_bytes,
            max_projects: row.max_projects,
            max_concurrent_runs: row.max_concurrent_runs,
            max_llm_tokens_per_day: row.max_llm_tokens_per_day,
            max_upload_bytes: row.max_upload_bytes,
            max_vt_lookups_per_day: row.max_vt_lookups_per_day,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub llm_prompt_tokens: i64,
    pub llm_completion_tokens: i64,
    pub vt_lookups: i32,
    pub tool_runs: i32,
}

#[derive(Debug, Serialize)]
pub struct QuotaUsageResponse {
    pub quota: QuotaResponse,
    pub usage: UsageResponse,
    /// Allowed LLM routes. Empty = unrestricted (all models).
    pub allowed_routes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateQuotaRequest {
    pub max_storage_bytes: Option<i64>,
    pub max_projects: Option<i32>,
    pub max_concurrent_runs: Option<i32>,
    pub max_llm_tokens_per_day: Option<i64>,
    pub max_upload_bytes: Option<i64>,
    pub max_vt_lookups_per_day: Option<i32>,
}

// --- LLM ---

#[derive(Debug, Serialize)]
pub struct LlmBackendResponse {
    pub name: String,
    pub supports_tool_calls: bool,
    pub supports_streaming: bool,
    pub is_local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_mtok_input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_mtok_output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge_cutoff: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LlmBackendsResponse {
    pub backends: Vec<LlmBackendResponse>,
    pub routes: Vec<String>,
}

// --- Agents ---

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    #[serde(default = "default_route")]
    pub default_route: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub timeout_secs: Option<u32>,
}

fn default_route() -> String {
    "auto".to_string()
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub system_prompt: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub default_route: Option<String>,
    pub metadata: Option<serde_json::Value>,
    /// Absent/None = keep existing; 0 = clear timeout; 1..=86400 = set timeout.
    pub timeout_secs: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub name: String,
    pub system_prompt: String,
    pub allowed_tools: serde_json::Value,
    pub default_route: String,
    pub metadata: serde_json::Value,
    pub is_builtin: bool,
    pub source_plugin: Option<String>,
    pub timeout_secs: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<af_db::agents::AgentRow> for AgentResponse {
    fn from(row: af_db::agents::AgentRow) -> Self {
        Self {
            name: row.name,
            system_prompt: row.system_prompt,
            allowed_tools: row.allowed_tools,
            default_route: row.default_route,
            metadata: row.metadata,
            is_builtin: row.is_builtin,
            source_plugin: row.source_plugin,
            timeout_secs: row.timeout_secs,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

// --- Workflows ---

#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: Option<String>,
    pub steps: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkflowRequest {
    pub description: Option<String>,
    pub steps: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowResponse {
    pub name: String,
    pub description: Option<String>,
    pub steps: serde_json::Value,
    pub is_builtin: bool,
    pub source_plugin: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<af_db::workflows::WorkflowRow> for WorkflowResponse {
    fn from(row: af_db::workflows::WorkflowRow) -> Self {
        Self {
            name: row.name,
            description: row.description,
            steps: row.steps,
            is_builtin: row.is_builtin,
            source_plugin: row.source_plugin,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExecuteWorkflowRequest {
    pub workflow_name: String,
    pub content: String,
    /// Optional LLM route override (e.g. "backend:openai:gpt-4o-mini").
    /// If set, overrides each agent's default_route for this workflow run.
    pub route: Option<String>,
}

// --- Hooks ---

#[derive(Debug, Deserialize)]
pub struct CreateHookRequest {
    pub name: String,
    pub event_type: String,
    pub workflow_name: Option<String>,
    pub agent_name: Option<String>,
    pub prompt_template: String,
    pub route_override: Option<String>,
    pub tick_interval_minutes: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateHookRequest {
    pub enabled: Option<bool>,
    pub prompt_template: Option<String>,
    /// Use `Some(None)` to clear the route override, `Some(Some("..."))` to set it,
    /// `None` to leave unchanged.
    pub route_override: Option<Option<String>>,
    pub tick_interval_minutes: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct HookResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub event_type: String,
    pub workflow_name: Option<String>,
    pub agent_name: Option<String>,
    pub prompt_template: String,
    pub route_override: Option<String>,
    pub tick_interval_minutes: Option<i32>,
    pub last_tick_at: Option<DateTime<Utc>>,
    pub tick_generation: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<af_db::project_hooks::ProjectHookRow> for HookResponse {
    fn from(row: af_db::project_hooks::ProjectHookRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            name: row.name,
            enabled: row.enabled,
            event_type: row.event_type,
            workflow_name: row.workflow_name,
            agent_name: row.agent_name,
            prompt_template: row.prompt_template,
            route_override: row.route_override,
            tick_interval_minutes: row.tick_interval_minutes,
            last_tick_at: row.last_tick_at,
            tick_generation: row.tick_generation,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

// --- Plugins ---

#[derive(Debug, Serialize)]
pub struct PluginResponse {
    pub name: String,
    pub tools: Vec<String>,
    pub agents: Vec<String>,
    pub workflows: Vec<String>,
}

// --- Users (admin) ---

fn default_user_roles() -> Vec<String> {
    vec!["operator".into()]
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    #[serde(default = "default_user_roles")]
    pub roles: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<af_db::users::UserRow> for UserResponse {
    fn from(row: af_db::users::UserRow) -> Self {
        Self {
            id: row.id,
            subject: row.subject,
            display_name: row.display_name,
            email: row.email,
            roles: row.roles,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

// --- API Keys (admin) ---

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyResponse {
    pub id: Uuid,
    pub key_prefix: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<af_db::api_keys::ApiKeyRow> for ApiKeyResponse {
    fn from(row: af_db::api_keys::ApiKeyRow) -> Self {
        Self {
            id: row.id,
            key_prefix: row.key_prefix,
            name: row.name,
            scopes: row.scopes,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    pub id: Uuid,
    pub raw_key: String,
    pub key_prefix: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

// --- Cost ---

#[derive(Debug, Serialize)]
pub struct ModelCostBreakdown {
    pub route: String,
    pub model: String,
    pub call_count: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cached_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Estimated cost in USD. None if model is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ProjectCostResponse {
    pub project_id: Uuid,
    pub breakdown: Vec<ModelCostBreakdown>,
    pub total_prompt_tokens: i64,
    pub total_completion_tokens: i64,
    pub total_cached_read_tokens: i64,
    pub total_cache_creation_tokens: i64,
    /// Total estimated cost in USD. None if any model is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct MonthlyCostResponse {
    pub year: i32,
    pub month: u32,
    pub breakdown: Vec<ModelCostBreakdown>,
    /// Total estimated cost in USD. None if any model is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
}

// --- URL Ingest ---

#[derive(Debug, Deserialize)]
pub struct SubmitUrlsRequest {
    pub urls: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UrlIngestResponse {
    pub id: uuid::Uuid,
    pub url: String,
    pub status: String,
    pub title: Option<String>,
    pub chunk_count: Option<i32>,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<af_db::url_ingest::UrlIngestRow> for UrlIngestResponse {
    fn from(r: af_db::url_ingest::UrlIngestRow) -> Self {
        Self {
            id: r.id,
            url: r.url,
            status: r.status,
            title: r.title,
            chunk_count: r.chunk_count,
            error_message: r.error_message,
            created_at: r.created_at,
            completed_at: r.completed_at,
        }
    }
}

// --- User Allowed Routes ---

#[derive(Debug, Deserialize)]
pub struct AddRouteRequest {
    pub route: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveRouteRequest {
    pub route: Option<String>,
    #[serde(default)]
    pub clear: bool,
}

#[derive(Debug, Serialize)]
pub struct UserRoutesResponse {
    pub routes: Vec<String>,
    pub unrestricted: bool,
}

// --- Notifications ---

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub channel_type: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub config: serde_json::Value,
    /// If omitted, keeps the current enabled state.
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    pub id: uuid::Uuid,
    pub name: String,
    pub channel_type: String,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<af_db::notifications::NotificationChannelRow> for ChannelResponse {
    fn from(r: af_db::notifications::NotificationChannelRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            channel_type: r.channel_type,
            enabled: r.enabled,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NotificationQueueResponse {
    pub id: uuid::Uuid,
    pub project_id: uuid::Uuid,
    pub channel_id: uuid::Uuid,
    pub subject: String,
    pub status: String,
    pub error_message: Option<String>,
    pub attempt_count: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<af_db::notifications::NotificationQueueRow> for NotificationQueueResponse {
    fn from(r: af_db::notifications::NotificationQueueRow) -> Self {
        Self {
            id: r.id,
            project_id: r.project_id,
            channel_id: r.channel_id,
            subject: r.subject,
            status: r.status,
            error_message: r.error_message,
            attempt_count: r.attempt_count,
            created_at: r.created_at,
            completed_at: r.completed_at,
        }
    }
}
