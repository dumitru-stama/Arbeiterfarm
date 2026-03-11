use async_trait::async_trait;
use af_db::{
    agents::AgentRow,
    api_keys::ApiKeyRow,
    artifacts::ArtifactRow,
    audit_log::AuditLogRow,
    messages::MessageRow,
    project_hooks::ProjectHookRow,
    project_members::{ProjectMemberWithName, ALL_USERS_SENTINEL},
    projects::ProjectRow,
    threads::ThreadRow,
    users::UserRow,
    workflows::WorkflowRow,
};
use reqwest::Client;
use uuid::Uuid;

use super::Backend;

pub struct RemoteApi {
    client: Client,
    base_url: String,
    api_key: String,
}

impl RemoteApi {
    pub fn new(base_url: &str, api_key: &str, allow_insecure: bool) -> anyhow::Result<Self> {
        let trimmed = base_url.trim_end_matches('/');
        if !allow_insecure && !trimmed.starts_with("https://") {
            anyhow::bail!(
                "refusing to send API key over plaintext HTTP. \
                 Use https:// or pass --allow-insecure to override."
            );
        }
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            base_url: trimmed.to_string(),
            api_key: api_key.to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url, path)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.api_key)
    }

    async fn check_response(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // Try to extract error message from JSON
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(msg) = v.get("error").and_then(|e| e.as_str()) {
                    anyhow::bail!("API error {status}: {msg}");
                }
            }
            anyhow::bail!("API error {status}: {body}");
        }
    }
}

// --- Helper response types for API responses that don't match Row types ---

#[derive(serde::Deserialize)]
struct MemberResponse {
    user_id: String,
    role: String,
    display_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
struct UploadResponse {
    id: Uuid,
    sha256: String,
    filename: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
struct CreateApiKeyResponse {
    id: Uuid,
    raw_key: String,
    key_prefix: String,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
struct UserRoutesApiResponse {
    routes: Vec<String>,
}

#[async_trait]
impl Backend for RemoteApi {
    fn is_local(&self) -> bool {
        false
    }

    // --- Projects ---

    async fn create_project(&self, name: &str) -> anyhow::Result<ProjectRow> {
        let resp = self
            .auth(self.client.post(self.url("/projects")))
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn list_projects(&self) -> anyhow::Result<Vec<ProjectRow>> {
        let resp = self
            .auth(self.client.get(self.url("/projects")))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn delete_project(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/projects/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Members ---

    async fn list_members(&self, project_id: Uuid) -> anyhow::Result<Vec<ProjectMemberWithName>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/projects/{project_id}/members"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        let items: Vec<MemberResponse> = resp.json().await?;
        Ok(items
            .into_iter()
            .map(|m| {
                let user_id = if m.user_id == "@all" {
                    ALL_USERS_SENTINEL
                } else {
                    m.user_id.parse().unwrap_or(ALL_USERS_SENTINEL)
                };
                ProjectMemberWithName {
                    project_id,
                    user_id,
                    role: m.role,
                    display_name: m.display_name,
                    created_at: m.created_at,
                }
            })
            .collect())
    }

    async fn add_member(
        &self,
        project_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> anyhow::Result<()> {
        let user_str = if user_id == ALL_USERS_SENTINEL {
            "@all".to_string()
        } else {
            user_id.to_string()
        };
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/projects/{project_id}/members"))),
            )
            .json(&serde_json::json!({ "user_id": user_str, "role": role }))
            .send()
            .await?;
        Self::check_response(resp).await?;
        Ok(())
    }

    async fn remove_member(&self, project_id: Uuid, user_id: Uuid) -> anyhow::Result<()> {
        let resp = self
            .auth(self.client.delete(
                self.url(&format!("/projects/{project_id}/members/{user_id}")),
            ))
            .send()
            .await?;
        Self::check_response(resp).await?;
        Ok(())
    }

    // --- Artifacts ---

    async fn upload_artifact(
        &self,
        project_id: Uuid,
        filename: &str,
        data: &[u8],
    ) -> anyhow::Result<ArtifactRow> {
        let form = reqwest::multipart::Form::new()
            .text("project_id", project_id.to_string())
            .part(
                "file",
                reqwest::multipart::Part::bytes(data.to_vec())
                    .file_name(filename.to_string()),
            );

        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/projects/{project_id}/artifacts"))),
            )
            .multipart(form)
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;

        // API returns UploadArtifactResponse (subset of ArtifactRow fields).
        // Construct a minimal ArtifactRow from it.
        let upload: UploadResponse = resp.json().await?;
        Ok(ArtifactRow {
            id: upload.id,
            project_id,
            sha256: upload.sha256,
            filename: upload.filename,
            mime_type: None,
            source_tool_run_id: None,
            metadata: serde_json::json!({}),
            description: None,
            created_at: upload.created_at,
        })
    }

    async fn list_artifacts(&self, project_id: Uuid) -> anyhow::Result<Vec<ArtifactRow>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/projects/{project_id}/artifacts"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_artifact(&self, id: Uuid) -> anyhow::Result<Option<ArtifactRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/artifacts/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    async fn update_artifact_description(
        &self,
        id: Uuid,
        desc: &str,
    ) -> anyhow::Result<Option<ArtifactRow>> {
        let resp = self
            .auth(self.client.patch(self.url(&format!("/artifacts/{id}"))))
            .json(&serde_json::json!({ "description": desc }))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    async fn delete_artifact(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/artifacts/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    async fn delete_generated_artifacts(&self, project_id: Uuid) -> anyhow::Result<u64> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/projects/{project_id}/artifacts/generated"))))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        let body: serde_json::Value = resp.json().await?;
        Ok(body.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0))
    }

