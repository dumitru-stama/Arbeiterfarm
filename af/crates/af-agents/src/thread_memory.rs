//! Deterministic memory extraction for per-thread persistent memory.
//!
//! Parses tool result strings (already compact from artifact-first output)
//! into `MemoryEntry` pairs. No LLM call required.

/// Maximum length of a single memory entry value.
pub const MAX_ENTRY_VALUE_LEN: usize = 256;

/// Higher cap for tools that produce complex, high-value findings.
pub const MAX_HIGH_VALUE_ENTRY_LEN: usize = 512;

/// Maximum total rendered memory size in bytes.
pub const MAX_MEMORY_SIZE: usize = 2048;

/// Tools whose findings benefit from a larger memory cap.
const HIGH_VALUE_TOOLS: &[&str] = &[
    "ghidra.decompile",
    "sandbox.trace",
    "sandbox.hook",
    "yara.generate",
];

/// A key-value memory entry to persist for a thread.
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
}

/// Extract memory entries from a tool result.
/// Called after each successful tool call. Returns empty vec for tools
/// that don't produce persistent findings.
pub fn extract_from_tool_result(tool_name: &str, result: &str) -> Vec<MemoryEntry> {
    // Known tools get a curated key name; unknown tools get a generic one.
    // Tools that are purely navigational (file.read_range) are excluded.
    let key = match tool_name {
        "rizin.bininfo" | "file.info" => {
            if tool_name == "rizin.bininfo" {
                "finding:rizin_bininfo"
            } else {
                "finding:file_info"
            }
        }
        "ghidra.analyze" => "finding:ghidra_analyze",
        "ghidra.decompile" => "finding:ghidra_decompile",
        "strings.extract" | "file.strings" => "finding:strings",
        "sandbox.trace" => "finding:sandbox_trace",
        "sandbox.hook" => "finding:sandbox_hook",
        "vt.file_report" => "finding:vtotal",
        "file.grep" => "finding:grep",
        "rizin.disasm" => "finding:rizin_disasm",
        "rizin.xrefs" => "finding:rizin_xrefs",
        "file.hexdump" => "finding:hexdump",
        "yara.scan" => "finding:yara_scan",
        "yara.test" => "finding:yara_test",
        "yara.generate" => "finding:yara_generate",
        // Navigational/read-only tools — no memory value
        "file.read_range" | "embed.search" | "embed.text" | "embed.list"
        | "meta.read_thread" | "meta.list_agents" | "meta.list_artifacts"
        | "meta.read_artifact" | "dedup.prior_analysis" => return vec![],
        // Generic fallback: any unknown tool gets a memory entry keyed by name.
        // This future-proofs new tools without requiring code changes.
        _ => {
            let safe_name = tool_name.replace('.', "_");
            let value = truncate_value_with_cap(result, MAX_ENTRY_VALUE_LEN);
            if value.is_empty() {
                return vec![];
            }
            return vec![MemoryEntry {
                key: format!("finding:{safe_name}"),
                value,
            }];
        }
    };

    // High-value tools get a larger truncation cap
    let cap = if HIGH_VALUE_TOOLS.contains(&tool_name) {
        MAX_HIGH_VALUE_ENTRY_LEN
    } else {
        MAX_ENTRY_VALUE_LEN
    };

    let value = truncate_value_with_cap(result, cap);
    if value.is_empty() {
        return vec![];
    }

    vec![MemoryEntry {
        key: key.to_string(),
        value,
    }]
}

/// Extract goal from the first user message.
/// Returns None if the message is empty/whitespace-only (e.g. workflow surface agent).
pub fn extract_goal(first_user_message: &str) -> Option<MemoryEntry> {
    let value = truncate_value(first_user_message);
    if value.is_empty() {
        return None;
    }
    Some(MemoryEntry {
        key: "goal".to_string(),
        value,
    })
}

