//! URL ingestion queue processor.
//!
//! Called from `af tick` to process pending url_ingest_queue items.
//! Fetches URLs, converts HTML to text, chunks, stores as artifacts,
//! and enqueues for background embedding via embed_queue.

use crate::chunking;
use sqlx::PgPool;
use std::path::Path;

/// Max response body size (5 MB).
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

/// How many URLs to process per tick (URL fetching is slow).
const BATCH_SIZE: i64 = 5;

/// How long (minutes) before a 'processing' item is considered stuck.
const STALE_PROCESSING_MINUTES: i32 = 2;

/// Process pending URL ingestion queue items. Returns the number completed.
pub async fn process_url_queue(
    pool: &PgPool,
    storage_root: &Path,
) -> anyhow::Result<u64> {
    // Recover stale items stuck in 'processing' from crashed tick workers.
    let recovered = af_db::url_ingest::recover_stale(pool, STALE_PROCESSING_MINUTES).await?;
    if !recovered.is_empty() {
        let retried = recovered.iter().filter(|r| r.status == "pending").count();
        let failed = recovered.iter().filter(|r| r.status == "failed").count();
        if retried > 0 {
            eprintln!("[url-ingest] recovered {retried} stale item(s) for retry");
        }
        if failed > 0 {
            eprintln!("[url-ingest] permanently failed {failed} item(s) after exhausting retries");
        }
    }

    let pending = af_db::url_ingest::list_pending(pool, BATCH_SIZE).await?;
    let mut completed = 0u64;

    // Build a shared reqwest client for all items in this tick
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("af-url-ingest/1.0")
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    for item in &pending {
        if !af_db::url_ingest::claim(pool, item.id).await? {
            continue;
        }

        match process_single_url(pool, storage_root, &client, item).await {
            Ok(()) => {
                completed += 1;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                eprintln!("[url-ingest] failed to process {}: {msg}", item.url);
                af_db::url_ingest::fail(pool, item.id, &msg).await?;
            }
        }
    }

    Ok(completed)
}

