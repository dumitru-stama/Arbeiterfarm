use af_core::{AgentEvent, ChatMessage, ChatRole};
use af_llm::{CompletionRequest, LlmBackend, RedactionLayer, ToolDescription};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::AgentError;

/// Context for deciding whether and how to compact messages.
pub struct CompactionContext {
    pub context_window: u32,
    pub max_output_tokens: u32,
    /// Fraction of context_window at which to trigger compaction (e.g. 0.85).
    pub threshold: f32,
}

impl CompactionContext {
    /// The token budget available for input messages (accounting for output reservation).
    pub fn budget(&self) -> u32 {
        let usable = (self.context_window as f32 * self.threshold) as u32;
        usable.saturating_sub(self.max_output_tokens)
    }

    /// Returns true if the estimated token count exceeds the budget.
    pub fn should_compact(&self, estimated_tokens: u32) -> bool {
        estimated_tokens > self.budget()
    }

    /// How many tokens need to be shed to get back under budget.
    pub fn tokens_to_shed(&self, estimated_tokens: u32) -> u32 {
        estimated_tokens.saturating_sub(self.budget())
    }
}

/// Result of selecting messages for compaction.
pub struct CompactionPlan {
    /// Index range of messages to compact (start..end, exclusive).
    pub compact_start: usize,
    pub compact_end: usize,
    /// Number of messages that will be compacted.
    pub message_count: usize,
}

/// Reason why context was trimmed.
#[derive(Debug)]
pub enum TrimReason {
    SlidingWindow,
    ThresholdReset,
    #[allow(dead_code)]
    ThresholdLlm,
}

impl std::fmt::Display for TrimReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrimReason::SlidingWindow => write!(f, "sliding_window"),
            TrimReason::ThresholdReset => write!(f, "threshold_reset"),
            TrimReason::ThresholdLlm => write!(f, "threshold_llm"),
        }
    }
}

/// Diagnostic metadata from a trim operation.
pub struct TrimMetadata {
    pub reason: TrimReason,
    pub pre_tokens: u32,
    pub post_tokens: u32,
    pub memory_keys: Vec<String>,
    pub turns_trimmed: usize,
    pub turns_kept: usize,
}

/// An atomic group of messages that must not be split during trimming.
struct Turn {
    start: usize,           // inclusive index in messages[]
    #[allow(dead_code)]
    end: usize,             // exclusive index (used in tests)
    tokens: u32,            // estimated tokens for all messages in turn
    has_user_request: bool, // contains a real User message (not nudge/memory)
}

/// Minimum tokens to keep in the tail after sliding window trim.
/// ~24KB text, roughly 4 tool call cycles with artifact-first output.
const SLIDING_WINDOW_TAIL_TOKENS: u32 = 6_000;

/// Minimum number of turns to always keep in the tail.
const SLIDING_WINDOW_MIN_TURNS: usize = 2;

/// Fraction of the token budget at which sliding window fires.
/// Budget = context_window * threshold - max_output. Window fires at 50% of that.
const SLIDING_WINDOW_BUDGET_FRACTION: f32 = 0.50;

/// Check if a User message content is synthetic (nudge/reinforcement/memory/summary).
pub fn is_nudge_or_reinforcement(content: &str) -> bool {
    content.starts_with("[Thread Memory")
        || content.starts_with("[SYSTEM REMINDER]")
        || content.starts_with("[Context Summary")
        || content.starts_with("[Sliding window trim")
        || content.starts_with("[Context reset")
        || content.starts_with("---\nContinue your analysis")
}

/// Detect where the message body starts (after system prompt + optional memory).
pub fn detect_body_start(messages: &[ChatMessage]) -> usize {
    if messages.len() > 1
        && messages[1].role == ChatRole::User
        && messages[1].content.starts_with("[Thread Memory")
    {
        2
    } else {
        1
    }
}

/// Map the exclusive-end index of a trim range to the DB seq number of the
/// last trimmed message. messages[0] is the system prompt (not in history),
/// so messages[i] for i >= 1 maps to history[i-1].
pub fn map_index_to_seq(
    exclusive_end: usize,
    history: &[af_db::messages::MessageRow],
) -> Option<i64> {
    if exclusive_end >= 2 && (exclusive_end - 2) < history.len() {
        Some(history[exclusive_end - 2].seq)
    } else if !history.is_empty() {
        Some(history[history.len() - 1].seq)
    } else {
        None
    }
}

