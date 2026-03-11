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

pub fn vt_file_report_spec(gateway_socket: &std::path::Path) -> ToolSpec {
    ToolSpec {
        name: "vt.file_report".to_string(),
        version: 1,
        deprecated: false,
        description: "Look up a file hash on VirusTotal. Returns detection ratio, tags, \
                      family names, and first-seen date. Hash-only lookup — no file upload."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "$defs": artifact_id_defs(),
            "properties": {
                "artifact_id": {
                    "$ref": "#/$defs/ArtifactId",
                    "description": "Artifact to look up (its SHA256 hash is sent to VT)"
                }
            },
            "required": ["artifact_id"]
        }),
        policy: ToolPolicy {
            sandbox: SandboxProfile::NoNetReadOnly,
            timeout_ms: 30_000,
            allow_exec: false,
            uds_bind_mounts: vec![gateway_socket.to_path_buf()],
            ..ToolPolicy::default()
        },
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn all_specs(gateway_socket: &std::path::Path) -> Vec<ToolSpec> {
    vec![vt_file_report_spec(gateway_socket)]
}
