use async_trait::async_trait;
use lettre::message::{header, Mailbox, MessageBuilder};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::providers::EmailProvider;
use crate::types::*;

const DEFAULT_SMTP_HOST: &str = "127.0.0.1";
const DEFAULT_SMTP_PORT: u16 = 1025;
const DEFAULT_IMAP_HOST: &str = "127.0.0.1";
const DEFAULT_IMAP_PORT: u16 = 1143;

pub struct ProtonMailProvider;

impl ProtonMailProvider {
    pub fn new() -> Self {
        Self
    }

    fn get_smtp_config(creds: &serde_json::Value) -> (String, u16) {
        let host = creds["smtp_host"]
            .as_str()
            .unwrap_or(DEFAULT_SMTP_HOST)
            .to_string();
        let port = creds["smtp_port"].as_u64().unwrap_or(DEFAULT_SMTP_PORT as u64) as u16;
        (host, port)
    }

    fn get_imap_config(creds: &serde_json::Value) -> (String, u16) {
        let host = creds["imap_host"]
            .as_str()
            .unwrap_or(DEFAULT_IMAP_HOST)
            .to_string();
        let port = creds["imap_port"].as_u64().unwrap_or(DEFAULT_IMAP_PORT as u64) as u16;
        (host, port)
    }

    fn get_credentials(creds: &serde_json::Value) -> Result<(String, String), EmailError> {
        let username = creds["username"]
            .as_str()
            .ok_or_else(|| EmailError::NoCredentials("missing username".into()))?
            .to_string();
        let password = creds["password"]
            .as_str()
            .ok_or_else(|| EmailError::NoCredentials("missing password".into()))?
            .to_string();
        Ok((username, password))
    }

    fn build_lettre_message(msg: &EmailMessage) -> Result<Message, EmailError> {
        let from: Mailbox = msg
            .from
            .parse()
            .map_err(|e| EmailError::InvalidInput(format!("invalid from address: {e}")))?;

        let mut builder = MessageBuilder::new()
            .from(from)
            .subject(&msg.subject);

        for addr in &msg.to {
            let mbox: Mailbox = addr
                .parse()
                .map_err(|e| EmailError::InvalidInput(format!("invalid to address '{addr}': {e}")))?;
            builder = builder.to(mbox);
        }
        for addr in &msg.cc {
            let mbox: Mailbox = addr
                .parse()
                .map_err(|e| EmailError::InvalidInput(format!("invalid cc address '{addr}': {e}")))?;
            builder = builder.cc(mbox);
        }
        for addr in &msg.bcc {
            let mbox: Mailbox = addr
                .parse()
                .map_err(|e| EmailError::InvalidInput(format!("invalid bcc address '{addr}': {e}")))?;
            builder = builder.bcc(mbox);
        }

        if let Some(ref reply_to) = msg.in_reply_to {
            builder = builder.header(header::InReplyTo::from(reply_to.clone()));
        }
        if let Some(ref refs) = msg.references {
            builder = builder.header(header::References::from(refs.clone()));
        }

        builder
            .body(msg.body_text.clone())
            .map_err(|e| EmailError::ProviderError(format!("failed to build message: {e}")))
    }

    fn is_localhost(host: &str) -> bool {
        host == "127.0.0.1" || host == "localhost" || host == "::1"
    }

    async fn send_via_smtp(
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<String, EmailError> {
        let (host, port) = Self::get_smtp_config(creds);
        let (username, password) = Self::get_credentials(creds)?;

        let smtp_creds = Credentials::new(username, password);

        // Use plain (unencrypted) transport for localhost (ProtonMail Bridge),
        // STARTTLS for remote hosts.
        let mailer = if Self::is_localhost(&host) {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host)
                .port(port)
                .credentials(smtp_creds)
                .build()
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
                .map_err(|e| EmailError::ProviderError(format!("SMTP connection failed: {e}")))?
                .port(port)
                .credentials(smtp_creds)
                .build()
        };

        let email = Self::build_lettre_message(msg)?;

        let response = mailer
            .send(email)
            .await
            .map_err(|e| EmailError::ProviderError(format!("SMTP send failed: {e}")))?;

        let msg_id = response
            .message()
            .next()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "sent".to_string());
        Ok(msg_id)
    }
}

#[async_trait]
impl EmailProvider for ProtonMailProvider {
    fn name(&self) -> &str {
        "protonmail"
    }

    async fn send(
        &self,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<SendResult, EmailError> {
        let msg_id = Self::send_via_smtp(msg, creds).await?;
        Ok(SendResult {
            provider_message_id: msg_id,
            provider: "protonmail".to_string(),
        })
    }

    async fn create_draft(
        &self,
        _msg: &EmailMessage,
        _creds: &serde_json::Value,
    ) -> Result<DraftResult, EmailError> {
        // IMAP APPEND to Drafts folder — basic implementation
        Err(EmailError::ProviderError(
            "ProtonMail draft creation via IMAP not yet implemented. \
             Use email.send with dry_run=true to preview, then send directly."
                .to_string(),
        ))
    }

    async fn list_inbox(
        &self,
        _params: &ListInboxParams,
        _creds: &serde_json::Value,
    ) -> Result<Vec<EmailSummary>, EmailError> {
        let (_host, _port) = Self::get_imap_config(_creds);
        // IMAP inbox listing — basic implementation placeholder
        Err(EmailError::ProviderError(
            "ProtonMail IMAP inbox listing not yet fully implemented. \
             Configure ProtonMail Bridge and ensure IMAP access is enabled."
                .to_string(),
        ))
    }

    async fn read_message(
        &self,
        _id: &str,
        _creds: &serde_json::Value,
    ) -> Result<EmailFull, EmailError> {
        Err(EmailError::ProviderError(
            "ProtonMail IMAP message read not yet fully implemented.".to_string(),
        ))
    }

    async fn search(
        &self,
        _query: &str,
        _max: u32,
        _creds: &serde_json::Value,
    ) -> Result<Vec<EmailSummary>, EmailError> {
        Err(EmailError::ProviderError(
            "ProtonMail IMAP search not yet fully implemented.".to_string(),
        ))
    }

    async fn reply(
        &self,
        _parent_id: &str,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<SendResult, EmailError> {
        // For ProtonMail, reply is just a send with In-Reply-To headers already set
        self.send(msg, creds).await
    }
}