/// Extract the latest user request for memory persistence.
/// Updated on every user message so the model knows the current task
/// even after sliding-window trimming drops the original message.
pub fn extract_latest_request(user_message: &str) -> Option<MemoryEntry> {
    let value = truncate_value(user_message);
    if value.is_empty() {
        return None;
    }
    Some(MemoryEntry {
        key: "latest_request".to_string(),
        value,
    })
}

/// Extract conclusion from the assistant's final text.
pub fn extract_conclusion(final_text: &str) -> MemoryEntry {
    MemoryEntry {
        key: "conclusion".to_string(),
        value: truncate_value(final_text),
    }
}

/// Render all memory entries into a structured task-digest format for injection.
///
/// Format:
/// ```text
/// [Thread Memory]
/// TASK: <goal>
/// CURRENT: <latest_request>
///
/// FINDINGS:
/// - ghidra_analyze: <value>
/// - strings: <value>
///
/// CONCLUSION: <conclusion>
///
/// Use these findings. Do not repeat completed work.
/// ```
pub fn render_memory(entries: &[(String, String)]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let goal: Option<&str> = entries
        .iter()
        .find(|(k, _)| k == "goal")
        .map(|(_, v)| v.as_str());
    let latest: Option<&str> = entries
        .iter()
        .find(|(k, _)| k == "latest_request")
        .map(|(_, v)| v.as_str());
    let conclusion: Option<&str> = entries
        .iter()
        .find(|(k, _)| k == "conclusion")
        .map(|(_, v)| v.as_str());

    let mut findings: Vec<(&str, &str)> = entries
        .iter()
        .filter(|(k, _)| k.starts_with("finding:"))
        .map(|(k, v)| (k.strip_prefix("finding:").unwrap_or(k), v.as_str()))
        .collect();
    findings.sort_by(|a, b| a.0.cmp(&b.0));

    let mut result = String::from("[Thread Memory]\n");

    if let Some(g) = goal {
        result.push_str(&format!("TASK: {g}\n"));
    }
    if let Some(l) = latest {
        result.push_str(&format!("CURRENT: {l}\n"));
    }

    if !findings.is_empty() {
        result.push_str("\nFINDINGS:\n");
        for (k, v) in &findings {
            let line = format!("- {k}: {v}\n");
            if result.len() + line.len() > MAX_MEMORY_SIZE {
                break;
            }
            result.push_str(&line);
        }
    }

    if let Some(c) = conclusion {
        result.push_str(&format!("\nCONCLUSION: {c}\n"));
    }

    result.push_str("\nUse these findings. Do not repeat completed work.");
    result
}

/// Truncate a string to the given cap, respecting char boundaries.
fn truncate_value_with_cap(s: &str, cap: usize) -> String {
    let s = s.trim();
    if s.len() <= cap {
        return s.to_string();
    }
    let end = s.floor_char_boundary(cap);
    format!("{}...", &s[..end])
}

