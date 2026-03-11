use af_core::LlmRoute;
use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::LlmBackend;
use crate::error::LlmError;
use crate::redact::RedactionLayer;
use crate::request::CompletionRequest;

#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub name: String,
    pub capabilities: af_core::BackendCapabilities,
}

/// Routes LLM requests to the appropriate backend.
/// Applies redaction when routing to non-local backends.
pub struct LlmRouter {
    backends: HashMap<String, Arc<dyn LlmBackend>>,
    aliases: HashMap<String, String>,
    default_backend: Option<String>,
    local_backend: Option<String>,
    redaction: RedactionLayer,
}

impl LlmRouter {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
            aliases: HashMap::new(),
            default_backend: None,
            local_backend: None,
            redaction: RedactionLayer::new(),
        }
    }

    pub fn register(&mut self, backend: Box<dyn LlmBackend>) {
        let name = backend.name().to_string();
        let caps = backend.capabilities();
        let arc: Arc<dyn LlmBackend> = Arc::from(backend);
        if caps.is_local && self.local_backend.is_none() {
            self.local_backend = Some(name.clone());
        }
        if self.default_backend.is_none() {
            self.default_backend = Some(name.clone());
        }
        self.backends.insert(name, arc);
    }

    pub fn set_default(&mut self, name: &str) {
        self.default_backend = Some(name.to_string());
    }

    /// Register an alias that maps to an existing backend name.
    pub fn register_alias(&mut self, alias: &str, target: &str) {
        self.aliases.insert(alias.to_string(), target.to_string());
    }

    /// Resolve a route to the corresponding backend. Returns a cloned Arc.
    /// For `Backend(name)`, tries exact match first, then falls back to alias lookup.
    pub fn resolve(&self, route: &LlmRoute) -> Result<Arc<dyn LlmBackend>, LlmError> {
        let name = match route {
            LlmRoute::Auto => self
                .default_backend
                .as_deref()
                .ok_or(LlmError::NoBackend)?,
            LlmRoute::Local => self
                .local_backend
                .as_deref()
                .ok_or(LlmError::NoBackend)?,
            LlmRoute::Backend(name) => name.as_str(),
        };
        self.backends
            .get(name)
            .or_else(|| self.aliases.get(name).and_then(|t| self.backends.get(t)))
            .cloned()
            .ok_or_else(|| LlmError::BackendNotFound(name.to_string()))
    }

    /// Redact a request if the resolved backend is non-local.
    pub fn maybe_redact(
        &self,
        route: &LlmRoute,
        request: &CompletionRequest,
    ) -> CompletionRequest {
        let backend = match self.resolve(route) {
            Ok(b) => b,
            Err(_) => return request.clone(),
        };

        if backend.capabilities().is_local {
            return request.clone();
        }

        // Apply redaction to message contents and tool call arguments
        let mut redacted = request.clone();
        for msg in &mut redacted.messages {
            msg.content = self.redaction.redact(&msg.content);
            for tc in &mut msg.tool_calls {
                tc.arguments = self.redaction.redact_json_values(&tc.arguments);
            }
            // Also redact text parts in multi-modal content
            if let Some(ref mut parts) = msg.content_parts {
                for part in parts.iter_mut() {
                    if let af_core::ContentPart::Text { ref mut text } = part {
                        *text = self.redaction.redact(text);
                    }
                }
            }
        }
        redacted
    }

    pub fn has_backends(&self) -> bool {
        !self.backends.is_empty()
    }

    pub fn list_backends(&self) -> Vec<BackendInfo> {
        let mut items: Vec<BackendInfo> = self
            .backends
            .iter()
            .map(|(name, backend)| BackendInfo {
                name: name.clone(),
                capabilities: backend.capabilities(),
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }
}
