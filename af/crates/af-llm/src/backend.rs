use async_trait::async_trait;
use af_core::BackendCapabilities;
use tokio::sync::mpsc;

use crate::error::LlmError;
use crate::request::{CompletionRequest, CompletionResponse, StreamChunk};

/// Trait implemented by LLM backends (OpenAI-compatible, Anthropic, etc.).
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Human-readable backend name.
    fn name(&self) -> &str;

    /// Capabilities of this backend.
    fn capabilities(&self) -> BackendCapabilities;

    /// Non-streaming completion.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Streaming completion. Sends chunks to the provided sender.
    /// Default implementation wraps `complete()` into synthetic stream events.
    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        tx: mpsc::Sender<Result<StreamChunk, LlmError>>,
    ) -> Result<(), LlmError> {
        let response = self.complete(request).await?;

        // Emit text as a single token
        if !response.content.is_empty() {
            let _ = tx.send(Ok(StreamChunk::Token(response.content))).await;
        }

        // Emit tool calls as start + done
        for tc in &response.tool_calls {
            let _ = tx
                .send(Ok(StreamChunk::ToolCallStart {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                }))
                .await;
            let args_str = serde_json::to_string(&tc.arguments).unwrap_or_default();
            let _ = tx
                .send(Ok(StreamChunk::ToolCallDelta {
                    id: tc.id.clone(),
                    arguments_delta: args_str,
                }))
                .await;
        }

        // Emit usage if available
        if let Some(usage) = response.usage {
            let _ = tx.send(Ok(StreamChunk::Usage(usage))).await;
        }

        let _ = tx
            .send(Ok(StreamChunk::Done(response.finish_reason)))
            .await;

        Ok(())
    }
}
