// Compaction (ADR 0023): shapes the derived Context Working Set when token_count
// nears the model window (~75%). Never destroys the persisted Session.
use crate::message::{Message, MessageItem, Role};
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
}
