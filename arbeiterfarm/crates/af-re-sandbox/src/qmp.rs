use serde_json::{json, Value};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// QMP (QEMU Machine Protocol) client for VM management.
pub struct QmpClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl QmpClient {
    /// Connect to a QMP Unix socket and complete the capability handshake.
    pub async fn connect(socket_path: &Path) -> Result<Self, String> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| format!("QMP connect to {}: {e}", socket_path.display()))?;

        let (reader, writer) = stream.into_split();
        let mut client = Self {
            reader: BufReader::new(reader),
            writer,
        };

        // Read QMP greeting
        let greeting = client.read_line().await?;
        if !greeting.contains("QMP") {
            return Err(format!("unexpected QMP greeting: {greeting}"));
        }

        // Send qmp_capabilities to exit negotiation mode
        client
            .send_command(json!({"execute": "qmp_capabilities"}))
            .await?;

        Ok(client)
    }

    async fn read_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("QMP read error: {e}"))?;
        Ok(line)
    }

    /// Read the next non-event response from QMP. Skips async event messages.
    async fn read_response(&mut self) -> Result<Value, String> {
        loop {
            let line = self.read_line().await?;
            if line.trim().is_empty() {
                continue;
            }
            let val: Value = serde_json::from_str(&line)
                .map_err(|e| format!("QMP parse error: {e}"))?;
            // Skip async events (they have an "event" key)
            if val.get("event").is_some() {
                tracing::debug!("QMP event (skipped): {}", val);
                continue;
            }
            return Ok(val);
        }
    }

    async fn send_command(&mut self, cmd: Value) -> Result<Value, String> {
        let mut bytes = serde_json::to_vec(&cmd)
            .map_err(|e| format!("QMP serialize error: {e}"))?;
        bytes.push(b'\n');
        self.writer
            .write_all(&bytes)
            .await
            .map_err(|e| format!("QMP write error: {e}"))?;
        self.read_response().await
    }

    /// Save a VM snapshot with the given name.
    pub async fn savevm(&mut self, name: &str) -> Result<(), String> {
        let resp = self
            .send_command(json!({
                "execute": "human-monitor-command",
                "arguments": {
                    "command-line": format!("savevm {name}")
                }
            }))
            .await?;
        check_hmp_response(&resp, "savevm")
    }

    /// Load (restore) a VM snapshot with the given name.
    pub async fn loadvm(&mut self, name: &str) -> Result<(), String> {
        let resp = self
            .send_command(json!({
                "execute": "human-monitor-command",
                "arguments": {
                    "command-line": format!("loadvm {name}")
                }
            }))
            .await?;
        check_hmp_response(&resp, "loadvm")
    }

    /// Take a screenshot via QMP screendump. Returns the path to the PPM file.
    /// The caller is responsible for converting to PNG if needed.
    pub async fn screendump(&mut self, output_path: &str) -> Result<(), String> {
        let resp = self
            .send_command(json!({
                "execute": "screendump",
                "arguments": {
                    "filename": output_path
                }
            }))
            .await?;

        if resp.get("error").is_some() {
            return Err(format!(
                "screendump failed: {}",
                resp["error"]["desc"]
                    .as_str()
                    .unwrap_or("unknown error")
            ));
        }
        Ok(())
    }
}

/// Check a human-monitor-command response for errors.
fn check_hmp_response(resp: &Value, cmd_name: &str) -> Result<(), String> {
    if let Some(err) = resp.get("error") {
        return Err(format!(
            "{cmd_name} failed: {}",
            err["desc"].as_str().unwrap_or("unknown error")
        ));
    }
    // HMP commands return {"return": "string"} — an empty string means success
    if let Some(ret) = resp["return"].as_str() {
        if !ret.is_empty() && ret.contains("Error") {
            return Err(format!("{cmd_name} error: {ret}"));
        }
    }
    Ok(())
}
