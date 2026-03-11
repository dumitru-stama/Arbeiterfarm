pub mod gmail;
pub mod protonmail;

use async_trait::async_trait;
use crate::types::{
    DraftResult, EmailError, EmailFull, EmailMessage, EmailSummary, ListInboxParams, SendResult,
};

/// Provider-agnostic email operations.
#[async_trait]
pub trait EmailProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn send(
        &self,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<SendResult, EmailError>;

    async fn create_draft(
        &self,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<DraftResult, EmailError>;

    async fn list_inbox(
        &self,
        params: &ListInboxParams,
        creds: &serde_json::Value,
    ) -> Result<Vec<EmailSummary>, EmailError>;

    async fn read_message(
        &self,
        id: &str,
        creds: &serde_json::Value,
    ) -> Result<EmailFull, EmailError>;

    async fn search(
        &self,
        query: &str,
        max: u32,
        creds: &serde_json::Value,
    ) -> Result<Vec<EmailSummary>, EmailError>;

    async fn reply(
        &self,
        parent_id: &str,
        msg: &EmailMessage,
        creds: &serde_json::Value,
    ) -> Result<SendResult, EmailError>;
}