/// Parse the message body into atomic turns that must not be split.
///
/// A new turn starts at each real User message (not nudge/memory/summary).
/// Nudge/reinforcement User messages stay attached to the preceding turn.
fn parse_turns(messages: &[ChatMessage], body_start: usize) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut current_start = body_start;
    let mut current_has_user = false;

    for i in body_start..messages.len() {
        let msg = &messages[i];
        let is_real_user =
            msg.role == ChatRole::User && !is_nudge_or_reinforcement(&msg.content);

        if is_real_user && i > current_start {
            // Close the current turn before starting a new one
            let tokens: u32 = messages[current_start..i]
                .iter()
                .map(|m| m.estimate_content_tokens() + 4)
                .sum();
            turns.push(Turn {
                start: current_start,
                end: i,
                tokens,
                has_user_request: current_has_user,
            });
            current_start = i;
            current_has_user = true;
        } else if is_real_user {
            current_has_user = true;
        }
    }

    // Close the last turn
    if current_start < messages.len() {
        let tokens: u32 = messages[current_start..messages.len()]
            .iter()
            .map(|m| m.estimate_content_tokens() + 4)
            .sum();
        turns.push(Turn {
            start: current_start,
            end: messages.len(),
            tokens,
            has_user_request: current_has_user,
        });
    }

    turns
}

/// Select which messages should be compacted.
///
/// Partitions messages into:
/// 1. Head — index 0 (system prompt), always preserved
/// 2. Middle — indices 1..boundary, compaction candidates
/// 3. Tail — boundary..end, always preserved (recent context)
///
/// The boundary walks backward from the last user message, keeping complete
/// tool-call groups together (assistant with tool_calls + matching Tool results).
pub fn select_messages_for_compaction(messages: &[ChatMessage]) -> Option<CompactionPlan> {
    // Need at least: system + some middle + some tail
    if messages.len() < 4 {
        return None;
    }

    // Find the boundary: preserve recent messages starting from the last user message.
    // Walk backward to find the last user message.
    let last_user_idx = messages
        .iter()
        .rposition(|m| m.role == ChatRole::User)?;

    // We want to keep at least the last user message and everything after it.
    // But we also need to keep any tool-call group that straddles the boundary:
    // an assistant message with tool_calls and its matching Tool results must stay together.
    let mut boundary = last_user_idx;

    // Walk backward from the boundary to include any preceding tool-call group.
    // A tool-call group = an assistant msg with tool_calls followed by tool results.
    while boundary > 1 {
        let prev = boundary - 1;
        if messages[prev].role == ChatRole::Tool {
            // This tool result belongs to a preceding assistant; include it.
            boundary = prev;
            continue;
        }
        if messages[prev].role == ChatRole::Assistant && !messages[prev].tool_calls.is_empty() {
            // The assistant that owns the tool results we just included.
            boundary = prev;
            continue;
        }
        // Also include TOOL_RESULT_REINFORCEMENT user messages that sit between tool results
        if messages[prev].role == ChatRole::User
            && prev > 1
            && messages[prev - 1].role == ChatRole::Tool
        {
            boundary = prev;
            continue;
        }
        break;
    }

    // Middle = indices 1..boundary (must have at least 1 message to compact)
    if boundary <= 1 {
        return None;
    }

    Some(CompactionPlan {
        compact_start: 1,
        compact_end: boundary,
        message_count: boundary - 1,
    })
}

