use af_plugin_api::{OutputRedirectPolicy, SandboxProfile, ToolPolicy, ToolSpec};
use serde_json::json;

fn artifact_id_defs() -> serde_json::Value {
    json!({
        "ArtifactId": {
            "type": "string",
            "format": "uuid",
            "description": "UUID of an artifact in the current project"
        }
    })
}

pub fn sandbox_trace_spec(gateway_socket: &std::path::Path) -> ToolSpec {
    ToolSpec {
        name: "sandbox.trace".to_string(),
        version: 1,
        deprecated: false,
        description: "Execute a sample in an isolated Windows VM with default API hooks (~60 APIs). \
                      Restores VM snapshot before each run, spawns the sample via Frida, and \
                      collects API call traces. Returns trace summary + full trace as artifact."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": {
                    "$ref": "#/$defs/ArtifactId",
                    "description": "Artifact (PE binary) to execute in the sandbox"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 5,
                    "maximum": 120,
                    "default": 30,
                    "description": "How long to let the sample run before collecting results"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": 10,
                    "description": "Optional command-line arguments to pass to the sample"
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 120_000,
            max_output_bytes: 64 * 1024 * 1024,
            max_produced_artifacts: 1,
            allow_exec: false,
            uds_bind_mounts: vec![gateway_socket.to_path_buf()],
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn sandbox_hook_spec(gateway_socket: &std::path::Path) -> ToolSpec {
    ToolSpec {
        name: "sandbox.hook".to_string(),
        version: 1,
        deprecated: false,
        description: "Execute a sample in an isolated Windows VM with a custom Frida hook script. \
                      Restores VM snapshot before each run, spawns the sample with the provided \
                      hook script, and collects results. Returns hook results as artifact."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": {
                    "$ref": "#/$defs/ArtifactId",
                    "description": "Artifact (PE binary) to execute in the sandbox"
                },
                "hook_script": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 65536,
                    "description": "Frida JavaScript hook script to inject into the sample"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 5,
                    "maximum": 120,
                    "default": 30,
                    "description": "How long to let the sample run before collecting results"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": 10,
                    "description": "Optional command-line arguments to pass to the sample"
                }
            },
            "required": ["artifact_id", "hook_script"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 120_000,
            max_output_bytes: 64 * 1024 * 1024,
            max_produced_artifacts: 1,
            allow_exec: false,
            uds_bind_mounts: vec![gateway_socket.to_path_buf()],
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn sandbox_screenshot_spec(gateway_socket: &std::path::Path) -> ToolSpec {
    ToolSpec {
        name: "sandbox.screenshot".to_string(),
        version: 1,
        deprecated: false,
        description: "Take a screenshot of the sandbox VM's current display. Returns a base64-encoded \
                      PNG image. Useful for observing GUI behavior of the running sample."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::Trusted,
            timeout_ms: 30_000,
            max_output_bytes: 16 * 1024 * 1024,
            max_produced_artifacts: 0,
            allow_exec: false,
            uds_bind_mounts: vec![gateway_socket.to_path_buf()],
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn all_specs(gateway_socket: &std::path::Path) -> Vec<ToolSpec> {
    vec![
        sandbox_trace_spec(gateway_socket),
        sandbox_hook_spec(gateway_socket),
        sandbox_screenshot_spec(gateway_socket),
    ]
}
