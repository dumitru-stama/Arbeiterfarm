use async_trait::async_trait;
use serde::Deserialize;

use crate::error::LlmError;

/// Trait for text embedding backends.
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    /// Backend name (e.g. "snowflake-arctic-embed2").
    fn name(&self) -> &str;

    /// Vector dimensions produced by this model.
    fn dimensions(&self) -> u32;

    /// Embed one or more texts in a single batch call.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, LlmError>;
}

/// Ollama embedding backend — calls `/api/embed` endpoint.
pub struct OllamaEmbeddingBackend {
    model: String,
    endpoint: String,
    dims: u32,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaEmbeddingBackend {
    pub fn new(endpoint: String, model: String, dimensions: u32) -> Self {
        Self {
            model,
            endpoint,
            dims: dimensions,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl EmbeddingBackend for OllamaEmbeddingBackend {
    fn name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> u32 {
        self.dims
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, LlmError> {
        let url = format!("{}/api/embed", self.endpoint.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        let parsed: OllamaEmbedResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::JsonParse(e.to_string()))?;

        Ok(parsed.embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_embed_response_parse() {
        let json = r#"{"embeddings":[[0.1,0.2,0.3],[0.4,0.5,0.6]]}"#;
        let resp: OllamaEmbedResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.embeddings.len(), 2);
        assert_eq!(resp.embeddings[0], vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_backend_creation() {
        let backend = OllamaEmbeddingBackend::new(
            "http://localhost:11434".to_string(),
            "snowflake-arctic-embed2".to_string(),
            1024,
        );
        assert_eq!(backend.name(), "snowflake-arctic-embed2");
        assert_eq!(backend.dimensions(), 1024);
    }
}