/// Summarize a set of messages using the LLM.
///
/// Sends a summarization request to produce a compact summary that preserves:
/// findings, artifact UUIDs, evidence references, tool outcomes.
/// Tool results are truncated to 2KB in the summarization input.
///
/// When the summarization backend is non-local (cloud), the conversation text
/// is redacted before sending to prevent sensitive data leakage.
pub async fn compact_messages(
    msgs: &[ChatMessage],
    backend: &Arc<dyn LlmBackend>,
    summarization_backend: Option<&Arc<dyn LlmBackend>>,
    max_summary_tokens: u32,
) -> Result<String, AgentError> {
    let mut conversation_text = String::new();

    for msg in msgs {
        let role_label = match msg.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
            ChatRole::Tool => "Tool",
            ChatRole::System => "System",
        };

        let content = if msg.role == ChatRole::Tool {
            // Truncate tool results to 2KB
            if msg.content.len() > 2048 {
                let end = msg.content.floor_char_boundary(2048);
                format!("{}... [truncated]", &msg.content[..end])
            } else {
                msg.content.clone()
            }
        } else {
            msg.content.clone()
        };

        conversation_text.push_str(&format!("[{role_label}]: {content}\n\n"));

        // Note presence of images in multi-modal messages
        if let Some(ref parts) = msg.content_parts {
            let image_count = parts.iter().filter(|p| matches!(p, af_core::ContentPart::Image { .. })).count();
            if image_count > 0 {
                conversation_text.push_str(&format!("[{image_count} image(s) omitted from summary]\n\n"));
            }
        }
    }

    let summarizer = summarization_backend.unwrap_or(backend);

    // Redact conversation text if the summarization backend is non-local (cloud).
    // This prevents sensitive data from tool outputs (e.g. secrets extracted from
    // analyzed binaries) from leaking to cloud providers via compaction.
    let conversation_text = if !summarizer.capabilities().is_local {
        let redaction = RedactionLayer::new();
        redaction.redact(&conversation_text)
    } else {
        conversation_text
    };

    let system = "You are a conversation summarizer. Produce a concise summary of the \
        conversation below. Preserve: key findings, artifact UUIDs (format: evidence:artifact:<uuid>), \
        evidence references, tool call outcomes and important results. \
        Omit: raw tool output data, verbose explanations, the system prompt content. \
        Write in factual, dense prose. Do not use bullet points.";

    let user_prompt = format!(
        "Summarize the following conversation in at most {} tokens. \
        Focus on preserving facts, findings, and references.\n\n---\n\n{}",
        max_summary_tokens, conversation_text
    );

    let request = CompletionRequest {
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: system.to_string(),
                tool_call_id: None,
                name: None,
                tool_calls: vec![],
                content_parts: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: user_prompt,
                tool_call_id: None,
                name: None,
                tool_calls: vec![],
                content_parts: None,
            },
        ],
        tools: vec![],
        max_tokens: Some(max_summary_tokens),
        temperature: Some(0.2),
    };

    let response = summarizer.complete(request).await?;
    Ok(response.content)
}

/// Attempt to compact messages if they exceed the context budget.
///
/// Orchestrates: select -> summarize -> persist -> return new messages vec.
///
/// The `history` parameter provides DB seq numbers: in-memory message index `i`
/// corresponds to `history[i-1].seq` since index 0 is the synthesized system prompt.
pub async fn try_compact(
    messages: &[ChatMessage],
    tools: &[ToolDescription],
    ctx: &CompactionContext,
    backend: &Arc<dyn LlmBackend>,
    summarization_backend: Option<&Arc<dyn LlmBackend>>,
    pool: &PgPool,
    thread_id: Uuid,
    history: &[af_db::messages::MessageRow],
    agent_name: Option<&str>,
) -> Result<(Vec<ChatMessage>, AgentEvent), AgentError> {
    let estimated = af_llm::estimate_tokens(messages, tools);

    let plan = select_messages_for_compaction(messages)
        .ok_or_else(|| AgentError::Other("not enough messages to compact".into()))?;

    let to_shed = ctx.tokens_to_shed(estimated);
    // Target summary: at most 1/4 of what we're shedding, capped at 2048, floor 256
    let max_summary_tokens = (to_shed / 4).max(256).min(2048);

    let msgs_to_compact = &messages[plan.compact_start..plan.compact_end];
    let summary = compact_messages(msgs_to_compact, backend, summarization_backend, max_summary_tokens).await?;

    // Map in-memory indices to DB seq numbers.
    // messages[0] = system prompt (synthesized, not in history)
    // messages[1] = history[0], messages[2] = history[1], etc.
    // compact_end refers to messages index, so the last compacted message is at compact_end - 1
    // which maps to history[compact_end - 2]
    let up_to_seq = if plan.compact_end >= 2 && (plan.compact_end - 2) < history.len() {
        history[plan.compact_end - 2].seq
    } else if !history.is_empty() {
        history[history.len() - 1].seq
    } else {
        return Err(AgentError::Other("no history rows for compaction".into()));
    };

    // Persist: mark old messages as compacted + insert summary
    af_db::messages::mark_messages_compacted(pool, thread_id, up_to_seq)
        .await
        .map_err(|e| AgentError::Db(format!("mark compacted: {e}")))?;

    af_db::messages::insert_compaction_summary(
        pool,
        thread_id,
        &summary,
        plan.message_count,
        up_to_seq,
        agent_name,
    )
    .await
    .map_err(|e| AgentError::Db(format!("insert compaction summary: {e}")))?;

    // Build new in-memory messages: system + summary-as-user + tail
    let mut new_messages = Vec::new();

    // Keep the system prompt (index 0)
    new_messages.push(messages[0].clone());

    // Insert the summary as a User message (not System, since only one system message is allowed)
    new_messages.push(ChatMessage {
        role: ChatRole::User,
        content: format!(
            "[Context Summary — {} earlier messages were summarized]\n\n{}",
            plan.message_count, summary
        ),
        tool_call_id: None,
        name: None,
        tool_calls: vec![],
        content_parts: None,
    });

    // Keep the tail (preserved recent messages)
    for msg in &messages[plan.compact_end..] {
        new_messages.push(msg.clone());
    }

    let event = AgentEvent::ContextCompacted {
        estimated_tokens: estimated,
        messages_compacted: plan.message_count,
        context_window: ctx.context_window,
    };

    Ok((new_messages, event))
}

