use crate::cache::VtCache;
use crate::rate_limiter::RateLimiter;
use af_plugin_api::PluginDb;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// VT gateway daemon. Listens on a Unix Domain Socket, enforces rate limits,
/// caches responses, and proxies hash lookups to the VirusTotal API.
pub struct VtGateway {
    socket_path: PathBuf,
    api_key: String,
    cache: VtCache,
    rate_limiter: Arc<RateLimiter>,
    http_client: reqwest::Client,
    per_user_limiters: DashMap<String, Arc<RateLimiter>>,
    per_user_rpm: u32,
    max_tracked_users: usize,
}

#[derive(Debug, Deserialize)]
struct GatewayRequest {
    action: String,
    sha256: Option<String>,
    user_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct GatewayResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// Build a normalized error response. Logs details server-side, returns generic message to client.
fn error_response(client_code: &str, client_msg: &str, log_detail: &str) -> GatewayResponse {
    eprintln!("[vt-gateway] {client_code}: {log_detail}");
    GatewayResponse {
        ok: false,
        data: None,
        cached: None,
        error: Some(client_code.to_string()),
        message: Some(client_msg.to_string()),
    }
}

impl VtGateway {
    pub fn new(
        socket_path: PathBuf,
        api_key: String,
        plugin_db: Arc<dyn PluginDb>,
        requests_per_minute: u32,
        cache_ttl: Duration,
    ) -> Self {
        Self {
            socket_path,
            api_key,
            cache: VtCache::new(plugin_db, cache_ttl),
            rate_limiter: Arc::new(RateLimiter::new(requests_per_minute)),
            http_client: reqwest::Client::new(),
            per_user_limiters: DashMap::new(),
            per_user_rpm: requests_per_minute.max(1),
            max_tracked_users: 10_000,
        }
    }

    /// Override the maximum number of tracked per-user rate limiters (default: 10,000).
    pub fn with_max_tracked_users(mut self, max: usize) -> Self {
        self.max_tracked_users = max;
        self
    }

    /// Start the gateway. Returns a JoinHandle for shutdown.
    /// Creates the UDS, listens for connections, processes requests.
    pub async fn start(self) -> tokio::task::JoinHandle<()> {
        // Remove stale socket file if it exists
        let _ = tokio::fs::remove_file(&self.socket_path).await;

        // Ensure parent directory exists
        if let Some(parent) = self.socket_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let listener = UnixListener::bind(&self.socket_path)
            .expect("failed to bind VT gateway socket");

        // Socket permissions: owner + group read/write (bwrap runs as same user)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.socket_path,
                std::fs::Permissions::from_mode(0o660),
            );
        }

        let gateway = Arc::new(self);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let gw = Arc::clone(&gateway);
                        tokio::spawn(async move {
                            if let Err(e) = gw.handle_connection(stream).await {
                                eprintln!("[vt-gateway] connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("[vt-gateway] accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        })
    }

    async fn handle_connection(
        &self,
        stream: tokio::net::UnixStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;

        let req = match serde_json::from_str::<GatewayRequest>(&line) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("[vt-gateway] dropping connection: invalid request: {e}");
                return Ok(());
            }
        };
        let response = self.process_request(req).await;

        let resp_json = serde_json::to_string(&response)?;
        writer.write_all(resp_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.shutdown().await?;

        Ok(())
    }

    async fn process_request(&self, req: GatewayRequest) -> GatewayResponse {
        match req.action.as_str() {
            "file_report" => {
                let sha256 = match req.sha256 {
                    Some(h) if !h.is_empty() => h,
                    _ => {
                        return error_response(
                            "error",
                            "invalid request",
                            "missing sha256 field",
                        );
                    }
                };
                self.handle_file_report(&sha256, req.user_id.as_deref()).await
            }
            other => error_response(
                "error",
                "invalid request",
                &format!("unknown action: {other}"),
            ),
        }
    }

