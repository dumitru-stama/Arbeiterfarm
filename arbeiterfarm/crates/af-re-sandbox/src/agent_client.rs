use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Client for communicating with the Python guest agent running inside the VM.
pub struct AgentClient {
    addr: String,
}

#[derive(Debug, Serialize)]
struct AgentRequest {
    cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_b64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct AgentResponse {
    pub status: String,
    #[serde(default)]
    pub trace: Vec<Value>,
    #[serde(default)]
    pub process_tree: Vec<Value>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub data: Value,
}

impl AgentClient {
    pub fn new(addr: String) -> Self {
        Self { addr }
    }

    /// Send a trace command: upload sample, inject default hooks, collect API trace.
    pub async fn trace(
        &self,
        sample_bytes: &[u8],
        hook_script: &str,
        timeout_secs: u64,
        args: Option<Vec<String>>,
    ) -> Result<AgentResponse, String> {
        let req = AgentRequest {
            cmd: "trace".to_string(),
            sample_b64: Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                sample_bytes,
            )),
            hook_script: Some(hook_script.to_string()),
            timeout: Some(timeout_secs),
            args,
        };
        self.send_request(&req, timeout_secs + 30).await
    }

    /// Send a hook command: upload sample, inject custom hook script, collect results.
    pub async fn hook(
        &self,
        sample_bytes: &[u8],
        hook_script: &str,
        timeout_secs: u64,
        args: Option<Vec<String>>,
    ) -> Result<AgentResponse, String> {
        let req = AgentRequest {
            cmd: "hook".to_string(),
            sample_b64: Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                sample_bytes,
            )),
            hook_script: Some(hook_script.to_string()),
            timeout: Some(timeout_secs),
            args,
        };
        self.send_request(&req, timeout_secs + 30).await
    }

    /// Send a screenshot command to the agent (for agent-side screenshots if needed).
    pub async fn screenshot(&self) -> Result<AgentResponse, String> {
        let req = AgentRequest {
            cmd: "screenshot".to_string(),
            sample_b64: None,
            hook_script: None,
            timeout: None,
            args: None,
        };
        self.send_request(&req, 15).await
    }

    async fn send_request(
        &self,
        req: &AgentRequest,
        timeout_secs: u64,
    ) -> Result<AgentResponse, String> {
        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            self.send_request_inner(req),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(format!(
                "agent request timed out after {timeout_secs}s"
            )),
        }
    }

    async fn send_request_inner(
        &self,
        req: &AgentRequest,
    ) -> Result<AgentResponse, String> {
        let stream = TcpStream::connect(&self.addr)
            .await
            .map_err(|e| format!("agent connect to {}: {e}", self.addr))?;

        let (reader, mut writer) = stream.into_split();

        // Send request as a single JSON line
        let mut req_bytes =
            serde_json::to_vec(req).map_err(|e| format!("serialize error: {e}"))?;
        req_bytes.push(b'\n');
        writer
            .write_all(&req_bytes)
            .await
            .map_err(|e| format!("write error: {e}"))?;
        writer
            .shutdown()
            .await
            .map_err(|e| format!("shutdown write: {e}"))?;

        // Read response
        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::new();
        buf_reader
            .read_line(&mut response_line)
            .await
            .map_err(|e| format!("read error: {e}"))?;

        serde_json::from_str(&response_line)
            .map_err(|e| format!("parse agent response: {e}"))
    }
}