async fn process_single_url(
    pool: &PgPool,
    storage_root: &Path,
    client: &reqwest::Client,
    item: &af_db::url_ingest::UrlIngestRow,
) -> anyhow::Result<()> {
    // Validate URL scheme
    if !item.url.starts_with("http://") && !item.url.starts_with("https://") {
        return Err(anyhow::anyhow!("invalid URL scheme: must be http or https"));
    }

    // Auto-upgrade HTTP to HTTPS
    let fetch_url = if item.url.starts_with("http://") {
        item.url.replacen("http://", "https://", 1)
    } else {
        item.url.clone()
    };

    // Fetch the URL
    let response = client
        .get(&fetch_url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("fetch failed: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP {status}"));
    }

    // Read body with streaming size limit.
    // Cannot trust Content-Length alone — chunked transfers may omit it.
    // Stream chunks into a Vec with a hard cap to prevent OOM.
    if let Some(len) = response.content_length() {
        if len as usize > MAX_BODY_BYTES {
            return Err(anyhow::anyhow!(
                "response too large: {len} bytes (max {MAX_BODY_BYTES})"
            ));
        }
    }

    let mut body = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read response body: {e}"))?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(anyhow::anyhow!(
                "response too large: exceeded {MAX_BODY_BYTES} bytes"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let bytes = body;

    // Extract title from HTML
    let html_str = String::from_utf8_lossy(&bytes);
    let title = extract_title(&html_str);

    // Convert HTML to text
    let text = html2text::from_read(&bytes[..], 120)
        .map_err(|e| anyhow::anyhow!("html2text conversion failed: {e}"))?;
    if text.trim().is_empty() {
        return Err(anyhow::anyhow!("page has no text content"));
    }

    let text_bytes = text.as_bytes();
    let text_len = text_bytes.len() as i32;

    // Store text as blob + artifact
    let (sha256, _path) = af_storage::blob_store::store_blob(pool, storage_root, text_bytes)
        .await
        .map_err(|e| anyhow::anyhow!("failed to store text blob: {e}"))?;

    let text_filename = url_to_filename(title.as_deref(), &item.url);
    let text_artifact = af_db::artifacts::create_artifact_with_description(
        pool,
        item.project_id,
        &sha256,
        &text_filename,
        Some("text/plain"),
        None,
        &item.url,
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to create text artifact: {e}"))?;

    // Chunk the text
    let label_prefix = text_filename
        .strip_suffix(".txt")
        .unwrap_or(&text_filename);
    let chunks = chunking::chunk_text(&text, 1000, 200, label_prefix);
    let chunk_count = chunks.len() as i32;

    // Store chunks as blob + artifact
    let chunks_json = serde_json::to_string_pretty(&chunks)
        .map_err(|e| anyhow::anyhow!("failed to serialize chunks: {e}"))?;

    let (chunks_sha, _chunks_path) =
        af_storage::blob_store::store_blob(pool, storage_root, chunks_json.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("failed to store chunks blob: {e}"))?;

    let chunks_filename = format!(
        "{}_chunks.json",
        text_filename.strip_suffix(".txt").unwrap_or(&text_filename)
    );
    let chunks_artifact = af_db::artifacts::create_artifact_with_description(
        pool,
        item.project_id,
        &chunks_sha,
        &chunks_filename,
        Some("application/json"),
        None,
        &format!("{chunk_count} chunks from {}", item.url),
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to create chunks artifact: {e}"))?;

    // Enqueue for background embedding
    if let Err(e) = af_db::embed_queue::enqueue(
        pool,
        item.project_id,
        chunks_artifact.id,
        Some(text_artifact.id),
        "url.ingest",
    )
    .await
    {
        eprintln!(
            "[url-ingest] WARNING: failed to enqueue for embedding (non-fatal): {e}"
        );
    }

    // Mark completed
    af_db::url_ingest::complete(
        pool,
        item.id,
        title.as_deref(),
        text_len,
        text_artifact.id,
        chunks_artifact.id,
        chunk_count,
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to mark completed: {e}"))?;

    Ok(())
}

/// Extract the <title> content from HTML.
/// Uses case-insensitive search directly on the original string to avoid
/// byte offset mismatches from to_lowercase() on non-ASCII content.
fn extract_title(html: &str) -> Option<String> {
    // Case-insensitive search for <title by scanning the original string
    let html_bytes = html.as_bytes();
    let needle = b"<title";
    let start = html_bytes
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))?;
    let rest = &html[start + needle.len()..];
    let gt = rest.find('>')?;
    let after_tag = &rest[gt + 1..];
    let end = after_tag.find("</").or_else(|| after_tag.find('<'))?;
    let title = after_tag[..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Convert a URL or title to a safe filename.
fn url_to_filename(title: Option<&str>, url: &str) -> String {
    let base = if let Some(t) = title {
        t.to_string()
    } else {
        // Use the last path segment of the URL
        url.split('/')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or("page")
            .split('?')
            .next()
            .unwrap_or("page")
            .split('#')
            .next()
            .unwrap_or("page")
            .to_string()
    };

    // Sanitize: keep alphanumeric, spaces, hyphens, underscores
    let sanitized: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c == ' ' {
                '_'
            } else {
                '_'
            }
        })
        .collect();

    // Collapse multiple underscores, trim, lowercase
    let mut result = String::new();
    let mut last_was_underscore = false;
    for c in sanitized.chars() {
        if c == '_' {
            if !last_was_underscore {
                result.push('_');
            }
            last_was_underscore = true;
        } else {
            result.push(c.to_lowercase().next().unwrap_or(c));
            last_was_underscore = false;
        }
    }

    // Trim leading/trailing underscores
    let result = result.trim_matches('_').to_string();

    // Truncate to 60 chars (char-safe)
    let truncated: String = result.chars().take(60).collect();

    if truncated.is_empty() {
        "page.txt".to_string()
    } else {
        format!("{truncated}.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title() {
        assert_eq!(
            extract_title("<html><head><title>My Page</title></head></html>"),
            Some("My Page".to_string())
        );
        assert_eq!(extract_title("<html><head></head></html>"), None);
        assert_eq!(extract_title("<title></title>"), None);
        assert_eq!(
            extract_title("<title lang=\"en\">Hello World</title>"),
            Some("Hello World".to_string())
        );
    }

    #[test]
    fn test_url_to_filename() {
        assert_eq!(
            url_to_filename(Some("My Great Blog Post"), "https://example.com/post"),
            "my_great_blog_post.txt"
        );
        assert_eq!(
            url_to_filename(None, "https://example.com/docs/getting-started"),
            "getting-started.txt"
        );
        assert_eq!(
            url_to_filename(None, "https://example.com/"),
            "example_com.txt"
        );
        assert_eq!(
            url_to_filename(Some("  "), "https://example.com/"),
            "page.txt"  // whitespace-only title falls through
        );
    }
}
