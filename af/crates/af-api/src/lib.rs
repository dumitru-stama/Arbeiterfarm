pub mod auth;
pub mod dto;
pub mod error;
pub mod hooks;
pub mod rate_limit;
pub mod routes;
pub mod source_map;

pub use source_map::SourceMap;

use axum::routing::{delete, get, patch, post};
use axum::Router;
use af_core::{
    AgentConfig, CoreConfig, EvidenceResolverRegistry, PostToolHook, ToolExecutorRegistry,
    ToolSpecRegistry,
};
use af_llm::LlmRouter;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

/// Security posture visible via the health endpoint.
pub struct SecurityConfig {
    pub sandbox_available: bool,
    pub sandbox_enforced: bool,
    pub tls_enabled: bool,
}

/// Per-user SSE stream counter with RAII guard.
pub struct ActiveStreamTracker {
    counts: Mutex<HashMap<Uuid, Arc<AtomicU32>>>,
    pub max_per_user: u32,
}

impl ActiveStreamTracker {
    pub fn new(max_per_user: u32) -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
            max_per_user,
        }
    }

    /// Acquire a stream slot. Returns a guard that decrements on drop.
    pub async fn acquire(&self, user_id: Uuid) -> Result<StreamGuard, ()> {
        let mut map = self.counts.lock().await;
        let counter = map
            .entry(user_id)
            .or_insert_with(|| Arc::new(AtomicU32::new(0)));
        let current = counter.load(Ordering::Relaxed);
        if current >= self.max_per_user {
            return Err(());
        }
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(StreamGuard {
            counter: counter.clone(),
        })
    }
}

