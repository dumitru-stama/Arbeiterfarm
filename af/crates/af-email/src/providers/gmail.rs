use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::json;

use crate::providers::EmailProvider;
use crate::types::*;

const GMAIL_API_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// Strip CR, LF, and NUL from header values to prevent RFC 2822 header injection.
/// An attacker-controlled address like "a@evil.com\r\nBcc: secret@victim.com"
/// would otherwise inject additional headers into the raw message.
fn sanitize_header_value(s: &str) -> String {
    s.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect()
}

pub struct GmailProvider {
    client: reqwest::Client,
}

impl GmailProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(25))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Refresh the OAuth2 access token using the stored refresh token.
    async fn get_access_token(&self, creds: &serde_json::Value) -> Result<String, EmailError> {
        let client_id = creds["client_id"]
            .as_str()
            .ok_or_else(|| EmailError::NoCredentials("missing client_id".into()))?;
        let client_secret = creds["client_secret"]
            .as_str()
            .ok_or_else(|| EmailError::NoCredentials("missing client_secret".into()))?;
        let refresh_token = creds["refresh_token"]
            .as_str()
            .ok_or_else(|| EmailError::NoCredentials("missing refresh_token".into()))?;
        let token_uri = creds["token_uri"]
            .as_str()
            .unwrap_or(TOKEN_URI);

        let resp = self
            .client
            .post(token_uri)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(|e| EmailError::ProviderError(format!("token refresh request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::ProviderError(format!(
                "token refresh failed ({status}): {body}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmailError::ProviderError(format!("token parse failed: {e}")))?;

        data["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| EmailError::ProviderError("no access_token in response".into()))
    }

    /// Build RFC 2822 message and base64url-encode for Gmail API.
    fn build_raw_message(msg: &EmailMessage) -> String {
        let mut rfc = String::new();
        rfc.push_str(&format!("From: {}\r\n", sanitize_header_value(&msg.from)));
        rfc.push_str(&format!(
            "To: {}\r\n",
            msg.to.iter().map(|a| sanitize_header_value(a)).collect::<Vec<_>>().join(", ")
        ));
        if !msg.cc.is_empty() {
            rfc.push_str(&format!(
                "Cc: {}\r\n",
                msg.cc.iter().map(|a| sanitize_header_value(a)).collect::<Vec<_>>().join(", ")
            ));
        }
        if !msg.bcc.is_empty() {
            rfc.push_str(&format!(
                "Bcc: {}\r\n",
                msg.bcc.iter().map(|a| sanitize_header_value(a)).collect::<Vec<_>>().join(", ")
            ));
        }
        // RFC 2047 encode subject if it contains non-ASCII characters
        let subject = if msg.subject.is_ascii() {
            sanitize_header_value(&msg.subject)
        } else {
            format!(
                "=?UTF-8?B?{}?=",
                base64::engine::general_purpose::STANDARD.encode(msg.subject.as_bytes())
            )
        };
        rfc.push_str(&format!("Subject: {subject}\r\n"));
        rfc.push_str(&format!("Date: {}\r\n", chrono::Utc::now().to_rfc2822()));
        if let Some(ref reply_to) = msg.in_reply_to {
            rfc.push_str(&format!("In-Reply-To: {}\r\n", sanitize_header_value(reply_to)));
        }
        if let Some(ref refs) = msg.references {
            rfc.push_str(&format!("References: {}\r\n", sanitize_header_value(refs)));
        }
        rfc.push_str("MIME-Version: 1.0\r\n");
        rfc.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
        rfc.push_str("Content-Transfer-Encoding: 8bit\r\n");
        rfc.push_str("\r\n");
        rfc.push_str(&msg.body_text);

        URL_SAFE_NO_PAD.encode(rfc.as_bytes())
    }

    /// Validate that a Gmail message ID is safe for URL interpolation.
    fn sanitize_message_id(id: &str) -> Result<&str, EmailError> {
        // Gmail message IDs are hex strings (e.g. "18f3a2b4c5d6e7f8")
        if id.is_empty() {
            return Err(EmailError::InvalidInput("message_id is empty".into()));
        }
        if id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(EmailError::InvalidInput(format!(
                "message_id '{}' contains invalid characters",
                &id[..id.len().min(32)]
            )));
        }
        Ok(id)
    }

    fn parse_message_json(data: &serde_json::Value) -> EmailSummary {
        let headers = data["payload"]["headers"].as_array();
        let get_header = |name: &str| -> String {
            headers
                .and_then(|h| {
                    h.iter()
                        .find(|hdr| hdr["name"].as_str().map(|s| s.eq_ignore_ascii_case(name)).unwrap_or(false))
                        .and_then(|hdr| hdr["value"].as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_default()
        };

        let labels = data["labelIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let is_unread = data["labelIds"]
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str() == Some("UNREAD")))
            .unwrap_or(false);

        EmailSummary {
            message_id: data["id"].as_str().unwrap_or_default().to_string(),
            from: get_header("From"),
            to: get_header("To")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            subject: get_header("Subject"),
            snippet: data["snippet"].as_str().unwrap_or_default().to_string(),
            date: get_header("Date"),
            is_unread,
            labels,
            thread_id: data["threadId"].as_str().map(|s| s.to_string()),
        }
    }

    fn parse_full_message(data: &serde_json::Value) -> EmailFull {
        let headers = data["payload"]["headers"].as_array();
        let get_header = |name: &str| -> String {
            headers
                .and_then(|h| {
                    h.iter()
                        .find(|hdr| hdr["name"].as_str().map(|s| s.eq_ignore_ascii_case(name)).unwrap_or(false))
                        .and_then(|hdr| hdr["value"].as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_default()
        };

        let body_text = extract_body_text(data);

        let labels = data["labelIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let attachments = extract_attachment_info(data);

        EmailFull {
            message_id: data["id"].as_str().unwrap_or_default().to_string(),
            from: get_header("From"),
            to: get_header("To")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            cc: get_header("Cc")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            subject: get_header("Subject"),
            body_text,
            body_html: None,
            date: get_header("Date"),
            labels,
            thread_id: data["threadId"].as_str().map(|s| s.to_string()),
            in_reply_to: {
                let v = get_header("In-Reply-To");
                if v.is_empty() { None } else { Some(v) }
            },
            references: {
                let v = get_header("References");
                if v.is_empty() { None } else { Some(v) }
            },
            attachments,
        }
    }
}

fn extract_body_text(data: &serde_json::Value) -> String {
    // Try direct body
    if let Some(body) = data["payload"]["body"]["data"].as_str() {
        if let Ok(decoded) = URL_SAFE_NO_PAD.decode(body) {
            if let Ok(text) = String::from_utf8(decoded) {
                return text;
            }
        }
    }
    // Try parts
    if let Some(parts) = data["payload"]["parts"].as_array() {
        for part in parts {
            let mime = part["mimeType"].as_str().unwrap_or_default();
            if mime == "text/plain" {
                if let Some(body) = part["body"]["data"].as_str() {
                    if let Ok(decoded) = URL_SAFE_NO_PAD.decode(body) {
                        if let Ok(text) = String::from_utf8(decoded) {
                            return text;
                        }
                    }
                }
            }
        }
    }
    String::new()
}

fn extract_attachment_info(data: &serde_json::Value) -> Vec<AttachmentInfo> {
    let mut attachments = vec![];
    if let Some(parts) = data["payload"]["parts"].as_array() {
        for part in parts {
            if let Some(filename) = part["filename"].as_str() {
                if !filename.is_empty() {
                    attachments.push(AttachmentInfo {
                        filename: filename.to_string(),
                        mime_type: part["mimeType"]
                            .as_str()
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        size_bytes: part["body"]["size"].as_u64().unwrap_or(0),
                    });
                }
            }
        }
    }
    attachments
}

#[async_trait]
impl EmailProvider for GmailProvider {
    fn name(&self) -> &str {
        "gmail"
    }

    async fn send(
        &self,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<SendResult, EmailError> {
        let token = self.get_access_token(creds).await?;
        let raw = Self::build_raw_message(msg);

        let mut body = json!({ "raw": raw });
        if let Some(ref tid) = msg.thread_id {
            body["threadId"] = json!(tid);
        }

        let resp = self
            .client
            .post(format!("{GMAIL_API_BASE}/messages/send"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| EmailError::ProviderError(format!("send request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::ProviderError(format!(
                "Gmail send failed ({status}): {body}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmailError::ProviderError(format!("response parse failed: {e}")))?;

        Ok(SendResult {
            provider_message_id: data["id"].as_str().unwrap_or_default().to_string(),
            provider: "gmail".to_string(),
        })
    }

    async fn create_draft(
        &self,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<DraftResult, EmailError> {
        let token = self.get_access_token(creds).await?;
        let raw = Self::build_raw_message(msg);

        let body = json!({
            "message": { "raw": raw }
        });

        let resp = self
            .client
            .post(format!("{GMAIL_API_BASE}/drafts"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| EmailError::ProviderError(format!("draft request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::ProviderError(format!(
                "Gmail draft failed ({status}): {body}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmailError::ProviderError(format!("response parse failed: {e}")))?;

        Ok(DraftResult {
            draft_id: data["id"].as_str().unwrap_or_default().to_string(),
            provider: "gmail".to_string(),
        })
    }

    async fn list_inbox(
        &self,
        params: &ListInboxParams,
        creds: &serde_json::Value,
    ) -> Result<Vec<EmailSummary>, EmailError> {
        let token = self.get_access_token(creds).await?;

        let mut url = format!(
            "{GMAIL_API_BASE}/messages?maxResults={}",
            params.max_results
        );
        if let Some(ref label) = params.label {
            url.push_str(&format!("&labelIds={}", urlencoding::encode(label)));
        } else {
            url.push_str("&labelIds=INBOX");
        }
        // Combine query terms into a single `q` parameter to avoid conflicts
        let mut q_parts: Vec<String> = Vec::new();
        if params.unread_only {
            q_parts.push("is:unread".into());
        }
        if let Some(ref since) = params.since {
            // Sanitize: only allow date-like characters
            let safe_since: String = since
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '/')
                .collect();
            q_parts.push(format!("after:{safe_since}"));
        }
        if !q_parts.is_empty() {
            url.push_str(&format!("&q={}", urlencoding::encode(&q_parts.join(" "))));
        }

        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| EmailError::ProviderError(format!("list request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::ProviderError(format!(
                "Gmail list failed ({status}): {body}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmailError::ProviderError(format!("response parse failed: {e}")))?;

        let message_ids: Vec<String> = data["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Fetch metadata for each message
        let mut summaries = Vec::with_capacity(message_ids.len());
        for mid in message_ids {
            let safe_mid = Self::sanitize_message_id(&mid)?;
            let msg_resp = self
                .client
                .get(format!("{GMAIL_API_BASE}/messages/{safe_mid}?format=metadata&metadataHeaders=From&metadataHeaders=To&metadataHeaders=Subject&metadataHeaders=Date"))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| EmailError::ProviderError(format!("message fetch failed: {e}")))?;

            if msg_resp.status().is_success() {
                let msg_data: serde_json::Value = msg_resp
                    .json()
                    .await
                    .map_err(|e| EmailError::ProviderError(format!("message parse failed: {e}")))?;
                summaries.push(Self::parse_message_json(&msg_data));
            }
        }

        Ok(summaries)
    }

    async fn read_message(
        &self,
        id: &str,
        creds: &serde_json::Value,
    ) -> Result<EmailFull, EmailError> {
        let token = self.get_access_token(creds).await?;
        let safe_id = Self::sanitize_message_id(id)?;

        let resp = self
            .client
            .get(format!("{GMAIL_API_BASE}/messages/{safe_id}?format=full"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| EmailError::ProviderError(format!("read request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::ProviderError(format!(
                "Gmail read failed ({status}): {body}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmailError::ProviderError(format!("response parse failed: {e}")))?;

        Ok(Self::parse_full_message(&data))
    }

    async fn search(
        &self,
        query: &str,
        max: u32,
        creds: &serde_json::Value,
    ) -> Result<Vec<EmailSummary>, EmailError> {
        let token = self.get_access_token(creds).await?;

        let encoded_query = urlencoding::encode(query);
        let resp = self
            .client
            .get(format!(
                "{GMAIL_API_BASE}/messages?q={encoded_query}&maxResults={max}"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| EmailError::ProviderError(format!("search request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::ProviderError(format!(
                "Gmail search failed ({status}): {body}"
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmailError::ProviderError(format!("response parse failed: {e}")))?;

        let message_ids: Vec<String> = data["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut summaries = Vec::with_capacity(message_ids.len());
        for mid in message_ids {
            let safe_mid = Self::sanitize_message_id(&mid)?;
            let msg_resp = self
                .client
                .get(format!("{GMAIL_API_BASE}/messages/{safe_mid}?format=metadata&metadataHeaders=From&metadataHeaders=To&metadataHeaders=Subject&metadataHeaders=Date"))
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| EmailError::ProviderError(format!("message fetch failed: {e}")))?;

            if msg_resp.status().is_success() {
                let msg_data: serde_json::Value = msg_resp
                    .json()
                    .await
                    .map_err(|e| EmailError::ProviderError(format!("message parse failed: {e}")))?;
                summaries.push(Self::parse_message_json(&msg_data));
            }
        }

        Ok(summaries)
    }

    async fn reply(
        &self,
        _parent_id: &str,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<SendResult, EmailError> {
        // The executor already reads the parent and sets in_reply_to, references,
        // and thread_id on the message — just delegate to send.
        self.send(msg, creds).await
    }
}

/// Needed for URL query encoding.
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push_str(&format!("%{:02X}", b));
                }
            }
        }
        result
    }
}
