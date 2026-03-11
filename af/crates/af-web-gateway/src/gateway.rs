use crate::geoip::GeoIpChecker;
use crate::rate_limiter::RateLimiter;
use crate::rules::{self, RuleResult};
use crate::ssrf;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Configuration for the web gateway daemon.
pub struct WebGatewayConfig {
    pub socket_path: PathBuf,
    pub pool: PgPool,
    pub geoip: Option<GeoIpChecker>,
    pub rate_limit_rpm: u32,
    pub per_user_rpm: u32,
    pub cache_ttl_secs: i64,
    pub max_response_bytes: usize,
    pub max_redirects: u32,
    pub fetch_timeout_secs: u64,
}

/// The gateway daemon.
pub struct WebGateway {
    socket_path: PathBuf,
    pool: PgPool,
    geoip: Option<GeoIpChecker>,
    rate_limiter: Arc<RateLimiter>,
    per_user_limiters: DashMap<String, Arc<RateLimiter>>,
    per_user_rpm: u32,
    max_tracked_users: usize,
    fetch_timeout_secs: u64,
    /// Client without DNS pinning (for DuckDuckGo search and other trusted targets).
    http_client_no_resolve: reqwest::Client,
    cache_ttl: i64,
    max_response_bytes: usize,
    max_redirects: u32,
}

#[derive(Deserialize)]
struct GatewayRequest {
    action: String,
    url: Option<String>,
    query: Option<String>,
    user_id: Option<String>,
    project_id: Option<String>,
}

#[derive(Serialize)]
struct GatewayResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl GatewayResponse {
    fn success(data: serde_json::Value, cached: bool) -> Self {
        Self {
            ok: true,
            data: Some(data),
            cached: Some(cached),
            error: None,
            message: None,
        }
    }

    fn error(code: &str, message: String) -> Self {
        Self {
            ok: false,
            data: None,
            cached: None,
            error: Some(code.to_string()),
            message: Some(message),
        }
    }
}

