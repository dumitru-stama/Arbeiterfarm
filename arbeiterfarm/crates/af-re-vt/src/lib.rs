pub mod cache;
pub mod executor;
pub mod gateway;
pub mod rate_limiter;
pub mod specs;

use af_plugin_api::{Migration, SpawnConfig, ToolExecutorRegistry, ToolSpecRegistry};
use std::path::Path;

pub use gateway::VtGateway;

/// Pure phase: register tool specs.
pub fn declare(specs: &mut ToolSpecRegistry, gateway_socket: &Path) {
    for spec in specs::all_specs(gateway_socket) {
        specs
            .register(spec)
            .expect("failed to register VT tool spec");
    }
}

/// Runtime phase: register executors (OOP — runs inside bwrap sandbox).
///
/// If `executor_path` is provided, registers OOP executor that runs in bwrap.
/// Otherwise falls back to InProcess executor (no sandbox isolation).
pub fn wire(
    executors: &mut ToolExecutorRegistry,
    gateway_socket: &Path,
    executor_path: Option<&Path>,
) {
    if let Some(exec_path) = executor_path {
        let config = SpawnConfig {
            binary_path: exec_path.to_path_buf(),
            protocol_version: 1,
            supported_tools: vec![("vt.file_report".into(), 1)],
            context_extra: serde_json::json!({
                "gateway_socket": gateway_socket.to_string_lossy(),
            }),
        };
        executors
            .register_oop(config)
            .expect("failed to register vt.file_report OOP executor");
    } else {
        // InProcess fallback: no bwrap isolation
        executors
            .register(Box::new(executor::VtFileReportExecutor {
                gateway_socket: gateway_socket.to_path_buf(),
            }))
            .expect("failed to register vt.file_report executor");
    }
}

/// VT plugin migrations.
pub fn migrations() -> Vec<Migration> {
    vec![Migration {
        version: 2,
        name: "create re.vt_cache table".to_string(),
        sql: include_str!("../../../migrations/002_vt_cache.sql").to_string(),
    }]
}
