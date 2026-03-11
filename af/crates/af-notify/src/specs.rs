use af_core::{OutputRedirectPolicy, SandboxProfile, ToolPolicy, ToolSpec};
use serde_json::json;

fn notify_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted,
        timeout_ms: 30_000,
        max_calls_per_run: 10,
        ..ToolPolicy::default()
    }
}

pub fn notify_send_spec() -> ToolSpec {
    ToolSpec {
        name: "notify.send".to_string(),
        version: 1,
        deprecated: false,
        description: "Send a notification to a named channel (webhook, email, matrix, or webdav). \
                      The notification is queued for delivery. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["channel", "subject"],
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "Channel name (project-scoped)"
                },
                "subject": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Notification subject/title"
                },
                "body": {
                    "type": "string",
                    "description": "Notification body text"
                }
            }
        }),
        policy: notify_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn notify_upload_spec() -> ToolSpec {
    ToolSpec {
        name: "notify.upload".to_string(),
        version: 1,
        deprecated: false,
        description: "Upload an artifact to a WebDAV channel. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["channel", "artifact_id"],
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "WebDAV channel name (project-scoped)"
                },
                "artifact_id": {
                    "$ref": "#/$defs/ArtifactId"
                },
                "filename": {
                    "type": "string",
                    "description": "Override filename on remote (defaults to artifact filename)"
                }
            }
        }),
        policy: notify_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn notify_list_spec() -> ToolSpec {
    ToolSpec {
        name: "notify.list".to_string(),
        version: 1,
        deprecated: false,
        description: "List notification channels available for the current project."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
        policy: notify_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn notify_test_spec() -> ToolSpec {
    ToolSpec {
        name: "notify.test".to_string(),
        version: 1,
        deprecated: false,
        description: "Send a test notification to verify channel configuration. \
                      Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["channel"],
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "Channel name to test (project-scoped)"
                }
            }
        }),
        policy: notify_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn all_specs() -> Vec<ToolSpec> {
    vec![
        notify_send_spec(),
        notify_upload_spec(),
        notify_list_spec(),
        notify_test_spec(),
    ]
}
