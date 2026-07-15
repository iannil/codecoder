// Compaction (ADR 0023): shapes the derived Context Working Set when token_count
// nears the model window (~75%). Never destroys the persisted Session.
use crate::message::{Message, MessageId, MessageItem, Role};
use crate::tokenizer::count_tokens;

/// Fraction of the model window at which compaction kicks in.
pub const COMPACTION_THRESHOLD: f32 = 0.75;

/// Recent messages kept fully intact — compaction only touches older history.
const RECENT_TAIL: usize = 6;

pub fn should_compact(token_count: u64, model_window: u64) -> bool {
    model_window > 0 && token_count as f32 >= model_window as f32 * COMPACTION_THRESHOLD
}

/// Derive the working set sent to the provider from the full-fidelity messages.
///
/// v1 implements **tier 1** of ADR 0023 only: when the full history crosses the
/// threshold, drop `Reasoning` items and elide old `ToolResult` bodies. The first
/// user message (the anchor / original goal) and the most recent `RECENT_TAIL`
/// messages are never touched. Tier 2 (summarizing the oldest dialogue span into a
/// synthetic System summary) is still deferred.
///
/// Two subtleties drive the design (see ADR 0023):
/// - The decision is made against the **full** history size, never the compacted
///   size — otherwise compaction would lower the count, un-trip the threshold next
///   turn, and oscillate.
/// - `Reasoning` is already skipped by the provider wire layer, so evicting it does
///   not shrink the real request; it realigns `count_tokens` (which does count it)
///   with what is actually sent. `ToolResult` elision is what shrinks the payload.
pub fn working_set(model: &str, messages: &[Message], model_window: u64) -> Vec<Message> {
    if !should_compact(count_tokens(model, messages), model_window) {
        return messages.to_vec();
    }

    let anchor = messages.iter().position(|m| m.role == Role::User);
    let tail_start = messages.len().saturating_sub(RECENT_TAIL);
    let mut out = Vec::with_capacity(messages.len());

    for (i, m) in messages.iter().enumerate() {
        if Some(i) == anchor || i >= tail_start {
            out.push(m.clone());
            continue;
        }
        let mut items = Vec::with_capacity(m.items.len());
        for it in &m.items {
            match it {
                // Never replayed to the provider anyway — dropping realigns the counter.
                MessageItem::Reasoning { .. } => {}
                // Body elided, but the item is KEPT: OpenAI requires every tool_call to
                // have a matching tool_call_id response, so the pairing must survive.
                MessageItem::ToolResult { call_id, output, is_error } => {
                    items.push(MessageItem::ToolResult {
                        call_id: call_id.clone(),
                        output: format!("[elided by compaction: {} chars]", output.len()),
                        is_error: *is_error,
                    });
                }
                other => items.push(other.clone()),
            }
        }
        // A message reduced to nothing (was Reasoning-only) is dropped entirely; it
        // carried no ToolCall, so no correlation pairing is broken.
        if !items.is_empty() {
            out.push(Message { id: m.id, role: m.role, items });
        }
    }
    out
}

/// The oldest summarizable dialogue span: everything between the anchor (first
/// user message = original goal) and the current turn (last user message). Returns
/// half-open indices `[anchor+1, last_user)`, or `None` when there is no earlier
/// turn to summarize (only one user message, or the two users are adjacent).
pub fn summary_span(messages: &[Message]) -> Option<(usize, usize)> {
    let anchor = messages.iter().position(|m| m.role == Role::User)?;
    let last_user = messages.iter().rposition(|m| m.role == Role::User)?;
    let start = anchor + 1;
    if start >= last_user {
        return None;
    }
    Some((start, last_user))
}

/// Render a span into bounded plain text for the summary prompt: drop `Reasoning`,
/// mark `ToolCall`s, and truncate each `ToolResult` body so a huge old span cannot
/// blow the summary call's own token budget.
pub fn render_span(span: &[Message]) -> String {
    fn role_str(r: Role) -> &'static str {
        match r {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        }
    }
    let mut s = String::new();
    for m in span {
        let role = role_str(m.role);
        for it in &m.items {
            match it {
                MessageItem::Text { text } => {
                    s.push_str(role);
                    s.push_str(": ");
                    s.push_str(text);
                    s.push('\n');
                }
                MessageItem::Reasoning { .. } => {}
                MessageItem::ToolCall { name, .. } => {
                    s.push_str(&format!("{role}: [tool_call {name}]\n"));
                }
                MessageItem::ToolResult { output, .. } => {
                    let snippet: String = output.chars().take(2000).collect();
                    s.push_str(&format!("tool: [result: {snippet}]\n"));
                }
            }
        }
    }
    s
}

