use crate::agent_client::AgentClient;
use crate::hooks::DEFAULT_HOOK_SCRIPT;
use crate::qmp::QmpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

/// Sandbox gateway daemon. Listens on a Unix Domain Socket, manages QEMU VM
/// state via QMP, and proxies trace/hook/screenshot commands to the guest agent.
pub struct SandboxGateway {
    socket_path: PathBuf,
    qmp_path: PathBuf,
    agent_addr: String,
    snapshot_name: String,
    /// Mutex to serialize VM operations (one trace/hook at a time).
    vm_lock: Mutex<()>,
}

#[derive(Debug, Deserialize)]
struct GatewayRequest {
    action: String,
    #[serde(default)]
    sample_b64: Option<String>,
    #[serde(default)]
    hook_script: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    args: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct GatewayResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn error_response(code: &str, client_msg: &str, log_detail: &str) -> GatewayResponse {
    tracing::warn!("[sandbox-gateway] {code}: {log_detail}");
    GatewayResponse {
        ok: false,
        data: None,
        error: Some(code.to_string()),
        message: Some(client_msg.to_string()),
    }
}

impl SandboxGateway {
    pub fn new(
        socket_path: PathBuf,
        qmp_path: PathBuf,
        agent_addr: String,
        snapshot_name: String,
    ) -> Self {
        Self {
            socket_path,
            qmp_path,
            agent_addr,
            snapshot_name,
            vm_lock: Mutex::new(()),
        }
    }

    /// Start the gateway. Returns a JoinHandle for shutdown.
    pub async fn start(self) -> tokio::task::JoinHandle<()> {
        // Remove stale socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        if let Some(parent) = self.socket_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let listener = UnixListener::bind(&self.socket_path)
            .expect("failed to bind sandbox gateway socket");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.socket_path,
                std::fs::Permissions::from_mode(0o660),
            );
        }

        let gateway = Arc::new(self);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let gw = Arc::clone(&gateway);
                        tokio::spawn(async move {
                            if let Err(e) = gw.handle_connection(stream).await {
                                tracing::error!("[sandbox-gateway] connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("[sandbox-gateway] accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        })
    }

    async fn handle_connection(
        &self,
        stream: tokio::net::UnixStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;

        let req = match serde_json::from_str::<GatewayRequest>(&line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[sandbox-gateway] invalid request: {e}");
                return Ok(());
            }
        };

        let response = self.process_request(req).await;
        let resp_json = serde_json::to_string(&response)?;
        writer.write_all(resp_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.shutdown().await?;

        Ok(())
    }

    async fn process_request(&self, req: GatewayRequest) -> GatewayResponse {
        match req.action.as_str() {
            "trace" => self.handle_trace(req).await,
            "hook" => self.handle_hook(req).await,
            "screenshot" => self.handle_screenshot().await,
            other => error_response("error", "invalid request", &format!("unknown action: {other}")),
        }
    }

    async fn handle_trace(&self, req: GatewayRequest) -> GatewayResponse {
        let sample_b64 = match req.sample_b64 {
            Some(ref s) if !s.is_empty() => s,
            _ => return error_response("error", "missing sample_b64", "trace: no sample data"),
        };

        let sample_bytes = match base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            sample_b64,
        ) {
            Ok(b) => b,
            Err(e) => {
                return error_response("error", "invalid sample data", &format!("base64 decode: {e}"))
            }
        };

        let timeout_secs = req.timeout_secs.unwrap_or(30).min(120);

        // Serialize VM operations
        let _lock = self.vm_lock.lock().await;

        // Restore snapshot
        if let Err(e) = self.restore_snapshot().await {
            return error_response("vm_error", "failed to restore VM snapshot", &e);
        }

        // Wait for guest agent to be ready after snapshot restore
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Send trace command to agent
        let agent = AgentClient::new(self.agent_addr.clone());
        match agent
            .trace(&sample_bytes, DEFAULT_HOOK_SCRIPT, timeout_secs, req.args)
            .await
        {
            Ok(resp) => {
                if resp.status != "ok" {
                    let err_msg = resp.errors.first().cloned().unwrap_or_default();
                    return error_response("agent_error", "trace execution failed", &err_msg);
                }
                GatewayResponse {
                    ok: true,
                    data: Some(json!({
                        "trace": resp.trace,
                        "process_tree": resp.process_tree,
                        "errors": resp.errors,
                    })),
                    error: None,
                    message: None,
                }
            }
            Err(e) => error_response("agent_error", "failed to communicate with guest agent", &e),
        }
    }