impl WebGateway {
    pub fn new(config: WebGatewayConfig) -> Self {
        Self {
            socket_path: config.socket_path,
            pool: config.pool,
            geoip: config.geoip,
            rate_limiter: Arc::new(RateLimiter::new(config.rate_limit_rpm)),
            per_user_limiters: DashMap::new(),
            per_user_rpm: config.per_user_rpm,
            max_tracked_users: 10_000,
            fetch_timeout_secs: config.fetch_timeout_secs,
            http_client_no_resolve: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(config.fetch_timeout_secs))
                .connect_timeout(Duration::from_secs(10))
                .user_agent("af-web-gateway/1.0")
                .build()
                .expect("failed to build reqwest client"),
            cache_ttl: config.cache_ttl_secs,
            max_response_bytes: config.max_response_bytes,
            max_redirects: config.max_redirects,
        }
    }

    /// Build a reqwest client that pins resolved IPs for a specific host,
    /// preventing DNS rebinding between our check and the actual connection.
    fn build_pinned_client(&self, host: &str, port: u16, addrs: &[std::net::IpAddr]) -> reqwest::Client {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(self.fetch_timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("af-web-gateway/1.0");
        // Pin each resolved IP so reqwest connects to the exact addresses we validated
        for ip in addrs {
            let socket_addr = std::net::SocketAddr::new(*ip, port);
            builder = builder.resolve(host, socket_addr);
        }
        builder.build().expect("failed to build pinned reqwest client")
    }

    /// Resolve DNS and validate all IPs against SSRF, GeoIP, and URL rules.
    /// Returns the resolved IPs on success, or a GatewayResponse error.
    async fn resolve_and_validate(
        &self,
        parsed: &url::Url,
        rules: &[af_db::web_fetch::WebFetchRuleRow],
    ) -> Result<Vec<std::net::IpAddr>, GatewayResponse> {
        let host = parsed.host_str().ok_or_else(|| {
            GatewayResponse::error("invalid_url", "URL has no host".into())
        })?;
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::IpAddr> =
            match tokio::net::lookup_host(format!("{}:{}", host, port)).await {
                Ok(a) => a.map(|a| a.ip()).collect(),
                Err(e) => {
                    return Err(GatewayResponse::error(
                        "dns_error",
                        format!("DNS resolution failed for {host}: {e}"),
                    ))
                }
            };
        if addrs.is_empty() {
            return Err(GatewayResponse::error(
                "dns_error",
                format!("no addresses found for {host}"),
            ));
        }
        // SSRF check
        for ip in &addrs {
            if ssrf::is_private_ip(ip) {
                return Err(GatewayResponse::error("ssrf_blocked", ssrf::ssrf_reason(ip)));
            }
        }
        // GeoIP check
        if let Some(ref geoip) = self.geoip {
            for ip in &addrs {
                if let Some(country) = geoip.check(ip) {
                    return Err(GatewayResponse::error(
                        "country_blocked",
                        format!("IP {} is in blocked country: {country}", ip),
                    ));
                }
            }
        }
        // IP-based URL rules
        match rules::evaluate_rules(parsed, &addrs, rules) {
            RuleResult::Blocked(reason) => {
                return Err(GatewayResponse::error(
                    "url_blocked",
                    format!(
                        "URL blocked by IP rule: {}",
                        reason.unwrap_or_else(|| "blocked".into())
                    ),
                ))
            }
            RuleResult::Allowed => {}
        }
        Ok(addrs)
    }

    pub async fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        // Remove stale socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        if let Some(parent) = self.socket_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let listener = UnixListener::bind(&self.socket_path)
            .expect("failed to bind web gateway UDS");

        // Set permissions 0o660
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.socket_path,
                std::fs::Permissions::from_mode(0o660),
            );
        }

        tracing::info!("web gateway listening on {}", self.socket_path.display());

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let gw = Arc::clone(&self);
                        tokio::spawn(async move {
                            if let Err(e) = gw.handle_connection(stream).await {
                                tracing::warn!("web gateway connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("web gateway accept error: {e}");
                    }
                }
            }
        })
    }

    async fn handle_connection(
        &self,
        stream: tokio::net::UnixStream,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        // Limit request line to 1MB to prevent OOM from malicious UDS clients
        let mut line = String::new();
        let max_line_bytes: usize = 1024 * 1024;
        loop {
            let buf = reader.fill_buf().await?;
            if buf.is_empty() {
                break; // EOF
            }
            if let Some(newline_pos) = buf.iter().position(|&b| b == b'\n') {
                let chunk = &buf[..=newline_pos];
                line.push_str(&String::from_utf8_lossy(chunk));
                let consumed = newline_pos + 1;
                reader.consume(consumed);
                break;
            } else {
                let chunk_len = buf.len();
                line.push_str(&String::from_utf8_lossy(buf));
                reader.consume(chunk_len);
            }
            if line.len() > max_line_bytes {
                return Err("request line too large".into());
            }
        }

        let request: GatewayRequest = serde_json::from_str(line.trim())?;
        let response = self.process_request(request).await;
        let json = serde_json::to_string(&response)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.shutdown().await?;
        Ok(())
    }

    async fn process_request(&self, req: GatewayRequest) -> GatewayResponse {
        match req.action.as_str() {
            "fetch" => self.handle_fetch(req).await,
            "search" => self.handle_search(req).await,
            other => GatewayResponse::error("unknown_action", format!("unknown action: {other}")),
        }
    }

    async fn handle_fetch(&self, req: GatewayRequest) -> GatewayResponse {
        let url_str = match req.url {
            Some(ref u) if !u.is_empty() => u.clone(),
            _ => return GatewayResponse::error("invalid_input", "url is required".into()),
        };

        // Parse URL
        let parsed = match url::Url::parse(&url_str) {
            Ok(u) => u,
            Err(e) => return GatewayResponse::error("invalid_url", format!("invalid URL: {e}")),
        };

        // Reject non-HTTP(S)
        match parsed.scheme() {
            "http" | "https" => {}
            other => {
                return GatewayResponse::error(
                    "invalid_scheme",
                    format!("only http and https are allowed, got: {other}"),
                )
            }
        }

        if parsed.host_str().is_none() {
            return GatewayResponse::error("invalid_url", "URL has no host".into());
        }

        // Per-user rate limit
        if let Err(e) = self.check_per_user_rate(&req.user_id).await {
            return GatewayResponse::error("rate_limited", format!("{e}"));
        }

        // Load URL rules (fail-closed: DB error → block)
        let project_id = req
            .project_id
            .as_deref()
            .and_then(|s| uuid::Uuid::parse_str(s).ok());
        let rules = match af_db::web_fetch::list_rules(&self.pool, None, project_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to load URL rules: {e}");
                return GatewayResponse::error(
                    "internal_error",
                    "failed to load URL rules".into(),
                );
            }
        };

        // Check URL rules (before DNS — no reason to resolve if the URL itself is blocked)
        match rules::evaluate_rules(&parsed, &[], &rules) {
            RuleResult::Blocked(reason) => {
                return GatewayResponse::error(
                    "url_blocked",
                    format!(
                        "URL blocked by rule: {}",
                        reason.unwrap_or_else(|| "no matching allow rule".into())
                    ),
                )
            }
            RuleResult::Allowed => {}
        }

        // Cache check (key on final URL computed after fetch, so use original for lookup)
        let url_hash = hex::encode(Sha256::digest(parsed.as_str().as_bytes()));
        if let Ok(Some(cached)) = af_db::web_fetch::cache_get(&self.pool, &url_hash).await {
            // Only serve cache for successful responses (don't cache errors permanently)
            if cached.status_code >= 200 && cached.status_code < 400 {
                let data = serde_json::json!({
                    "url": cached.url,
                    "status": cached.status_code,
                    "content_type": cached.content_type,
                    "body": cached.body,
                    "body_length": cached.body.as_ref().map(|b| b.len()).unwrap_or(0),
                    "truncated": false,
                });
                return GatewayResponse::success(data, true);
            }
        }

        // DNS resolution + SSRF/GeoIP/IP-rule validation (reusable helper)
        let addrs = match self.resolve_and_validate(&parsed, &rules).await {
            Ok(a) => a,
            Err(resp) => return resp,
        };

        // Global rate limit
        if let Err(e) = self.rate_limiter.acquire(Duration::from_secs(30)).await {
            return GatewayResponse::error("rate_limited", format!("global: {e}"));
        }

        // Build a pinned client so reqwest connects to the exact IPs we validated
        // (prevents DNS rebinding between our check and the actual connection)
        let host = parsed.host_str().unwrap(); // checked above
        let port = parsed.port_or_known_default().unwrap_or(443);
        let client = self.build_pinned_client(host, port, &addrs);

        // HTTP fetch with redirect handling — full SSRF re-check on each hop
        let mut current_url = parsed.clone();
        let mut redirect_count = 0;
        let mut current_client = client;
        let response = loop {
            let resp = match current_client.get(current_url.as_str()).send().await {
                Ok(r) => r,
                Err(e) => {
                    return GatewayResponse::error(
                        "fetch_error",
                        format!("HTTP request failed: {e}"),
                    )
                }
            };

            if resp.status().is_redirection() {
                redirect_count += 1;
                if redirect_count > self.max_redirects {
                    return GatewayResponse::error(
                        "too_many_redirects",
                        format!("exceeded {} redirects", self.max_redirects),
                    );
                }
                if let Some(loc) = resp.headers().get("location") {
                    let loc_str = match loc.to_str() {
                        Ok(s) => s,
                        Err(_) => {
                            return GatewayResponse::error(
                                "fetch_error",
                                "invalid redirect location header".into(),
                            )
                        }
                    };
                    let next_url = match current_url.join(loc_str) {
                        Ok(u) => u,
                        Err(e) => {
                            return GatewayResponse::error(
                                "fetch_error",
                                format!("invalid redirect URL: {e}"),
                            )
                        }
                    };
                    // Full security re-check on redirect target
                    match next_url.scheme() {
                        "http" | "https" => {}
                        other => {
                            return GatewayResponse::error(
                                "invalid_scheme",
                                format!("redirect to non-HTTP scheme: {other}"),
                            )
                        }
                    }
                    // Re-check URL rules on the redirect target
                    match rules::evaluate_rules(&next_url, &[], &rules) {
                        RuleResult::Blocked(reason) => {
                            return GatewayResponse::error(
                                "url_blocked",
                                format!(
                                    "redirect URL blocked by rule: {}",
                                    reason.unwrap_or_else(|| "no matching allow rule".into())
                                ),
                            )
                        }
                        RuleResult::Allowed => {}
                    }
                    // DNS resolution + SSRF + GeoIP + IP rules on redirect target
                    let redir_addrs = match self.resolve_and_validate(&next_url, &rules).await {
                        Ok(a) => a,
                        Err(resp) => return resp,
                    };
                    // Build a new pinned client for the redirect target
                    let redir_host = next_url.host_str().unwrap_or("");
                    let redir_port = next_url.port_or_known_default().unwrap_or(443);
                    current_client = self.build_pinned_client(redir_host, redir_port, &redir_addrs);
                    current_url = next_url;
                    continue;
                }
            }

            break resp;
        };

        let status = response.status().as_u16() as i32;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Read body with streaming size cap (prevents OOM on huge responses)
        let mut body_bytes = Vec::with_capacity(
            std::cmp::min(self.max_response_bytes + 1, 1024 * 1024),
        );
        let mut truncated = false;
        {
            use futures_util::StreamExt;
            let mut stream = response.bytes_stream();
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        let remaining = self.max_response_bytes.saturating_sub(body_bytes.len());
                        if remaining == 0 {
                            truncated = true;
                            break;
                        }
                        let take = std::cmp::min(chunk.len(), remaining);
                        body_bytes.extend_from_slice(&chunk[..take]);
                        if take < chunk.len() {
                            truncated = true;
                            break;
                        }
                    }
                    Err(e) => {
                        return GatewayResponse::error(
                            "fetch_error",
                            format!("failed to read response body: {e}"),
                        )
                    }
                }
            }
        }

        // Content processing: HTML → plaintext
        let is_html = content_type
            .as_deref()
            .map(|ct| ct.contains("text/html"))
            .unwrap_or(false);

        let body_text = if is_html {
            let text = html2text::from_read(&body_bytes[..], 120);
            let title = extract_html_title(&body_bytes);
            if let Some(ref t) = title {
                format!("Title: {t}\n\n{text}")
            } else {
                text
            }
        } else {
            String::from_utf8_lossy(&body_bytes).to_string()
        };

        let body_length = body_text.len();

        // Cache store (only cache successful responses)
        if status >= 200 && status < 400 {
            if let Err(e) = af_db::web_fetch::cache_put(
                &self.pool,
                &url_hash,
                current_url.as_str(),
                status,
                content_type.as_deref(),
                Some(&body_text),
                None,
                self.cache_ttl,
            )
            .await
            {
                tracing::warn!("failed to write web fetch cache: {e}");
            }
        }

        let data = serde_json::json!({
            "url": current_url.as_str(),
            "status": status,
            "content_type": content_type,
            "body": body_text,
            "body_length": body_length,
            "truncated": truncated,
        });

        GatewayResponse::success(data, false)
    }

    async fn handle_search(&self, req: GatewayRequest) -> GatewayResponse {
        let query = match req.query {
            Some(ref q) if !q.is_empty() => q.clone(),
            _ => return GatewayResponse::error("invalid_input", "query is required".into()),
        };

        // Truncate query (char-safe to avoid UTF-8 panic)
        let query: String = query.chars().take(200).collect();

        // Per-user rate limit
        if let Err(e) = self.check_per_user_rate(&req.user_id).await {
            return GatewayResponse::error("rate_limited", format!("{e}"));
        }

        // Global rate limit
        if let Err(e) = self.rate_limiter.acquire(Duration::from_secs(30)).await {
            return GatewayResponse::error("rate_limited", format!("global: {e}"));
        }

        // Fetch DuckDuckGo HTML
        let ddg_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&query)
        );

        let resp = match self.http_client_no_resolve.get(&ddg_url).send().await {
            Ok(r) => r,
            Err(e) => {
                return GatewayResponse::error(
                    "search_error",
                    format!("DuckDuckGo request failed: {e}"),
                )
            }
        };

        // Read search response with 2MB cap (DuckDuckGo pages are typically small)
        let max_search_bytes = 2 * 1024 * 1024;
        let body = {
            use futures_util::StreamExt;
            let mut buf = Vec::with_capacity(64 * 1024);
            let mut stream = resp.bytes_stream();
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buf.extend_from_slice(&chunk);
                        if buf.len() > max_search_bytes {
                            buf.truncate(max_search_bytes);
                            break;
                        }
                    }
                    Err(e) => {
                        return GatewayResponse::error(
                            "search_error",
                            format!("failed to read search response: {e}"),
                        )
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        };

        // Parse results from HTML (simplified extraction)
        let results = parse_ddg_results(&body);

        let data = serde_json::json!({
            "query": query,
            "results": results,
            "total": results.len(),
        });

        GatewayResponse::success(data, false)
    }

    async fn check_per_user_rate(
        &self,
        user_id: &Option<String>,
    ) -> Result<(), crate::rate_limiter::RateLimitError> {
        let uid = match user_id {
            Some(u) if !u.is_empty() => u.clone(),
            _ => return Ok(()), // no user = no per-user limit
        };
        // Cap map size: evict oldest half instead of clearing all (prevents reset attacks)
        if self.per_user_limiters.len() >= self.max_tracked_users {
            let keys: Vec<String> = self.per_user_limiters.iter().map(|e| e.key().clone()).collect();
            // Evict the first half of keys (arbitrary but stable, avoids resetting everyone)
            for key in keys.iter().take(keys.len() / 2) {
                self.per_user_limiters.remove(key);
            }
        }
        let limiter = self
            .per_user_limiters
            .entry(uid)
            .or_insert_with(|| Arc::new(RateLimiter::new(self.per_user_rpm)))
            .clone();
        limiter.acquire(Duration::from_secs(5)).await
    }
}

