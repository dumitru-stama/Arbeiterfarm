use std::collections::HashMap;
use std::fmt;

/// Signal kind emitted by agents to influence workflow routing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// Add agent to next group.
    Request,
    /// Remove agent from all future groups.
    Skip,
    /// Move agent from later group to next group.
    Priority,
}

impl fmt::Display for SignalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalKind::Request => write!(f, "request"),
            SignalKind::Skip => write!(f, "skip"),
            SignalKind::Priority => write!(f, "priority"),
        }
    }
}

/// A signal parsed from agent output text.
#[derive(Debug, Clone)]
pub struct AgentSignal {
    pub kind: SignalKind,
    pub target_agent: String,
    pub reason: String,
    pub source_agent: String,
}

/// Parse signal markers from agent output content.
///
/// Format: `signal:<kind>:<agent>:<reason>`
/// where kind is request|skip|priority, agent is alphanumeric+hyphen+underscore,
/// and reason is non-empty free text (may contain spaces).
pub fn parse_signals(content: &str, source_agent: &str) -> Vec<AgentSignal> {
    let mut signals = Vec::new();

    for line in content.lines() {
        // Find all occurrences of "signal:" in the line (may be preceded by backticks/quotes)
        let mut search_from = 0;
        while let Some(pos) = line[search_from..].find("signal:") {
            let abs_pos = search_from + pos;

            // Extract the signal text from this position to end of line,
            // trimming trailing backticks/quotes/punctuation
            let raw = line[abs_pos..].trim_end_matches(|c: char| {
                c == '`' || c == '\'' || c == '"' || c == ',' || c == '.'
            });

            if let Some(rest) = raw.strip_prefix("signal:") {
                if let Some(sig) = parse_one_signal(rest, source_agent) {
                    signals.push(sig);
                }
            }

            // Advance past this match
            search_from = abs_pos + "signal:".len();
        }
    }

    signals
}

fn parse_one_signal(rest: &str, source_agent: &str) -> Option<AgentSignal> {
    // rest = "request:decompiler:Found packed UPX binary"
    let mut parts = rest.splitn(3, ':');
    let kind_str = parts.next()?;
    let target = parts.next()?;
    let reason = parts.next().unwrap_or("");

    let kind = match kind_str {
        "request" => SignalKind::Request,
        "skip" => SignalKind::Skip,
        "priority" => SignalKind::Priority,
        _ => return None,
    };

    // Validate agent name: non-empty, alphanumeric + hyphen + underscore
    if target.is_empty() {
        return None;
    }
    if !target
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }

    // Reason must be non-empty
    let reason = reason.trim();
    if reason.is_empty() {
        return None;
    }

    Some(AgentSignal {
        kind,
        target_agent: target.to_string(),
        reason: reason.to_string(),
        source_agent: source_agent.to_string(),
    })
}