/// Rewrite the tier-1 result: drop every message whose id is in
/// `(anchor_id, covered_last_id]` and insert one synthetic `System` summary right
/// after the anchor. Works by **id** because tier-1 may have dropped Reasoning-only
/// messages, so raw indices no longer line up with the original history.
pub fn apply_tier2(
    tier1: &[Message],
    anchor_id: MessageId,
    covered_last_id: MessageId,
    summary: &str,
) -> Vec<Message> {
    let mut out = Vec::with_capacity(tier1.len());
    let mut inserted = false;
    for m in tier1 {
        if m.id > anchor_id && m.id <= covered_last_id {
            if !inserted {
                out.push(Message::text(
                    MessageId::MAX,
                    Role::System,
                    format!("先前对话摘要：\n{summary}"),
                ));
                inserted = true;
            }
            continue; // covered span replaced by the summary
        }
        out.push(m.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(id: u64, role: Role, items: Vec<MessageItem>) -> Message {
        Message { id, role, items }
    }

    #[test]
    fn below_threshold_returns_full_history_unchanged() {
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "hi".into() }]),
            msg(1, Role::Assistant, vec![MessageItem::Reasoning { text: "think".into() }]),
        ];
        // Huge window → never compacts.
        let out = working_set("gpt-4o", &msgs, 1_000_000);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[1].items[0], MessageItem::Reasoning { .. }));
    }

    #[test]
    fn tier1_drops_reasoning_and_elides_toolresult_but_protects_anchor_and_tail() {
        // Build a long history so the tail protection leaves older items to compact.
        let mut msgs = vec![msg(
            0,
            Role::User,
            vec![MessageItem::Text { text: "original goal".into() }],
        )];
        // Older, compactable region: a reasoning-only msg + a tool call/result pair.
        msgs.push(msg(1, Role::Assistant, vec![MessageItem::Reasoning { text: "secret cot".into() }]));
        msgs.push(msg(
            2,
            Role::Assistant,
            vec![MessageItem::ToolCall { id: "c1".into(), name: "read_file".into(), args: json!({}) }],
        ));
        msgs.push(msg(
            3,
            Role::Tool,
            vec![MessageItem::ToolResult { call_id: "c1".into(), output: "x".repeat(500), is_error: false }],
        ));
        // Recent tail (untouched) — pad past RECENT_TAIL.
        for i in 4..12 {
            msgs.push(msg(i, Role::Assistant, vec![MessageItem::Text { text: "t".into() }]));
        }

        // Tiny window forces compaction.
        let out = working_set("gpt-4o", &msgs, 10);

        // Anchor (first user msg) survives verbatim.
        assert!(matches!(&out[0].items[0], MessageItem::Text { text } if text == "original goal"));
        // Reasoning-only message (id 1) is dropped entirely.
        assert!(!out.iter().any(|m| m.id == 1));
        // The ToolCall is preserved (correlation must survive).
        assert!(out.iter().any(|m| matches!(&m.items[0], MessageItem::ToolCall { id, .. } if id == "c1")));
        // Its ToolResult is kept but the body is elided.
        let tr = out
            .iter()
            .find_map(|m| m.items.iter().find_map(|it| match it {
                MessageItem::ToolResult { call_id, output, .. } if call_id == "c1" => Some(output.clone()),
                _ => None,
            }))
            .expect("tool result should still be present");
        assert!(tr.starts_with("[elided by compaction:"), "got: {tr}");
    }

    #[test]
    fn summary_span_selects_between_first_and_last_user() {
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),      // anchor
            msg(1, Role::Assistant, vec![MessageItem::Text { text: "a".into() }]),
            msg(2, Role::User, vec![MessageItem::Text { text: "mid".into() }]),
            msg(3, Role::Assistant, vec![MessageItem::Text { text: "b".into() }]),
            msg(4, Role::User, vec![MessageItem::Text { text: "current".into() }]),   // last user
        ];
        assert_eq!(summary_span(&msgs), Some((1, 4)));
    }

    #[test]
    fn summary_span_none_when_single_turn() {
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),
            msg(1, Role::Assistant, vec![MessageItem::Text { text: "a".into() }]),
        ];
        assert_eq!(summary_span(&msgs), None); // only one user → nothing older to summarize
    }

    #[test]
    fn summary_span_none_when_users_adjacent() {
        // anchor at 0, last user at 1 → span (1,1) is empty → None.
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),
            msg(1, Role::User, vec![MessageItem::Text { text: "again".into() }]),
        ];
        assert_eq!(summary_span(&msgs), None);
    }

    #[test]
    fn apply_tier2_replaces_covered_range_by_id_and_keeps_anchor_and_tail() {
        // tier1 with a Reasoning-only message ALREADY dropped (id 2 missing) to prove
        // apply_tier2 works by id, not raw index.
        let tier1 = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),          // anchor
            msg(1, Role::Assistant, vec![MessageItem::Text { text: "OLD".into() }]),       // covered
            // id 2 (Reasoning-only) dropped by tier-1
            msg(3, Role::Tool, vec![MessageItem::ToolResult { call_id: "c".into(), output: "[elided]".into(), is_error: false }]), // covered
            msg(4, Role::User, vec![MessageItem::Text { text: "current".into() }]),        // tail
            msg(5, Role::Assistant, vec![MessageItem::Text { text: "reply".into() }]),     // tail
        ];
        let out = apply_tier2(&tier1, 0, 3, "SUMMARY");
        // anchor kept
        assert!(matches!(&out[0].items[0], MessageItem::Text { text } if text == "goal"));
        // summary inserted right after anchor
        assert!(matches!(&out[1], Message { role: Role::System, .. }));
        assert!(matches!(&out[1].items[0], MessageItem::Text { text } if text.contains("SUMMARY")));
        // covered ids (1, 3) gone
        assert!(!out.iter().any(|m| m.id == 1 || m.id == 3));
        // tail preserved
        assert!(out.iter().any(|m| m.id == 4));
        assert!(out.iter().any(|m| m.id == 5));
    }

    #[test]
    fn render_span_drops_reasoning_and_truncates_tool_results() {
        let span = vec![
            msg(1, Role::Assistant, vec![
                MessageItem::Text { text: "hello".into() },
                MessageItem::Reasoning { text: "SECRET".into() },
            ]),
            msg(2, Role::Tool, vec![MessageItem::ToolResult { call_id: "c".into(), output: "x".repeat(5000), is_error: false }]),
        ];
        let s = render_span(&span);
        assert!(s.contains("hello"));
        assert!(!s.contains("SECRET"));        // reasoning omitted
        assert!(s.len() > 1000);               // keeps well past the old 200 cap
        assert!(s.len() < 2200);               // but still truncated near 2000
    }
}
