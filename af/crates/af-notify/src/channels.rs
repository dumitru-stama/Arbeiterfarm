use af_db::notifications::{NotificationChannelRow, NotificationQueueRow};
use serde_json::json;
use std::path::Path;

/// Error wrapper that signals non-transient failures (should not be retried).
#[derive(Debug)]
pub struct PermanentError(pub String);

impl std::fmt::Display for PermanentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PermanentError {}

/// Check if an error is permanent (should not be retried).
pub fn is_permanent(e: &anyhow::Error) -> bool {
    e.downcast_ref::<PermanentError>().is_some()
}

/// Headers that must not be overridden via webhook config.
const BLOCKED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "proxy-authorization",
    "cookie",
];

/// Validate a URL is https:// (defense-in-depth, also checked at creation time).
fn require_https(url: &str, label: &str) -> anyhow::Result<()> {
    if !url.starts_with("https://") {
        anyhow::bail!("{label} URL must use https:// (got: {}...)", &url[..url.len().min(30)]);
    }
    Ok(())
}

/// Sanitize error messages to avoid leaking credentials.
fn sanitize_http_error(status: reqwest::StatusCode, body: &str) -> String {
    let preview: String = body.chars().take(200).collect();
    format!("HTTP {status}: {preview}")
}

/// Dispatch delivery based on channel type.
pub async fn deliver(
    pool: &sqlx::PgPool,
    storage_root: &Path,
    item: &NotificationQueueRow,
    channel: &NotificationChannelRow,
) -> anyhow::Result<()> {
    match channel.channel_type.as_str() {
        "webhook" => deliver_webhook(item, channel).await,
        "email" => deliver_email(item, channel).await,
        "matrix" => deliver_matrix(item, channel).await,
        "webdav" => deliver_webdav(pool, storage_root, item, channel).await,
        other => anyhow::bail!("unsupported channel type: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Webhook
// ---------------------------------------------------------------------------

async fn deliver_webhook(
    item: &NotificationQueueRow,
    channel: &NotificationChannelRow,
) -> anyhow::Result<()> {
    let config = &channel.config_json;
    let url = config["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("webhook config missing 'url'"))?;

    require_https(url, "webhook")?;

    let method = config["method"]
        .as_str()
        .unwrap_or("POST")
        .to_uppercase();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let body = json!({
        "subject": item.subject,
        "body": item.body,
        "project_id": item.project_id.to_string(),
        "timestamp": item.created_at.to_rfc3339(),
    });

    let mut request = match method.as_str() {
        "PUT" => client.put(url),
        _ => client.post(url),
    };

    request = request
        .header("Content-Type", "application/json")
        .json(&body);

    // Apply custom headers from config (with blocklist)
    if let Some(headers) = config["headers"].as_object() {
        for (key, val) in headers {
            if BLOCKED_HEADERS.contains(&key.to_lowercase().as_str()) {
                tracing::warn!(header = %key, channel = %channel.name, "blocked header in webhook config");
                continue;
            }
            if let Some(v) = val.as_str() {
                request = request.header(key.as_str(), v);
            }
        }
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        anyhow::bail!("webhook delivery failed: {}", sanitize_http_error(status, &body_text));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Email
// ---------------------------------------------------------------------------

async fn deliver_email(
    _item: &NotificationQueueRow,
    _channel: &NotificationChannelRow,
) -> anyhow::Result<()> {
    // Email delivery via notifications is not yet integrated.
    // Full email delivery requires af-email provider infrastructure.
    // Use webhook channels with an email bridge, or email.send tool directly.
    Err(PermanentError(
        "email channel delivery not yet integrated; use webhook channel with email bridge, \
         or configure email.send tool directly".to_string()
    ).into())
}

// ---------------------------------------------------------------------------
// Matrix
// ---------------------------------------------------------------------------

async fn deliver_matrix(
    item: &NotificationQueueRow,
    channel: &NotificationChannelRow,
) -> anyhow::Result<()> {
    let config = &channel.config_json;
    let homeserver = config["homeserver"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("matrix config missing 'homeserver'"))?;
    let room_id = config["room_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("matrix config missing 'room_id'"))?;
    let access_token = config["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("matrix config missing 'access_token'"))?;

    require_https(homeserver, "matrix homeserver")?;

    // Use notification queue ID as txn_id for idempotency
    let txn_id = item.id.to_string();

    // Percent-encode room_id for URL path safety (room IDs contain ! and :)
    let encoded_room_id: String = room_id
        .bytes()
        .flat_map(|b| {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    vec![b as char]
                }
                _ => format!("%{:02X}", b).chars().collect(),
            }
        })
        .collect();

    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
        homeserver.trim_end_matches('/'),
        encoded_room_id,
        txn_id,
    );

    let body_text = if item.body.is_empty() {
        item.subject.clone()
    } else {
        format!("{}\n\n{}", item.subject, item.body)
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let response = client
        .put(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .json(&json!({
            "msgtype": "m.text",
            "body": body_text,
        }))
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        anyhow::bail!("matrix delivery failed: {}", sanitize_http_error(status, &body_text));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// WebDAV
// ---------------------------------------------------------------------------

/// Max artifact size for WebDAV upload (256 MB).
const WEBDAV_MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;

async fn deliver_webdav(
    pool: &sqlx::PgPool,
    storage_root: &Path,
    item: &NotificationQueueRow,
    channel: &NotificationChannelRow,
) -> anyhow::Result<()> {
    let config = &channel.config_json;
    let base_url = config["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("webdav config missing 'url'"))?
        .trim_end_matches('/');
    let username = config["username"].as_str().unwrap_or("");
    let password = config["password"].as_str().unwrap_or("");

    require_https(base_url, "webdav")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    if let Some(artifact_id) = item.attachment_artifact_id {
        // Upload artifact blob
        let artifact = af_db::artifacts::get_artifact(pool, artifact_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("artifact not found"))?;

        let blob_path = storage_root
            .join(&artifact.sha256[..2])
            .join(&artifact.sha256);

        // Check file size before reading into memory
        let metadata = tokio::fs::metadata(&blob_path).await.map_err(|e| {
            anyhow::anyhow!("failed to read artifact blob: {e}")
        })?;
        if metadata.len() > WEBDAV_MAX_UPLOAD_BYTES {
            anyhow::bail!(
                "artifact too large for WebDAV upload ({} bytes, max {})",
                metadata.len(),
                WEBDAV_MAX_UPLOAD_BYTES
            );
        }

        let data = tokio::fs::read(&blob_path).await.map_err(|e| {
            anyhow::anyhow!("failed to read artifact blob: {e}")
        })?;

        // Sanitize filename — only allow safe characters
        let safe_filename: String = artifact.filename
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let url = format!("{base_url}/{safe_filename}");

        let mut req = client.put(&url).body(data);
        if !username.is_empty() {
            req = req.basic_auth(username, Some(password));
        }

        let response = req.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("webdav upload failed: {}", sanitize_http_error(status, &body));
        }
    } else {
        // Upload text notification as .txt file
        let slug: String = item
            .subject
            .chars()
            .take(40)
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let ts = item.created_at.format("%Y%m%d_%H%M%S");
        let filename = format!("{slug}_{ts}.txt");
        let content = format!("{}\n\n{}", item.subject, item.body);

        let url = format!("{base_url}/{filename}");

        let mut req = client
            .put(&url)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(content);
        if !username.is_empty() {
            req = req.basic_auth(username, Some(password));
        }

        let response = req.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("webdav text upload failed: {}", sanitize_http_error(status, &body));
        }
    }

    Ok(())
}