/// RAII guard that decrements the stream counter when dropped.
pub struct StreamGuard {
    counter: Arc<AtomicU32>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct AppState {
    pub pool: PgPool,
    pub specs: Arc<ToolSpecRegistry>,
    pub executors: Arc<ToolExecutorRegistry>,
    pub evidence_resolvers: Arc<EvidenceResolverRegistry>,
    pub post_tool_hook: Option<Arc<dyn PostToolHook>>,
    pub core_config: CoreConfig,
    pub agent_configs: Vec<AgentConfig>,
    pub router: Arc<LlmRouter>,
    pub upload_max_bytes: usize,
    pub rate_limiter: Option<Arc<rate_limit::ApiRateLimiter>>,
    /// Allowed CORS origin. None = no CORS headers (same-origin only).
    pub cors_origin: Option<String>,
    /// Path to Ghidra analysis cache directory (for project downloads).
    pub ghidra_cache_dir: Option<std::path::PathBuf>,
    /// Tracks the origin plugin/source for every tool, agent, and workflow.
    pub source_map: SourceMap,
    /// Security posture for health endpoint.
    pub security_config: SecurityConfig,
    /// Per-user concurrent SSE stream limiter.
    pub stream_tracker: ActiveStreamTracker,
    /// Compaction config: threshold + optional summarization backend.
    pub compaction_threshold: f32,
    pub summarization_backend: Option<Arc<dyn af_llm::LlmBackend>>,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    // Body limit: upload_max_bytes + 1MB overhead for multipart headers/framing
    let body_limit = state.upload_max_bytes + 1_048_576;
    let cors_origin = state.cors_origin.clone();
    let shared = state;

    let api = Router::new()
        // Health (no auth)
        .route("/health", get(routes::health::health))
        .route("/health/security", get(routes::health::security))
        // LLM backends
        .route("/llm/backends", get(routes::llm::list_backends))
        // Projects
        .route("/projects", post(routes::projects::create).get(routes::projects::list))
        .route("/projects/{id}", get(routes::projects::get_one).delete(routes::projects::delete))
        .route(
            "/projects/{id}/settings",
            get(routes::projects::get_settings).patch(routes::projects::update_settings),
        )
        // Members
        .route(
            "/projects/{id}/members",
            post(routes::members::add).get(routes::members::list),
        )
        .route(
            "/projects/{id}/members/{user_id}",
            delete(routes::members::remove),
        )
        // Artifacts
        .route(
            "/projects/{id}/artifacts",
            post(routes::artifacts::upload).get(routes::artifacts::list),
        )
        .route("/artifacts/{id}", patch(routes::artifacts::update_description).delete(routes::artifacts::delete))
        .route("/artifacts/{id}/download", get(routes::artifacts::download))
        .route("/artifacts/{id}/ghidra-project", get(routes::artifacts::download_ghidra_project))
        .route("/projects/{id}/artifacts/generated", delete(routes::artifacts::delete_generated))
        // Threads
        .route(
            "/projects/{id}/threads",
            post(routes::threads::create).get(routes::threads::list),
        )
        .route("/threads/{id}", delete(routes::threads::delete))
        .route("/threads/{id}/export", get(routes::threads::export))
        .route("/threads/{id}/children", get(routes::threads::children))
        // Thinking threads
        .route("/projects/{id}/thinking", post(routes::thinking::start))
        .route("/threads/{id}/thinking", post(routes::thinking::run))
        // Messages
        .route(
            "/threads/{id}/messages",
            post(routes::messages::send_sse).get(routes::messages::list),
        )
        .route(
            "/threads/{id}/messages/sync",
            post(routes::messages::send_sync),
        )
        .route("/threads/{id}/messages/queue", post(routes::messages::queue_message))
        .route("/threads/{id}/prompt-preview", get(routes::messages::prompt_preview))
        // Workflow execution on thread
        .route("/threads/{id}/workflow", post(routes::workflows::execute))
        // Tools
        .route("/tools", get(routes::tools::list))
        .route("/tools/{name}/run", post(routes::tools::run))
        // Plugins
        .route("/plugins", get(routes::plugins::list))
        // Agents
        .route("/agents", post(routes::agents::create).get(routes::agents::list))
        .route(
            "/agents/{name}",
            get(routes::agents::get_one)
                .put(routes::agents::update)
                .delete(routes::agents::delete),
        )
        // Workflows
        .route("/workflows", post(routes::workflows::create).get(routes::workflows::list))
        .route(
            "/workflows/{name}",
            get(routes::workflows::get_one)
                .put(routes::workflows::update)
                .delete(routes::workflows::delete),
        )
        // Hooks
        .route(
            "/projects/{id}/hooks",
            post(routes::hooks::create).get(routes::hooks::list),
        )
        .route(
            "/hooks/{id}",
            get(routes::hooks::get_one)
                .put(routes::hooks::update)
                .delete(routes::hooks::delete),
        )
        // URL Ingest
        .route(
            "/projects/{id}/url-ingest",
            post(routes::url_ingest::submit).get(routes::url_ingest::list),
        )
        .route(
            "/projects/{id}/url-ingest/{queue_id}",
            delete(routes::url_ingest::cancel),
        )
        .route(
            "/projects/{id}/url-ingest/{queue_id}/retry",
            post(routes::url_ingest::retry),
        )
        // Audit
        .route("/audit", get(routes::audit::list))
        // Cost
        .route("/projects/{id}/cost", get(routes::projects::cost))
        .route("/cost/monthly", get(routes::quota::monthly_cost))
        // Quota
        .route("/quota", get(routes::quota::get_own))
        .route("/admin/quota/{user_id}", get(routes::quota::admin_get).put(routes::quota::admin_update))
        // Admin: Users
        .route("/admin/users", get(routes::admin::list_users).post(routes::admin::create_user))
        .route("/admin/users/{id}", get(routes::admin::get_user))
        .route("/admin/users/{id}/api_keys", post(routes::admin::create_key).get(routes::admin::list_keys))
        .route("/admin/api_keys/{id}", delete(routes::admin::revoke_key))
        // Admin: User allowed routes
        .route(
            "/admin/users/{id}/routes",
            get(routes::admin::list_user_routes)
                .post(routes::admin::add_user_route)
                .delete(routes::admin::remove_user_route),
        )
        // Web Rules
        .route("/web-rules", get(routes::web_rules::list_rules).post(routes::web_rules::add_rule))
        .route("/web-rules/{id}", delete(routes::web_rules::remove_rule))
        .route(
            "/web-rules/countries",
            get(routes::web_rules::list_countries).post(routes::web_rules::add_country),
        )
        .route("/web-rules/countries/{code}", delete(routes::web_rules::remove_country))
        // Admin: Restricted Tools
        .route(
            "/admin/restricted-tools",
            get(routes::admin::list_restricted_tools)
                .post(routes::admin::add_restricted_tool)
                .delete(routes::admin::remove_restricted_tool),
        )
        // Admin: User Tool Grants
        .route(
            "/admin/users/{id}/tool-grants",
            get(routes::admin::list_user_grants)
                .post(routes::admin::add_user_grant)
                .delete(routes::admin::remove_user_grant),
        )
        // Admin: Email Management
        .route("/admin/email/credentials", get(routes::email_mgmt::list_credentials))
        .route("/admin/email/credentials/{id}", delete(routes::email_mgmt::remove_credential))
        .route(
            "/admin/email/rules",
            get(routes::email_mgmt::list_rules).post(routes::email_mgmt::add_rule),
        )
        .route("/admin/email/rules/{id}", delete(routes::email_mgmt::remove_rule))
        .route(
            "/admin/email/tones",
            get(routes::email_mgmt::list_tones).post(routes::email_mgmt::upsert_tone),
        )
        .route("/admin/email/tones/{name}", delete(routes::email_mgmt::remove_tone))
        .route("/admin/email/scheduled", get(routes::email_mgmt::list_scheduled))
        .route("/admin/email/scheduled/{id}", delete(routes::email_mgmt::cancel_scheduled))
        // Notification Channels
        .route(
            "/projects/{id}/notification-channels",
            post(routes::notifications::create_channel).get(routes::notifications::list_channels),
        )
        .route(
            "/projects/{id}/notification-channels/{ch_id}",
            axum::routing::put(routes::notifications::update_channel)
                .delete(routes::notifications::delete_channel),
        )
        .route(
            "/projects/{id}/notification-channels/{ch_id}/test",
            post(routes::notifications::test_channel),
        )
        // Notification Queue
        .route(
            "/projects/{id}/notifications",
            get(routes::notifications::list_queue),
        )
        .route(
            "/projects/{id}/notifications/{queue_id}",
            delete(routes::notifications::cancel_notification),
        )
        .route(
            "/projects/{id}/notifications/{queue_id}/retry",
            post(routes::notifications::retry_notification),
        )
        // Admin: Embed Queue
        .route("/admin/embed-queue", get(routes::embed_queue::list))
        .route("/admin/embed-queue/{id}", delete(routes::embed_queue::cancel))
        .route("/admin/embed-queue/{id}/retry", post(routes::embed_queue::retry))
        // Admin: YARA Rules
        .route("/admin/yara/rules", get(routes::yara_rules::list_rules))
        .route(
            "/admin/yara/rules/{id}",
            get(routes::yara_rules::get_rule).delete(routes::yara_rules::remove_rule),
        )
        .route("/admin/yara/scan-results", get(routes::yara_rules::list_scan_results))
        // Rate limiting middleware
        .layer(axum::middleware::from_fn_with_state(
            shared.clone(),
            rate_limit::rate_limit_middleware,
        ))
        .with_state(shared);

    let mut outer = Router::new()
        .nest("/api/v1", api)
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(TraceLayer::new_for_http());

    // CORS: only enabled if explicitly configured via cors_origin
    if let Some(ref origin) = cors_origin {
        if origin == "*" {
            outer = outer.layer(CorsLayer::permissive());
        } else {
            outer = outer.layer(
                CorsLayer::new()
                    .allow_origin(origin.parse::<axum::http::HeaderValue>()
                        .expect("CORS origin already validated at startup"))
                    .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::PUT, axum::http::Method::DELETE, axum::http::Method::PATCH])
                    .allow_headers([axum::http::header::AUTHORIZATION, axum::http::header::CONTENT_TYPE]),
            );
        }
    }

    // Static UI (no-build) served from repo-root ui/ directory.
    // /ui/* requests serve static assets; all other unmatched paths get index.html (SPA).
    let outer = outer.nest_service("/ui", ServeDir::new("ui"));
    outer.fallback_service(ServeFile::new("ui/index.html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::postgres::PgPoolOptions;
    use std::path::PathBuf;
    use tower::ServiceExt;

    /// Build a test AppState with a lazy pool (never actually connects to DB).
    /// Auth-rejection tests fail at the header-parsing stage before the pool is used.
    fn test_state() -> AppState {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://fake:fake@localhost/fake")
            .expect("connect_lazy should not fail");
        AppState {
            pool,
            specs: Arc::new(ToolSpecRegistry::new()),
            executors: Arc::new(ToolExecutorRegistry::new()),
            evidence_resolvers: Arc::new(EvidenceResolverRegistry::new()),
            post_tool_hook: None,
            core_config: CoreConfig {
                storage_root: PathBuf::from("/tmp/af-test/storage"),
                scratch_root: PathBuf::from("/tmp/af-test/scratch"),
                use_oaie: false,
            },
            agent_configs: vec![],
            router: Arc::new(LlmRouter::new()),
            upload_max_bytes: 1024,
            rate_limiter: None,
            cors_origin: None,
            ghidra_cache_dir: None,
            source_map: SourceMap::default(),
            security_config: SecurityConfig {
                sandbox_available: false,
                sandbox_enforced: false,
                tls_enabled: false,
            },
            stream_tracker: ActiveStreamTracker::new(5),
            compaction_threshold: 0.85,
            summarization_backend: None,
        }
    }

    fn test_router() -> Router {
        build_router(Arc::new(test_state()))
    }

    #[tokio::test]
    async fn test_health_no_auth() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_missing_auth_returns_401() {
        let app = test_router();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("Authorization"));
    }

    #[tokio::test]
    async fn test_bad_bearer_returns_401() {
        let app = test_router();

        // "Basic" scheme instead of "Bearer" — fails at header parsing, no DB hit
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects")
                    .header("Authorization", "Basic dXNlcjpwYXNz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("Bearer"));
    }
}