/// Deterministic context reset for local models.
///
/// Instead of LLM-based summarization, this function:
/// 1. Selects messages for compaction (reuses existing logic)
/// 2. Marks them as compacted in DB
/// 3. Inserts a marker summary (no LLM call)
/// 4. Rebuilds messages: system prompt + fresh thread memory + preserved tail
///
/// Thread memory serves as the summary — instant, no LLM cost.
pub async fn local_context_reset(
    messages: &[ChatMessage],
    pool: &PgPool,
    thread_id: Uuid,
    history: &[af_db::messages::MessageRow],
    agent_name: Option<&str>,
) -> Result<(Vec<ChatMessage>, AgentEvent), AgentError> {
    let plan = select_messages_for_compaction(messages)
        .ok_or_else(|| AgentError::Other("not enough messages to compact".into()))?;

    let estimated = af_llm::estimate_tokens(messages, &[]);

    // Map in-memory indices to DB seq numbers (same logic as try_compact)
    let up_to_seq = if plan.compact_end >= 2 && (plan.compact_end - 2) < history.len() {
        history[plan.compact_end - 2].seq
    } else if !history.is_empty() {
        history[history.len() - 1].seq
    } else {
        return Err(AgentError::Other("no history rows for compaction".into()));
    };

    // Persist: mark old messages as compacted + insert marker summary
    eprintln!("[thread-memory] reset: marking {} messages compacted (up_to_seq={})",
        plan.message_count, up_to_seq);
    af_db::messages::mark_messages_compacted(pool, thread_id, up_to_seq)
        .await
        .map_err(|e| AgentError::Db(format!("mark compacted: {e}")))?;

    // Re-read thread memory from DB (may have been updated during this run)
    let memory_rows = af_db::thread_memory::get_thread_memory(pool, thread_id)
        .await
        .unwrap_or_default();
    eprintln!("[thread-memory] reset: re-read {} memory entries from DB", memory_rows.len());
    let memory_pairs: Vec<(String, String)> = memory_rows
        .iter()
        .map(|r| (r.key.clone(), r.value.clone()))
        .collect();
    let memory_keys: Vec<String> = memory_pairs.iter().map(|(k, _)| k.clone()).collect();

    let marker = format!(
        "[Context reset | reason=threshold_reset | pre_tokens={} | memory_keys={}]",
        estimated, memory_keys.join(","),
    );
    af_db::messages::insert_compaction_summary(
        pool,
        thread_id,
        &marker,
        plan.message_count,
        up_to_seq,
        agent_name,
    )
    .await
    .map_err(|e| AgentError::Db(format!("insert compaction summary: {e}")))?;

    // Build new in-memory messages: system + memory + tail
    let mut new_messages = Vec::new();

    // Keep the system prompt (index 0)
    new_messages.push(messages[0].clone());

    // Inject thread memory as context
    if let Some(mem_msg) = crate::prompt_builder::build_memory_message(&memory_pairs) {
        new_messages.push(mem_msg);
    }

    // Keep the tail (preserved recent messages)
    let tail_count = messages.len() - plan.compact_end;
    for msg in &messages[plan.compact_end..] {
        new_messages.push(msg.clone());
    }
    eprintln!("[thread-memory] reset: rebuilt messages: system + memory + {} tail = {} messages",
        tail_count, new_messages.len());

    let context_window = 0; // not critical for the event — caller may override
    let event = AgentEvent::ContextCompacted {
        estimated_tokens: estimated,
        messages_compacted: plan.message_count,
        context_window,
    };

    Ok((new_messages, event))
}

