use af_core::{OutputRedirectPolicy, SandboxProfile, ToolPolicy, ToolSpec};
use serde_json::json;

fn email_send_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted,
        timeout_ms: 30_000,
        max_calls_per_run: 5,
        ..ToolPolicy::default()
    }
}

fn email_read_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted,
        timeout_ms: 30_000,
        max_calls_per_run: 10,
        ..ToolPolicy::default()
    }
}

fn email_schedule_policy() -> ToolPolicy {
    ToolPolicy {
        sandbox: SandboxProfile::Trusted,
        timeout_ms: 15_000,
        max_calls_per_run: 5,
        ..ToolPolicy::default()
    }
}

pub fn email_send_spec() -> ToolSpec {
    ToolSpec {
        name: "email.send".to_string(),
        version: 1,
        deprecated: false,
        description: "Send an email. Validates recipients against allowlist/blocklist rules \
                      before sending. Use dry_run=true to validate without sending. \
                      Restricted tool: requires admin grant and configured credentials."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["to", "subject", "body"],
            "properties": {
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Recipient email addresses"
                },
                "cc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CC email addresses"
                },
                "bcc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "BCC email addresses"
                },
                "subject": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Email body text"
                },
                "tone": {
                    "type": "string",
                    "description": "Tone preset name (brief, formal, informal, technical, executive_summary, friendly, urgent, diplomatic)"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Validate everything without actually sending"
                }
            }
        }),
        policy: email_send_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn email_draft_spec() -> ToolSpec {
    ToolSpec {
        name: "email.draft".to_string(),
        version: 1,
        deprecated: false,
        description: "Create an email draft without sending. The draft is saved in the provider's \
                      drafts folder for review. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["to", "subject", "body"],
            "properties": {
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Recipient email addresses"
                },
                "cc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CC email addresses"
                },
                "bcc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "BCC email addresses"
                },
                "subject": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Email body text"
                },
                "tone": {
                    "type": "string",
                    "description": "Tone preset name used for composition"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                }
            }
        }),
        policy: email_read_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn email_schedule_spec() -> ToolSpec {
    ToolSpec {
        name: "email.schedule".to_string(),
        version: 1,
        deprecated: false,
        description: "Schedule an email to be sent at a future time. The email is validated \
                      immediately but sent later by the tick cron job. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["to", "subject", "body", "scheduled_at"],
            "properties": {
                "to": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Recipient email addresses"
                },
                "cc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CC email addresses"
                },
                "bcc": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "BCC email addresses"
                },
                "subject": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Email body text"
                },
                "scheduled_at": {
                    "type": "string",
                    "description": "ISO 8601 UTC datetime for when to send (must be in the future)"
                },
                "tone": {
                    "type": "string",
                    "description": "Tone preset name"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                }
            }
        }),
        policy: email_schedule_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn email_list_inbox_spec() -> ToolSpec {
    ToolSpec {
        name: "email.list_inbox".to_string(),
        version: 1,
        deprecated: false,
        description: "List recent emails in your inbox. Returns summaries with message IDs \
                      for use with email.read. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 10,
                    "description": "Maximum number of emails to return"
                },
                "label": {
                    "type": "string",
                    "description": "Filter by label/folder (provider-specific)"
                },
                "unread_only": {
                    "type": "boolean",
                    "description": "Only return unread messages"
                },
                "since": {
                    "type": "string",
                    "description": "Only return messages after this ISO 8601 date"
                }
            }
        }),
        policy: email_send_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn email_read_spec() -> ToolSpec {
    ToolSpec {
        name: "email.read".to_string(),
        version: 1,
        deprecated: false,
        description: "Read the full content of an email by message ID (from email.list_inbox or email.search). \
                      Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["message_id"],
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "Provider message ID"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                },
                "include_attachments": {
                    "type": "boolean",
                    "description": "Include attachment metadata in response"
                }
            }
        }),
        policy: email_read_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn email_reply_spec() -> ToolSpec {
    ToolSpec {
        name: "email.reply".to_string(),
        version: 1,
        deprecated: false,
        description: "Reply to an email. Automatically sets threading headers (In-Reply-To, References). \
                      Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["message_id", "body"],
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "Provider message ID of the email to reply to"
                },
                "body": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Reply body text"
                },
                "reply_all": {
                    "type": "boolean",
                    "description": "Reply to all recipients (default: false, reply to sender only)"
                },
                "tone": {
                    "type": "string",
                    "description": "Tone preset name"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                }
            }
        }),
        policy: email_send_policy(),
        output_redirect: OutputRedirectPolicy::Forbidden,
    }
}

pub fn email_search_spec() -> ToolSpec {
    ToolSpec {
        name: "email.search".to_string(),
        version: 1,
        deprecated: false,
        description: "Search emails by query string. Returns summaries with message IDs. \
                      Supports provider-specific search syntax. Restricted tool: requires admin grant."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (provider-specific syntax)"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gmail", "protonmail"],
                    "description": "Email provider (uses default if omitted)"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 10,
                    "description": "Maximum number of results"
                }
            }
        }),
        policy: email_send_policy(),
        output_redirect: OutputRedirectPolicy::Allowed,
    }
}

pub fn all_specs() -> Vec<ToolSpec> {
    vec![
        email_send_spec(),
        email_draft_spec(),
        email_schedule_spec(),
        email_list_inbox_spec(),
        email_read_spec(),
        email_reply_spec(),
        email_search_spec(),
    ]
}
