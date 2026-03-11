pub mod agent_client;
pub mod executor;
pub mod gateway;
pub mod hooks;
pub mod qmp;
pub mod specs;

use af_plugin_api::{ToolExecutorRegistry, ToolSpecRegistry};
use std::path::Path;

/// Pure phase: register sandbox tool specs.
pub fn declare(registry: &mut ToolSpecRegistry, gateway_socket: &Path) {
    for spec in specs::all_specs(gateway_socket) {
        registry
            .register(spec)
            .expect("failed to register sandbox tool spec");
    }
}

/// Runtime phase: register sandbox tool executors (Trusted, in-process — talk to gateway via UDS).
pub fn wire(executors: &mut ToolExecutorRegistry, gateway_socket: &Path) {
    executors
        .register(Box::new(executor::SandboxTraceExecutor {
            gateway_socket: gateway_socket.to_path_buf(),
        }))
        .expect("failed to register sandbox.trace executor");

    executors
        .register(Box::new(executor::SandboxHookExecutor {
            gateway_socket: gateway_socket.to_path_buf(),
        }))
        .expect("failed to register sandbox.hook executor");

    executors
        .register(Box::new(executor::SandboxScreenshotExecutor {
            gateway_socket: gateway_socket.to_path_buf(),
        }))
        .expect("failed to register sandbox.screenshot executor");
}

/// Start the sandbox gateway daemon. Returns a JoinHandle for the gateway task.
///
/// Environment variables:
/// - `AF_SANDBOX_QMP`: QMP Unix socket path (required)
/// - `AF_SANDBOX_AGENT`: Guest agent TCP address (default: `192.168.122.10:9111`)
/// - `AF_SANDBOX_SNAPSHOT`: Snapshot name (default: `clean`)
pub async fn start_sandbox_gateway(
    socket_path: &Path,
) -> Option<tokio::task::JoinHandle<()>> {
    let qmp_path = match std::env::var("AF_SANDBOX_QMP") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => {
            eprintln!("[sandbox-gateway] AF_SANDBOX_QMP not set, gateway disabled");
            return None;
        }
    };

    let agent_addr = std::env::var("AF_SANDBOX_AGENT")
        .unwrap_or_else(|_| "192.168.122.10:9111".to_string());
    let snapshot_name = std::env::var("AF_SANDBOX_SNAPSHOT")
        .unwrap_or_else(|_| "clean".to_string());

    let gw = gateway::SandboxGateway::new(
        socket_path.to_path_buf(),
        qmp_path,
        agent_addr,
        snapshot_name,
    );

    let handle = gw.start().await;
    eprintln!(
        "[sandbox-gateway] started at {}",
        socket_path.display()
    );
    Some(handle)
}