/// Token-budget sliding window trim for local models.
///
/// Triggered when estimated tokens exceed 50% of the context budget.
/// Trims old turns to keep context small, preserving at least
/// SLIDING_WINDOW_MIN_TURNS turns and SLIDING_WINDOW_TAIL_TOKENS tokens
/// in the tail. Thread memory preserves findings so trimmed context is
/// not lost.
///
/// Returns the new messages vec, count of messages trimmed, and diagnostic
/// metadata, or None if no trimming was needed.
pub async fn sliding_window_trim(
    messages: &[ChatMessage],
    tools: &[ToolDescription],
    pool: &PgPool,
    thread_id: Uuid,
    history: &[af_db::messages::MessageRow],
    agent_name: Option<&str>,
    compaction_ctx: &CompactionContext,
) -> Result<Option<(Vec<ChatMessage>, usize, TrimMetadata)>, AgentError> {
    let estimated = af_llm::estimate_tokens(messages, tools);
    let trigger = (compaction_ctx.budget() as f32 * SLIDING_WINDOW_BUDGET_FRACTION) as u32;

    if estimated <= trigger {
        return Ok(None); // not enough tokens to warrant trimming
    }

    let body_start = detect_body_start(messages);
    let turns = parse_turns(messages, body_start);
    if turns.len() < 2 {
        return Ok(None); // need at least 2 turns to trim anything
    }

    // Walk turns back-to-front to find the tail boundary
    let mut tail_tokens: u32 = 0;
    let mut tail_turn_count: usize = 0;
    let mut tail_has_user_request = false;
    let mut tail_boundary_turn = turns.len();

    for i in (0..turns.len()).rev() {
        tail_tokens += turns[i].tokens;
        tail_turn_count += 1;
        if turns[i].has_user_request {
            tail_has_user_request = true;
        }
        tail_boundary_turn = i;

        // Stop collecting tail when all conditions met
        if tail_tokens >= SLIDING_WINDOW_TAIL_TOKENS
            && tail_turn_count >= SLIDING_WINDOW_MIN_TURNS
            && tail_has_user_request
        {
            break;
        }
    }

    // If tail_boundary_turn is 0, we'd trim nothing
    if tail_boundary_turn == 0 {
        return Ok(None);
    }

    let safe_trim_end = turns[tail_boundary_turn].start;
    let actual_trim_count = safe_trim_end - body_start;
    if actual_trim_count == 0 {
        return Ok(None);
    }

    let turns_trimmed = tail_boundary_turn;
    let turns_kept = turns.len() - tail_boundary_turn;

    // Map trim boundary to DB seq
    let up_to_seq = map_index_to_seq(safe_trim_end, history)
        .ok_or_else(|| AgentError::Other("no history rows for sliding window".into()))?;

    eprintln!(
        "[sliding-window] trimming {} messages ({} turns), keeping {} turns ({} tokens), trigger={}, estimated={}",
        actual_trim_count, turns_trimmed, turns_kept, tail_tokens, trigger, estimated
    );

    af_db::messages::mark_messages_compacted(pool, thread_id, up_to_seq)
        .await
        .map_err(|e| AgentError::Db(format!("sliding window mark compacted: {e}")))?;

    // Re-read thread memory from DB
    let memory_rows = af_db::thread_memory::get_thread_memory(pool, thread_id)
        .await
        .unwrap_or_default();
    let memory_pairs: Vec<(String, String)> = memory_rows
        .iter()
        .map(|r| (r.key.clone(), r.value.clone()))
        .collect();
    let memory_keys: Vec<String> = memory_pairs.iter().map(|(k, _)| k.clone()).collect();

    let marker = format!(
        "[Sliding window trim | reason=sliding_window | pre_tokens={} | turns_trimmed={} | turns_kept={} | memory_keys={}]",
        estimated, turns_trimmed, turns_kept, memory_keys.join(","),
    );
    af_db::messages::insert_compaction_summary(
        pool,
        thread_id,
        &marker,
        actual_trim_count,
        up_to_seq,
        agent_name,
    )
    .await
    .map_err(|e| AgentError::Db(format!("sliding window insert summary: {e}")))?;

    // Rebuild: system + fresh memory + tail
    let mut new_messages = Vec::new();
    new_messages.push(messages[0].clone());

    if let Some(mem_msg) = crate::prompt_builder::build_memory_message(&memory_pairs) {
        new_messages.push(mem_msg);
    }

    for msg in &messages[safe_trim_end..] {
        new_messages.push(msg.clone());
    }

    let post_tokens = af_llm::estimate_tokens(&new_messages, tools);
    eprintln!(
        "[sliding-window] rebuilt: system + memory + {} tail = {} messages (pre={} post={} tokens)",
        messages.len() - safe_trim_end, new_messages.len(), estimated, post_tokens
    );

    let meta = TrimMetadata {
        reason: TrimReason::SlidingWindow,
        pre_tokens: estimated,
        post_tokens,
        memory_keys,
        turns_trimmed,
        turns_kept,
    };

    Ok(Some((new_messages, actual_trim_count, meta)))
}

