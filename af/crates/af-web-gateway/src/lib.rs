pub mod executor;
pub mod gateway;
pub mod geoip;
pub mod rate_limiter;
pub mod rules;
pub mod specs;
pub mod ssrf;

use af_core::{ToolExecutorRegistry, ToolSpecRegistry};
use std::path::Path;
use std::sync::Arc;

/// Phase 1: register tool specs (pure, no deps).
pub fn declare(registry: &mut ToolSpecRegistry) {
    for spec in specs::all_specs() {
        registry
            .register(spec)
            .expect("failed to register web gateway tool spec");
    }
}

/// Phase 2: wire in-process executors.
pub fn wire(executors: &mut ToolExecutorRegistry, gateway_socket: &Path) {
    executors
        .register(Box::new(executor::WebFetchExecutor {
            gateway_socket: gateway_socket.to_path_buf(),
        }))
        .expect("failed to register web.fetch executor");

    executors
        .register(Box::new(executor::WebSearchExecutor {
            gateway_socket: gateway_socket.to_path_buf(),
        }))
        .expect("failed to register web.search executor");
}

/// Start the gateway daemon. Returns a JoinHandle.
pub async fn start_gateway(
    config: gateway::WebGatewayConfig,
) -> tokio::task::JoinHandle<()> {
    let gw = Arc::new(gateway::WebGateway::new(config));
    gw.start().await
}