/// Extract <title> from HTML bytes (case-insensitive, byte-safe).
fn extract_html_title(html: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(html).ok()?;
    // Case-insensitive search directly on the original string to avoid
    // byte-offset mismatches between lowercased and original (multi-byte chars).
    let lower = text.as_bytes();
    let open_tag = b"<title>";
    let close_tag = b"</title>";
    let start = lower
        .windows(open_tag.len())
        .position(|w| w.eq_ignore_ascii_case(open_tag))?;
    let after = start + open_tag.len();
    let end = lower[after..]
        .windows(close_tag.len())
        .position(|w| w.eq_ignore_ascii_case(close_tag))?;
    let title = &text[after..after + end];
    Some(title.trim().to_string())
}

/// Parse DuckDuckGo HTML search results.
fn parse_ddg_results(html: &str) -> Vec<serde_json::Value> {
    let mut results = Vec::new();

    // DuckDuckGo lite uses <a class="result__a"> for links
    // and <a class="result__snippet"> for snippets.
    // Simplified regex-free parsing.
    for chunk in html.split("class=\"result__a\"") {
        if results.len() >= 20 {
            break;
        }
        // Extract href
        let href = if let Some(start) = chunk.find("href=\"") {
            let rest = &chunk[start + 6..];
            if let Some(end) = rest.find('"') {
                let url = &rest[..end];
                // DuckDuckGo wraps URLs in redirects, extract actual URL
                if let Some(ud_start) = url.find("uddg=") {
                    let encoded = &url[ud_start + 5..];
                    let decoded = urlencoding::decode(encoded).unwrap_or_default().to_string();
                    // Strip anything after & in the decoded URL
                    decoded.split('&').next().unwrap_or(&decoded).to_string()
                } else {
                    url.to_string()
                }
            } else {
                continue;
            }
        } else {
            continue;
        };

        // Skip DDG internal links
        if href.contains("duckduckgo.com") || href.is_empty() {
            continue;
        }

        // Extract title text (between > and </a>)
        let title = if let Some(start) = chunk.find('>') {
            let rest = &chunk[start + 1..];
            if let Some(end) = rest.find("</a>") {
                strip_html_tags(&rest[..end]).trim().to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Extract snippet
        let snippet = if let Some(snip_start) = chunk.find("class=\"result__snippet\"") {
            let rest = &chunk[snip_start..];
            if let Some(tag_end) = rest.find('>') {
                let inner = &rest[tag_end + 1..];
                if let Some(end) = inner.find("</") {
                    strip_html_tags(&inner[..end]).trim().to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if !title.is_empty() || !href.is_empty() {
            results.push(serde_json::json!({
                "title": title,
                "url": href,
                "snippet": snippet,
            }));
        }
    }

    results
}

fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }
    result
}

// Need urlencoding for DuckDuckGo URL encoding — use a simple implementation
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 3);
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push('%');
                    result.push_str(&format!("{:02X}", b));
                }
            }
        }
        result
    }

    pub fn decode(s: &str) -> Result<String, ()> {
        let mut result = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| ())?;
                let byte = u8::from_str_radix(hex, 16).map_err(|_| ())?;
                result.push(byte);
                i += 3;
            } else if bytes[i] == b'+' {
                result.push(b' ');
                i += 1;
            } else {
                result.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(result).map_err(|_| ())
    }
}