/// Pre-flight invariant check before every LLM request (local models only).
///
/// Verifies and auto-repairs the message array:
/// 1. System prompt at messages[0]
/// 2. Memory message at messages[1] if memory_pairs non-empty
/// 3. At least one real User message in body
/// 4. No orphaned Tool messages
pub fn preflight_check(
    messages: &mut Vec<ChatMessage>,
    memory_pairs: &[(String, String)],
    _is_local: bool,
) -> Result<(), AgentError> {
    if messages.is_empty() || messages[0].role != ChatRole::System {
        return Err(AgentError::Other(
            "preflight: messages[0] must be System".into(),
        ));
    }

    // Auto-repair: inject memory message if missing
    if !memory_pairs.is_empty() {
        let has_memory = messages.len() > 1
            && messages[1].role == ChatRole::User
            && messages[1].content.starts_with("[Thread Memory");
        if !has_memory {
            if let Some(mem_msg) = crate::prompt_builder::build_memory_message(memory_pairs) {
                messages.insert(1, mem_msg);
                eprintln!("[preflight] auto-repaired: injected missing memory message");
            }
        }
    }

    // Check for real User message in body
    let body_start = detect_body_start(messages);
    let has_real_user = messages[body_start..].iter().any(|m| {
        m.role == ChatRole::User && !is_nudge_or_reinforcement(&m.content)
    });
    if !has_real_user {
        eprintln!("[preflight] warning: no real User message in body (workflow mode?)");
    }

    // Auto-repair: remove orphaned Tool messages
    let mut in_tool_group = false;
    let mut orphans = Vec::new();
    for (i, msg) in messages.iter().enumerate().skip(body_start) {
        match msg.role {
            ChatRole::Assistant if !msg.tool_calls.is_empty() => {
                in_tool_group = true;
            }
            ChatRole::Tool => {
                if !in_tool_group {
                    orphans.push(i);
                }
            }
            _ => {
                in_tool_group = false;
            }
        }
    }
    if !orphans.is_empty() {
        eprintln!(
            "[preflight] auto-repaired: removing {} orphaned Tool messages",
            orphans.len()
        );
        for &idx in orphans.iter().rev() {
            messages.remove(idx);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use af_core::{ChatMessage, ChatRole, ToolCallInfo};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: vec![],
            content_parts: None,
        }
    }

    fn assistant_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: vec![],
            content_parts: None,
        }
    }

    fn assistant_with_tools(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: vec![ToolCallInfo {
                id: "tc1".into(),
                name: "file.info".into(),
                arguments: serde_json::json!({}),
            }],
            content_parts: None,
        }
    }

    fn tool_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Tool,
            content: content.to_string(),
            tool_call_id: Some("tc1".into()),
            name: Some("file.info".into()),
            tool_calls: vec![],
            content_parts: None,
        }
    }

    fn system_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::System,
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: vec![],
            content_parts: None,
        }
    }

    // --- Existing tests (unchanged) ---

    #[test]
    fn test_too_few_messages() {
        let msgs = vec![system_msg("sys"), user_msg("hi"), assistant_msg("hello")];
        assert!(select_messages_for_compaction(&msgs).is_none());
    }

    #[test]
    fn test_basic_compaction_plan() {
        let msgs = vec![
            system_msg("sys"),
            user_msg("first question"),
            assistant_msg("first answer"),
            user_msg("second question"),
            assistant_msg("second answer"),
            user_msg("third question"),
        ];
        let plan = select_messages_for_compaction(&msgs).unwrap();
        assert_eq!(plan.compact_start, 1);
        assert_eq!(plan.compact_end, 5);
        assert_eq!(plan.message_count, 4);
    }

    #[test]
    fn test_tool_call_group_preserved() {
        let msgs = vec![
            system_msg("sys"),
            user_msg("first"),
            assistant_msg("reply"),
            user_msg("analyze this"),
            assistant_with_tools("calling tool"),
            tool_msg("tool result"),
            user_msg("reinforcement"),
            user_msg("next question"),
        ];
        let plan = select_messages_for_compaction(&msgs).unwrap();
        assert_eq!(plan.compact_end, 4);
        assert_eq!(plan.compact_start, 1);
        assert_eq!(plan.message_count, 3);
    }

    #[test]
    fn test_compaction_context() {
        let ctx = CompactionContext {
            context_window: 100_000,
            max_output_tokens: 4_096,
            threshold: 0.85,
        };
        assert!(!ctx.should_compact(80_000));
        assert!(ctx.should_compact(81_000));
        assert_eq!(ctx.tokens_to_shed(90_000), 90_000 - 80_904);
    }

    // --- New helper tests ---

    #[test]
    fn test_is_nudge_or_reinforcement() {
        assert!(is_nudge_or_reinforcement(
            "[Thread Memory — findings]\n- goal: test"
        ));
        assert!(is_nudge_or_reinforcement(
            "[SYSTEM REMINDER] Tool output above..."
        ));
        assert!(is_nudge_or_reinforcement(
            "[Context Summary — 5 messages]\nsummary"
        ));
        assert!(is_nudge_or_reinforcement(
            "[Sliding window trim | reason=sliding_window]"
        ));
        assert!(is_nudge_or_reinforcement(
            "[Context reset | reason=threshold_reset]"
        ));
        assert!(is_nudge_or_reinforcement(
            "---\nContinue your analysis. Use the tool results..."
        ));
        assert!(!is_nudge_or_reinforcement("Analyze the malware sample"));
        assert!(!is_nudge_or_reinforcement("Show me FUN_00102540"));
    }

    #[test]
    fn test_detect_body_start_with_memory() {
        let msgs = vec![
            system_msg("sys"),
            user_msg("[Thread Memory — findings]\nTASK: test"),
            user_msg("real question"),
        ];
        assert_eq!(detect_body_start(&msgs), 2);
    }

    #[test]
    fn test_detect_body_start_without_memory() {
        let msgs = vec![system_msg("sys"), user_msg("real question")];
        assert_eq!(detect_body_start(&msgs), 1);
    }

    #[test]
    fn test_parse_turns_basic() {
        let msgs = vec![
            system_msg("sys"),
            user_msg("first question"),
            assistant_msg("first answer"),
            user_msg("second question"),
            assistant_with_tools("calling tool"),
            tool_msg("result"),
        ];
        let turns = parse_turns(&msgs, 1);
        assert_eq!(turns.len(), 2);
        assert!(turns[0].has_user_request);
        assert_eq!(turns[0].start, 1);
        assert_eq!(turns[0].end, 3);
        assert!(turns[1].has_user_request);
        assert_eq!(turns[1].start, 3);
        assert_eq!(turns[1].end, 6);
    }

    #[test]
    fn test_parse_turns_nudge_not_new_turn() {
        let msgs = vec![
            system_msg("sys"),
            user_msg("analyze"),
            assistant_with_tools("calling"),
            tool_msg("result"),
            user_msg("[SYSTEM REMINDER] untrusted data..."),
            user_msg("next question"),
        ];
        let turns = parse_turns(&msgs, 1);
        assert_eq!(turns.len(), 2);
        // First turn: "analyze" + tool call + nudge
        assert_eq!(turns[0].start, 1);
        assert_eq!(turns[0].end, 5);
        assert!(turns[0].has_user_request);
        // Second turn: "next question"
        assert_eq!(turns[1].start, 5);
        assert!(turns[1].has_user_request);
    }

    #[test]
    fn test_parse_turns_no_user() {
        let msgs = vec![
            system_msg("sys"),
            assistant_with_tools("calling"),
            tool_msg("result"),
            assistant_msg("done"),
        ];
        let turns = parse_turns(&msgs, 1);
        assert_eq!(turns.len(), 1);
        assert!(!turns[0].has_user_request);
    }

    #[test]
    fn test_parse_turns_tokens() {
        // Each message gets estimate_content_tokens() + 4 overhead
        let msgs = vec![
            system_msg("sys"),
            user_msg("hello world"), // ~11 chars / 4 = ~2 tokens + 4 = 6
            assistant_msg("ok"),     // ~2 chars / 4 = ~0 tokens + 4 = 4
        ];
        let turns = parse_turns(&msgs, 1);
        assert_eq!(turns.len(), 1);
        assert!(turns[0].tokens > 0);
    }

    #[test]
    fn test_sliding_window_constants() {
        assert!(SLIDING_WINDOW_TAIL_TOKENS >= 4000);
        assert!(SLIDING_WINDOW_TAIL_TOKENS <= 10000);
        assert!(SLIDING_WINDOW_MIN_TURNS >= 2);
        assert!(SLIDING_WINDOW_BUDGET_FRACTION > 0.0);
        assert!(SLIDING_WINDOW_BUDGET_FRACTION < 1.0);
    }

    #[test]
    fn test_preflight_check_ok() {
        let mut msgs = vec![
            system_msg("sys"),
            user_msg("[Thread Memory — findings]\nTASK: test"),
            user_msg("real question"),
        ];
        let pairs = vec![("goal".to_string(), "test".to_string())];
        assert!(preflight_check(&mut msgs, &pairs, true).is_ok());
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_preflight_check_missing_system() {
        let mut msgs = vec![user_msg("oops")];
        assert!(preflight_check(&mut msgs, &[], true).is_err());
    }

    #[test]
    fn test_preflight_check_injects_missing_memory() {
        let mut msgs = vec![
            system_msg("sys"),
            user_msg("real question"),
        ];
        let pairs = vec![("goal".to_string(), "test goal".to_string())];
        assert!(preflight_check(&mut msgs, &pairs, true).is_ok());
        // Memory should have been injected at index 1
        assert_eq!(msgs.len(), 3);
        assert!(msgs[1].content.starts_with("[Thread Memory"));
    }

    #[test]
    fn test_preflight_check_orphaned_tool() {
        let mut msgs = vec![
            system_msg("sys"),
            user_msg("question"),
            tool_msg("orphaned result"),
            assistant_msg("answer"),
        ];
        assert!(preflight_check(&mut msgs, &[], true).is_ok());
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, ChatRole::Assistant);
    }

    #[test]
    fn test_preflight_check_valid_tool_not_orphaned() {
        let mut msgs = vec![
            system_msg("sys"),
            user_msg("question"),
            assistant_with_tools("calling"),
            tool_msg("valid result"),
        ];
        assert!(preflight_check(&mut msgs, &[], true).is_ok());
        assert_eq!(msgs.len(), 4); // no removal
    }

    #[test]
    fn test_map_index_to_seq() {
        let rows = vec![
            af_db::messages::MessageRow {
                id: uuid::Uuid::nil(),
                thread_id: uuid::Uuid::nil(),
                role: "user".into(),
                content: None,
                content_json: None,
                tool_call_id: None,
                tool_name: None,
                agent_name: None,
                seq: 10,
                created_at: chrono::Utc::now(),
            },
            af_db::messages::MessageRow {
                id: uuid::Uuid::nil(),
                thread_id: uuid::Uuid::nil(),
                role: "assistant".into(),
                content: None,
                content_json: None,
                tool_call_id: None,
                tool_name: None,
                agent_name: None,
                seq: 20,
                created_at: chrono::Utc::now(),
            },
        ];
        // exclusive_end=2: last trimmed = messages[1] = history[0] → seq 10
        assert_eq!(map_index_to_seq(2, &rows), Some(10));
        // exclusive_end=3: last trimmed = messages[2] = history[1] → seq 20
        assert_eq!(map_index_to_seq(3, &rows), Some(20));
        // empty history
        assert_eq!(map_index_to_seq(2, &[]), None);
    }
}