/// Truncate a string to MAX_ENTRY_VALUE_LEN, respecting char boundaries.
fn truncate_value(s: &str) -> String {
    truncate_value_with_cap(s, MAX_ENTRY_VALUE_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_from_tool_result() {
        let entries = extract_from_tool_result("rizin.bininfo", "PE32+ x86-64, NX+ASLR");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "finding:rizin_bininfo");
        assert_eq!(entries[0].value, "PE32+ x86-64, NX+ASLR");
    }

    #[test]
    fn test_extract_unknown_tool_gets_generic_key() {
        let entries = extract_from_tool_result("custom.hash", "SHA256: abc123");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "finding:custom_hash");
        assert_eq!(entries[0].value, "SHA256: abc123");
    }

    #[test]
    fn test_extract_navigational_tool_excluded() {
        let entries = extract_from_tool_result("file.read_range", "lots of content");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_high_value_tool_gets_larger_cap() {
        let long_result = "x".repeat(400);
        let entries = extract_from_tool_result("ghidra.decompile", &long_result);
        assert_eq!(entries.len(), 1);
        // High-value tools get 512 cap, so 400 chars fits without truncation
        assert_eq!(entries[0].value, long_result);

        // Normal tool would truncate at 256
        let normal_entries = extract_from_tool_result("file.grep", &long_result);
        assert_eq!(normal_entries.len(), 1);
        assert!(normal_entries[0].value.ends_with("..."));
        assert!(normal_entries[0].value.len() <= MAX_ENTRY_VALUE_LEN + 3);
    }

    #[test]
    fn test_extract_goal() {
        let entry = extract_goal("Analyze the malware sample").unwrap();
        assert_eq!(entry.key, "goal");
        assert_eq!(entry.value, "Analyze the malware sample");
    }

    #[test]
    fn test_extract_goal_empty() {
        assert!(extract_goal("").is_none());
        assert!(extract_goal("   ").is_none());
        assert!(extract_goal("\n\t").is_none());
    }

    #[test]
    fn test_extract_conclusion() {
        let entry = extract_conclusion("Binary is a Go-compiled dropper");
        assert_eq!(entry.key, "conclusion");
        assert_eq!(entry.value, "Binary is a Go-compiled dropper");
    }

    #[test]
    fn test_extract_latest_request() {
        let entry = extract_latest_request("Show me FUN_00102540").unwrap();
        assert_eq!(entry.key, "latest_request");
        assert_eq!(entry.value, "Show me FUN_00102540");
    }

    #[test]
    fn test_extract_latest_request_empty() {
        assert!(extract_latest_request("").is_none());
        assert!(extract_latest_request("  ").is_none());
    }

    #[test]
    fn test_truncate_value() {
        let short = "hello";
        assert_eq!(truncate_value(short), "hello");

        let long = "x".repeat(300);
        let truncated = truncate_value(&long);
        assert!(truncated.len() <= MAX_ENTRY_VALUE_LEN + 3); // +3 for "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_render_memory_empty() {
        assert_eq!(render_memory(&[]), "");
    }

    #[test]
    fn test_render_memory_task_digest_format() {
        let entries = vec![
            ("finding:strings".to_string(), "142 strings found".to_string()),
            ("goal".to_string(), "Analyze the binary".to_string()),
            ("latest_request".to_string(), "Show me FUN_00102540".to_string()),
            ("finding:file_info".to_string(), "ELF 64-bit".to_string()),
            ("conclusion".to_string(), "Binary is a Go dropper".to_string()),
        ];
        let rendered = render_memory(&entries);
        assert!(rendered.starts_with("[Thread Memory]\n"));
        assert!(rendered.contains("TASK: Analyze the binary"));
        assert!(rendered.contains("CURRENT: Show me FUN_00102540"));
        assert!(rendered.contains("FINDINGS:\n"));
        // Findings sorted alphabetically by stripped key
        assert!(rendered.contains("- file_info: ELF 64-bit"));
        assert!(rendered.contains("- strings: 142 strings found"));
        assert!(rendered.contains("CONCLUSION: Binary is a Go dropper"));
        assert!(rendered.contains("Use these findings. Do not repeat completed work."));
    }

    #[test]
    fn test_render_memory_goal_only() {
        let entries = vec![("goal".to_string(), "Analyze sample".to_string())];
        let rendered = render_memory(&entries);
        assert!(rendered.contains("TASK: Analyze sample"));
        assert!(!rendered.contains("FINDINGS:"));
        assert!(!rendered.contains("CONCLUSION:"));
    }

    #[test]
    fn test_render_memory_max_size() {
        let mut entries = Vec::new();
        entries.push(("goal".to_string(), "test goal".to_string()));
        for i in 0..100 {
            entries.push((format!("finding:test_{i:03}"), "x".repeat(100)));
        }
        let rendered = render_memory(&entries);
        assert!(rendered.len() <= MAX_MEMORY_SIZE + 200); // slack for header/footer
    }
}