    // --- Conversations ---

    async fn list_threads(&self, project_id: Uuid) -> anyhow::Result<Vec<ThreadRow>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/projects/{project_id}/threads"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_thread_messages(&self, thread_id: Uuid) -> anyhow::Result<Vec<MessageRow>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/threads/{thread_id}/messages"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn export_thread(&self, thread_id: Uuid, format: &str) -> anyhow::Result<String> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/threads/{thread_id}/export")))
                    .query(&[("format", format)]),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.text().await?)
    }

    async fn delete_thread(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/threads/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    async fn queue_message(&self, thread_id: Uuid, content: &str) -> anyhow::Result<MessageRow> {
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/threads/{thread_id}/messages/queue")))
                    .json(&serde_json::json!({ "content": content })),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    // --- Agents ---

    async fn list_agents(&self) -> anyhow::Result<Vec<AgentRow>> {
        let resp = self
            .auth(self.client.get(self.url("/agents")))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_agent(&self, name: &str) -> anyhow::Result<Option<AgentRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/agents/{name}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    async fn upsert_agent(
        &self,
        name: &str,
        prompt: &str,
        tools: &serde_json::Value,
        route: &str,
        metadata: &serde_json::Value,
        _is_builtin: bool,
        _source: Option<&str>,
        timeout_secs: Option<i32>,
    ) -> anyhow::Result<AgentRow> {
        let body = serde_json::json!({
            "name": name,
            "system_prompt": prompt,
            "allowed_tools": tools,
            "default_route": route,
            "metadata": metadata,
            "timeout_secs": timeout_secs,
        });
        let resp = self
            .auth(self.client.post(self.url("/agents")))
            .json(&body)
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn delete_agent(&self, name: &str) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/agents/{name}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Workflows ---

    async fn list_workflows(&self) -> anyhow::Result<Vec<WorkflowRow>> {
        let resp = self
            .auth(self.client.get(self.url("/workflows")))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_workflow(&self, name: &str) -> anyhow::Result<Option<WorkflowRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/workflows/{name}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    // --- Hooks ---

    async fn list_hooks(&self, project_id: Uuid) -> anyhow::Result<Vec<ProjectHookRow>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/projects/{project_id}/hooks"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
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
        let body = serde_json::json!({
            "name": name,
            "event_type": event,
            "workflow_name": workflow,
            "agent_name": agent,
            "prompt_template": prompt,
            "route_override": route,
            "tick_interval_minutes": interval,
        });
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/projects/{project_id}/hooks"))),
            )
            .json(&body)
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_hook(&self, id: Uuid) -> anyhow::Result<Option<ProjectHookRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/hooks/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    async fn update_hook(
        &self,
        id: Uuid,
        enabled: Option<bool>,
        prompt: Option<&str>,
        route: Option<Option<&str>>,
        interval: Option<i32>,
    ) -> anyhow::Result<Option<ProjectHookRow>> {
        let body = serde_json::json!({
            "enabled": enabled,
            "prompt_template": prompt,
            "route_override": route,
            "tick_interval_minutes": interval,
        });
        let resp = self
            .auth(self.client.put(self.url(&format!("/hooks/{id}"))))
            .json(&body)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    async fn delete_hook(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/hooks/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Audit ---

    async fn list_audit(
        &self,
        limit: i64,
        event_type: Option<&str>,
    ) -> anyhow::Result<Vec<AuditLogRow>> {
        let mut params = vec![("limit", limit.to_string())];
        if let Some(et) = event_type {
            params.push(("type", et.to_string()));
        }
        let resp = self
            .auth(self.client.get(self.url("/audit")).query(&params))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    // --- Users ---

    async fn create_user(
        &self,
        subject: &str,
        display: Option<&str>,
        email: Option<&str>,
        roles: &[String],
    ) -> anyhow::Result<UserRow> {
        let body = serde_json::json!({
            "subject": subject,
            "display_name": display,
            "email": email,
            "roles": roles,
        });
        let resp = self
            .auth(self.client.post(self.url("/admin/users")))
            .json(&body)
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn list_users(&self) -> anyhow::Result<Vec<UserRow>> {
        let resp = self
            .auth(self.client.get(self.url("/admin/users")))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/admin/users/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    // --- API Keys ---

    async fn create_api_key(
        &self,
        user_id: Uuid,
        name: &str,
    ) -> anyhow::Result<(String, ApiKeyRow)> {
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/admin/users/{user_id}/api_keys"))),
            )
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        let create_resp: CreateApiKeyResponse = resp.json().await?;

        let row = ApiKeyRow {
            id: create_resp.id,
            user_id,
            key_hash: String::new(),
            key_prefix: create_resp.key_prefix,
            name: create_resp.name,
            scopes: vec!["all".to_string()],
            expires_at: None,
            last_used_at: None,
            created_at: create_resp.created_at,
        };
        Ok((create_resp.raw_key, row))
    }

    async fn list_api_keys(&self, user_id: Uuid) -> anyhow::Result<Vec<ApiKeyRow>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/admin/users/{user_id}/api_keys"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn revoke_api_key(&self, key_id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(
                self.client
                    .delete(self.url(&format!("/admin/api_keys/{key_id}"))),
            )
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Project Settings ---

    async fn get_project_settings(&self, project_id: Uuid) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/projects/{project_id}/settings"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn update_project_settings(
        &self,
        project_id: Uuid,
        settings: &serde_json::Value,
    ) -> anyhow::Result<ProjectRow> {
        let resp = self
            .auth(
                self.client
                    .patch(self.url(&format!("/projects/{project_id}/settings"))),
            )
            .json(settings)
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn set_nda(&self, project_id: Uuid, nda: bool) -> anyhow::Result<(ProjectRow, bool)> {
        // Fetch the project to read current NDA value (dedicated column, not in settings JSONB).
        // The server-side set_nda handles audit trail and no-op detection.
        let proj_resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/projects/{project_id}"))),
            )
            .send()
            .await?;
        let proj_resp = Self::check_response(proj_resp).await?;
        let current: ProjectRow = proj_resp.json().await?;
        let old_nda = current.nda;

        let resp = self
            .auth(
                self.client
                    .patch(self.url(&format!("/projects/{project_id}/settings"))),
            )
            .json(&serde_json::json!({ "nda": nda }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        let row: ProjectRow = resp.json().await?;
        Ok((row, old_nda))
    }

    // --- User Allowed Routes ---

    async fn list_user_routes(&self, user_id: Uuid) -> anyhow::Result<Vec<String>> {
        let resp = self
            .auth(
                self.client
                    .get(self.url(&format!("/admin/users/{user_id}/routes"))),
            )
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        let body: UserRoutesApiResponse = resp.json().await?;
        Ok(body.routes)
    }

    async fn add_user_route(&self, user_id: Uuid, route: &str) -> anyhow::Result<()> {
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("/admin/users/{user_id}/routes"))),
            )
            .json(&serde_json::json!({ "route": route }))
            .send()
            .await?;
        Self::check_response(resp).await?;
        Ok(())
    }

    async fn remove_user_route(&self, user_id: Uuid, route: &str) -> anyhow::Result<bool> {
        let resp = self
            .auth(
                self.client
                    .delete(self.url(&format!("/admin/users/{user_id}/routes"))),
            )
            .json(&serde_json::json!({ "route": route }))
            .send()
            .await?;
        Self::check_response(resp).await?;
        Ok(true)
    }

    async fn clear_user_routes(&self, user_id: Uuid) -> anyhow::Result<u64> {
        let resp = self
            .auth(
                self.client
                    .delete(self.url(&format!("/admin/users/{user_id}/routes"))),
            )
            .json(&serde_json::json!({ "clear": true }))
            .send()
            .await?;
        Self::check_response(resp).await?;
        Ok(0) // remote doesn't return count
    }

    // --- Web Fetch Rules ---

    async fn list_web_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<af_db::web_fetch::WebFetchRuleRow>> {
        let url = match project_id {
            Some(pid) => self.url(&format!("/web-rules?project_id={pid}")),
            None => self.url("/web-rules"),
        };
        let resp = self.auth(self.client.get(url)).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
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
        let resp = self
            .auth(self.client.post(self.url("/web-rules")))
            .json(&serde_json::json!({
                "scope": scope,
                "project_id": project_id,
                "rule_type": rule_type,
                "pattern_type": pattern_type,
                "pattern": pattern,
                "description": description,
            }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn remove_web_rule(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/web-rules/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    async fn list_country_blocks(&self) -> anyhow::Result<Vec<af_db::web_fetch::CountryBlockRow>> {
        let resp = self.auth(self.client.get(self.url("/web-rules/countries"))).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn add_country_block(&self, code: &str, name: Option<&str>) -> anyhow::Result<af_db::web_fetch::CountryBlockRow> {
        let resp = self
            .auth(self.client.post(self.url("/web-rules/countries")))
            .json(&serde_json::json!({
                "country_code": code,
                "country_name": name,
            }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn remove_country_block(&self, code: &str) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/web-rules/countries/{code}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Restricted Tools ---

    async fn list_restricted_tools(&self) -> anyhow::Result<Vec<af_db::restricted_tools::RestrictedToolRow>> {
        let resp = self.auth(self.client.get(self.url("/admin/restricted-tools"))).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn add_restricted_tool(&self, pattern: &str, description: &str) -> anyhow::Result<af_db::restricted_tools::RestrictedToolRow> {
        let resp = self
            .auth(self.client.post(self.url("/admin/restricted-tools")))
            .json(&serde_json::json!({
                "tool_pattern": pattern,
                "description": description,
            }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn remove_restricted_tool(&self, pattern: &str) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url("/admin/restricted-tools")))
            .json(&serde_json::json!({ "tool_pattern": pattern }))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    async fn list_user_grants(&self, user_id: Uuid) -> anyhow::Result<Vec<af_db::restricted_tools::UserToolGrantRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/admin/users/{user_id}/tool-grants"))))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn add_user_grant(&self, user_id: Uuid, pattern: &str) -> anyhow::Result<af_db::restricted_tools::UserToolGrantRow> {
        let resp = self
            .auth(self.client.post(self.url(&format!("/admin/users/{user_id}/tool-grants"))))
            .json(&serde_json::json!({ "tool_pattern": pattern }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn remove_user_grant(&self, user_id: Uuid, pattern: &str) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/admin/users/{user_id}/tool-grants"))))
            .json(&serde_json::json!({ "tool_pattern": pattern }))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Email Recipient Rules ---

    async fn list_email_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<af_db::email::EmailRecipientRuleRow>> {
        let url = match project_id {
            Some(pid) => self.url(&format!("/email-rules?project_id={pid}")),
            None => self.url("/email-rules"),
        };
        let resp = self.auth(self.client.get(url)).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
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
        let resp = self
            .auth(self.client.post(self.url("/email-rules")))
            .json(&serde_json::json!({
                "scope": scope,
                "project_id": project_id,
                "rule_type": rule_type,
                "pattern_type": pattern_type,
                "pattern": pattern,
                "description": description,
            }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn remove_email_rule(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/email-rules/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
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
        let resp = self
            .auth(self.client.post(self.url(&format!("/admin/users/{user_id}/email-credentials"))))
            .json(&serde_json::json!({
                "provider": provider,
                "email_address": email_address,
                "credentials_json": credentials_json,
                "is_default": is_default,
            }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn list_email_credentials(&self, user_id: Uuid) -> anyhow::Result<Vec<af_db::email::EmailCredentialRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/admin/users/{user_id}/email-credentials"))))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn delete_email_credential(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/admin/email-credentials/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Email Tone Presets ---

    async fn list_email_tone_presets(&self) -> anyhow::Result<Vec<af_db::email::EmailTonePresetRow>> {
        let resp = self
            .auth(self.client.get(self.url("/email-tones")))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn upsert_email_tone_preset(
        &self,
        name: &str,
        description: Option<&str>,
        system_instruction: &str,
    ) -> anyhow::Result<af_db::email::EmailTonePresetRow> {
        let resp = self
            .auth(self.client.post(self.url("/email-tones")))
            .json(&serde_json::json!({
                "name": name,
                "description": description,
                "system_instruction": system_instruction,
            }))
            .send()
            .await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn delete_email_tone_preset(&self, name: &str) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/email-tones/{name}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- Email Scheduling ---

    async fn list_scheduled_emails(
        &self,
        project_id: Option<Uuid>,
        status: Option<&str>,
    ) -> anyhow::Result<Vec<af_db::email::EmailScheduledRow>> {
        let mut url = self.url("/email-scheduled");
        let mut params = vec![];
        if let Some(pid) = project_id {
            params.push(format!("project_id={pid}"));
        }
        if let Some(s) = status {
            params.push(format!("status={s}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let resp = self.auth(self.client.get(url)).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn cancel_scheduled_email(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.post(self.url(&format!("/email-scheduled/{id}/cancel"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    // --- YARA Rules ---

    async fn list_yara_rules(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<af_db::yara::YaraRuleRow>> {
        let url = match project_id {
            Some(pid) => self.url(&format!("/admin/yara/rules?project_id={pid}")),
            None => self.url("/admin/yara/rules"),
        };
        let resp = self.auth(self.client.get(url)).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn get_yara_rule(&self, id: Uuid) -> anyhow::Result<Option<af_db::yara::YaraRuleRow>> {
        let resp = self
            .auth(self.client.get(self.url(&format!("/admin/yara/rules/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = Self::check_response(resp).await?;
        Ok(Some(resp.json().await?))
    }

    async fn remove_yara_rule(&self, id: Uuid) -> anyhow::Result<bool> {
        let resp = self
            .auth(self.client.delete(self.url(&format!("/admin/yara/rules/{id}"))))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_response(resp).await?;
        Ok(true)
    }

    async fn list_yara_scan_results(
        &self,
        artifact_id: Option<Uuid>,
        rule_name: Option<&str>,
    ) -> anyhow::Result<Vec<af_db::yara::YaraScanResultRow>> {
        let mut url = self.url("/admin/yara/scan-results");
        let mut params = vec![];
        if let Some(aid) = artifact_id {
            params.push(format!("artifact_id={aid}"));
        }
        if let Some(rn) = rule_name {
            params.push(format!("rule_name={rn}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let resp = self.auth(self.client.get(url)).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json().await?)
    }
}