/// Resolve conflicting signals for the same target agent.
///
/// Rules:
/// - request wins over skip for the same target (conservative)
/// - First signal per (target, kind) wins
pub fn resolve_conflicts(signals: Vec<AgentSignal>) -> Vec<AgentSignal> {
    // Track which (target, kind) we've already seen — first wins
    let mut seen: HashMap<(String, SignalKind), usize> = HashMap::new();
    let mut result: Vec<AgentSignal> = Vec::new();

    for sig in signals {
        let key = (sig.target_agent.clone(), sig.kind.clone());
        if seen.contains_key(&key) {
            continue; // first per (target, kind) wins
        }
        seen.insert(key, result.len());
        result.push(sig);
    }

    // Request wins over skip: remove skip entries where a request exists for the same target
    let request_targets: Vec<String> = result
        .iter()
        .filter(|s| s.kind == SignalKind::Request)
        .map(|s| s.target_agent.clone())
        .collect();

    result.retain(|s| !(s.kind == SignalKind::Skip && request_targets.contains(&s.target_agent)));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_request_signal() {
        let content = "Analysis complete. signal:request:decompiler:Found packed UPX binary at offset 0x4000";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, SignalKind::Request);
        assert_eq!(signals[0].target_agent, "decompiler");
        assert_eq!(
            signals[0].reason,
            "Found packed UPX binary at offset 0x4000"
        );
        assert_eq!(signals[0].source_agent, "surface");
    }

    #[test]
    fn parse_skip_signal() {
        let content = "signal:skip:reporter:Binary is benign, no report needed";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, SignalKind::Skip);
        assert_eq!(signals[0].target_agent, "reporter");
    }

    #[test]
    fn parse_priority_signal() {
        let content = "signal:priority:decompiler:Critical anti-debug code in main(), analyze urgently";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, SignalKind::Priority);
        assert_eq!(signals[0].target_agent, "decompiler");
    }

    #[test]
    fn parse_backtick_wrapped() {
        let content = "Found something: `signal:request:intel:Suspicious domain found`";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].target_agent, "intel");
        assert_eq!(signals[0].reason, "Suspicious domain found");
    }

    #[test]
    fn parse_quote_wrapped() {
        let content = r#""signal:request:intel:Suspicious domain found""#;
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].target_agent, "intel");
    }

    #[test]
    fn reject_missing_reason() {
        let content = "signal:request:decompiler:";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 0);
    }

    #[test]
    fn reject_no_reason_field() {
        let content = "signal:request:decompiler";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 0);
    }

    #[test]
    fn reject_bad_agent_name() {
        let content = "signal:request:bad agent:some reason";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 0);
    }

    #[test]
    fn reject_empty_agent_name() {
        let content = "signal:request::some reason";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 0);
    }

    #[test]
    fn reject_unknown_kind() {
        let content = "signal:unknown:agent:some reason";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 0);
    }

    #[test]
    fn parse_multiple_signals() {
        let content = "signal:request:decompiler:UPX found\nOther text signal:skip:reporter:benign";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].kind, SignalKind::Request);
        assert_eq!(signals[1].kind, SignalKind::Skip);
    }

    #[test]
    fn parse_agent_with_hyphens_underscores() {
        let content = "signal:request:my-agent_v2:good reason";
        let signals = parse_signals(content, "surface");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].target_agent, "my-agent_v2");
    }

    #[test]
    fn conflict_request_wins_over_skip() {
        let signals = vec![
            AgentSignal {
                kind: SignalKind::Skip,
                target_agent: "decompiler".into(),
                reason: "not needed".into(),
                source_agent: "a".into(),
            },
            AgentSignal {
                kind: SignalKind::Request,
                target_agent: "decompiler".into(),
                reason: "actually needed".into(),
                source_agent: "b".into(),
            },
        ];
        let resolved = resolve_conflicts(signals);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, SignalKind::Request);
        assert_eq!(resolved[0].reason, "actually needed");
    }

    #[test]
    fn conflict_first_same_kind_wins() {
        let signals = vec![
            AgentSignal {
                kind: SignalKind::Request,
                target_agent: "decompiler".into(),
                reason: "first reason".into(),
                source_agent: "a".into(),
            },
            AgentSignal {
                kind: SignalKind::Request,
                target_agent: "decompiler".into(),
                reason: "second reason".into(),
                source_agent: "b".into(),
            },
        ];
        let resolved = resolve_conflicts(signals);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].reason, "first reason");
    }

    #[test]
    fn no_conflict_different_targets() {
        let signals = vec![
            AgentSignal {
                kind: SignalKind::Skip,
                target_agent: "reporter".into(),
                reason: "not needed".into(),
                source_agent: "a".into(),
            },
            AgentSignal {
                kind: SignalKind::Request,
                target_agent: "decompiler".into(),
                reason: "needed".into(),
                source_agent: "b".into(),
            },
        ];
        let resolved = resolve_conflicts(signals);
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn display_signal_kind() {
        assert_eq!(SignalKind::Request.to_string(), "request");
        assert_eq!(SignalKind::Skip.to_string(), "skip");
        assert_eq!(SignalKind::Priority.to_string(), "priority");
    }

    #[test]
    fn no_signals_in_normal_text() {
        let content = "This is a normal analysis. The binary contains no malware indicators.";
        let signals = parse_signals(content, "surface");
        assert!(signals.is_empty());
    }
}