    async fn handle_hook(&self, req: GatewayRequest) -> GatewayResponse {
        let sample_b64 = match req.sample_b64 {
            Some(ref s) if !s.is_empty() => s,
            _ => return error_response("error", "missing sample_b64", "hook: no sample data"),
        };

        let hook_script = match req.hook_script {
            Some(ref s) if !s.is_empty() => s.clone(),
            _ => return error_response("error", "missing hook_script", "hook: no script"),
        };

        let sample_bytes = match base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            sample_b64,
        ) {
            Ok(b) => b,
            Err(e) => {
                return error_response("error", "invalid sample data", &format!("base64 decode: {e}"))
            }
        };

        let timeout_secs = req.timeout_secs.unwrap_or(30).min(120);

        let _lock = self.vm_lock.lock().await;

        if let Err(e) = self.restore_snapshot().await {
            return error_response("vm_error", "failed to restore VM snapshot", &e);
        }

        tokio::time::sleep(Duration::from_secs(2)).await;

        let agent = AgentClient::new(self.agent_addr.clone());
        match agent
            .hook(&sample_bytes, &hook_script, timeout_secs, req.args)
            .await
        {
            Ok(resp) => {
                if resp.status != "ok" {
                    let err_msg = resp.errors.first().cloned().unwrap_or_default();
                    return error_response("agent_error", "hook execution failed", &err_msg);
                }
                GatewayResponse {
                    ok: true,
                    data: Some(json!({
                        "trace": resp.trace,
                        "process_tree": resp.process_tree,
                        "errors": resp.errors,
                        "data": resp.data,
                    })),
                    error: None,
                    message: None,
                }
            }
            Err(e) => error_response("agent_error", "failed to communicate with guest agent", &e),
        }
    }

    async fn handle_screenshot(&self) -> GatewayResponse {
        let ppm_path = "/tmp/af_sandbox_screen.ppm";

        // QMP screendump — no need to hold vm_lock for screenshots
        let mut qmp = match QmpClient::connect(&self.qmp_path).await {
            Ok(c) => c,
            Err(e) => return error_response("qmp_error", "failed to connect to QMP", &e),
        };

        if let Err(e) = qmp.screendump(ppm_path).await {
            return error_response("qmp_error", "screendump failed", &e);
        }

        // Read PPM file and convert to PNG-compatible base64
        // PPM is large; we return it as-is with the format noted (tools can handle PPM)
        match tokio::fs::read(ppm_path).await {
            Ok(data) => {
                let b64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &data,
                );
                let _ = tokio::fs::remove_file(ppm_path).await;
                GatewayResponse {
                    ok: true,
                    data: Some(json!({
                        "format": "ppm",
                        "image_b64": b64,
                        "size_bytes": data.len(),
                    })),
                    error: None,
                    message: None,
                }
            }
            Err(e) => error_response("io_error", "failed to read screenshot", &format!("{e}")),
        }
    }

    async fn restore_snapshot(&self) -> Result<(), String> {
        let mut qmp = QmpClient::connect(&self.qmp_path).await?;
        qmp.loadvm(&self.snapshot_name).await?;
        tracing::info!("[sandbox-gateway] restored snapshot '{}'", self.snapshot_name);
        Ok(())
    }
}