    async fn handle_file_report(&self, sha256: &str, user_id: Option<&str>) -> GatewayResponse {
        // 0. Per-user rate limit (before cache, so abusive users can't even flood cache lookups)
        if let Some(uid) = user_id {
            // Validate user_id is a UUID to prevent map-stuffing attacks
            if uuid::Uuid::parse_str(uid).is_err() {
                return error_response(
                    "error",
                    "invalid request",
                    &format!("invalid user_id: not a UUID"),
                );
            }
            // Cap map size to prevent unbounded growth from distinct user IDs
            if !self.per_user_limiters.contains_key(uid) && self.per_user_limiters.len() >= self.max_tracked_users {
                return error_response(
                    "rate_limited",
                    "rate limit exceeded",
                    "per-user limiter map full",
                );
            }
            let limiter = self
                .per_user_limiters
                .entry(uid.to_string())
                .or_insert_with(|| Arc::new(RateLimiter::new(self.per_user_rpm)))
                .clone();
            if let Err(e) = limiter.acquire(Duration::from_secs(5)).await {
                return error_response(
                    "rate_limited",
                    "rate limit exceeded",
                    &format!("per-user: {e}"),
                );
            }
        }

        // 1. Check cache
        match self.cache.get(sha256).await {
            Ok(Some(cached_response)) => {
                return GatewayResponse {
                    ok: true,
                    data: Some(cached_response),
                    cached: Some(true),
                    error: None,
                    message: None,
                };
            }
            Ok(None) => {} // not cached, continue
            Err(e) => {
                eprintln!("[vt-gateway] cache lookup failed: {e}");
                // Continue without cache — still try the API
            }
        }

        // 2. Rate limit
        if let Err(e) = self.rate_limiter.acquire(Duration::from_secs(30)).await {
            return error_response(
                "rate_limited",
                "rate limit exceeded",
                &format!("global: {e}"),
            );
        }

        // 3. Call VT API
        let url = format!("https://www.virustotal.com/api/v3/files/{sha256}");
        let api_result = self
            .http_client
            .get(&url)
            .header("x-apikey", &self.api_key)
            .timeout(Duration::from_secs(20))
            .send()
            .await;

        let resp = match api_result {
            Ok(r) => r,
            Err(e) => {
                return error_response(
                    "upstream_error",
                    "external lookup failed",
                    &format!("request failed: {e}"),
                );
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return GatewayResponse {
                ok: true,
                data: None,
                cached: Some(false),
                error: None,
                message: Some("Hash not found in VirusTotal database".to_string()),
            };
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return error_response(
                "upstream_error",
                "external lookup failed",
                &format!("HTTP {status}: {body}"),
            );
        }

        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    "upstream_error",
                    "external lookup failed",
                    &format!("parse error: {e}"),
                );
            }
        };

        // 4. Extract summary from VT v3 response
        let summary = extract_vt_summary(sha256, &body);

        // 5. Cache the result
        let positives = summary.get("positives").and_then(|v| v.as_i64());
        let total = summary.get("total").and_then(|v| v.as_i64());
        if let Err(e) = self
            .cache
            .put(sha256, &summary, positives, total)
            .await
        {
            eprintln!("[vt-gateway] cache store failed: {e}");
        }

        GatewayResponse {
            ok: true,
            data: Some(summary),
            cached: Some(false),
            error: None,
            message: None,
        }
    }
}

/// Extract a concise summary from the VT v3 API response.
fn extract_vt_summary(sha256: &str, body: &Value) -> Value {
    let attrs = &body["data"]["attributes"];
    let stats = &attrs["last_analysis_stats"];

    let positives = stats["malicious"].as_i64().unwrap_or(0)
        + stats["suspicious"].as_i64().unwrap_or(0);
    let total = stats["malicious"].as_i64().unwrap_or(0)
        + stats["suspicious"].as_i64().unwrap_or(0)
        + stats["undetected"].as_i64().unwrap_or(0)
        + stats["harmless"].as_i64().unwrap_or(0)
        + stats["timeout"].as_i64().unwrap_or(0)
        + stats["failure"].as_i64().unwrap_or(0);

    let tags = attrs["tags"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
        .unwrap_or_default();

    let first_seen = attrs["first_submission_date"]
        .as_i64()
        .map(|ts| {
            chrono::DateTime::from_timestamp(ts, 0)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    // Extract top family names from popular_threat_classification
    let top_families: Vec<String> = attrs["popular_threat_classification"]["popular_threat_name"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(5)
                .filter_map(|v| v["value"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let permalink = format!("https://www.virustotal.com/gui/file/{sha256}");

    json!({
        "sha256": sha256,
        "detection_ratio": format!("{positives}/{total}"),
        "positives": positives,
        "total": total,
        "first_seen": first_seen,
        "tags": tags,
        "top_families": top_families,
        "permalink": permalink,
    })
}
