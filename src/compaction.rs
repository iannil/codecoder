// Compaction (ADR 0023): shapes the derived Context Working Set when token_count
// nears the model window (~75%). Never destroys the persisted Session.
use crate::message::{Message, MessageId, MessageItem, Role};
use crate::tokenizer::count_tokens;
use std::collections::BTreeSet;

/// Fraction of the model window at which compaction kicks in.
pub const COMPACTION_THRESHOLD: f32 = 0.75;

/// Recent messages kept fully intact — compaction only touches older history.
const RECENT_TAIL: usize = 6;

pub fn should_compact(token_count: u64, model_window: u64) -> bool {
    model_window > 0 && token_count as f32 >= model_window as f32 * COMPACTION_THRESHOLD
}

/// Derive the working set sent to the provider from the full-fidelity messages.
///
/// Implements **tier 1** of ADR 0023: when the full history crosses the threshold,
/// drop `Reasoning` items and elide old `ToolResult` bodies. The first user message
/// (the anchor / original goal) and the most recent `RECENT_TAIL` messages are never
/// touched.
///
/// When `compaction_tier2` is true and `provider` is `Some`, after tier-1 processing
/// the function checks whether the tier-1 result is still over the window threshold.
/// If so, it summarises the oldest dialogue span (between the anchor and the last
/// user message) via `tier2_summarize`, replaces that span with a synthetic `System`
/// message, and returns the combined result. This is the **no-cache** path; callers
/// that want in-memory caching (e.g. `AgentLoop::context_working_set`) should pass
/// `compaction_tier2: false` and manage tier-2 themselves.
///
/// Two subtleties drive the design (see ADR 0023):
/// - The decision is made against the **full** history size, never the compacted
///   size — otherwise compaction would lower the count, un-trip the threshold next
///   turn, and oscillate.
/// - `Reasoning` is already skipped by the provider wire layer, so evicting it does
///   not shrink the real request; it realigns `count_tokens` (which does count it)
///   with what is actually sent. `ToolResult` elision is what shrinks the payload.
pub fn working_set(
    model: &str,
    messages: &[Message],
    model_window: u64,
    provider: Option<&dyn crate::provider::Provider>,
    compaction_tier2: bool,
) -> Vec<Message> {
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

    // Tier-2: if still over threshold and enabled, summarise the oldest span.
    if compaction_tier2 {
        if let Some(prov) = provider {
            if should_compact(count_tokens(model, &out), model_window) {
                if let Some((start, end)) = summary_span(&out) {
                    let anchor_id = out[start - 1].id;
                    let covered_last_id = out[end - 1].id;
                    // Build refs for the summarizable span.
                    let span_refs: Vec<&Message> = out[start..end].iter().collect();
                    // Check tier2_should_run against the full working set, not just the span.
                    let out_refs: Vec<&Message> = out.iter().collect();
                    if tier2_should_run(&out_refs, 0.5, RECENT_TAIL).is_some() {
                        if let Ok(summary) = tier2_summarize(prov, &span_refs, model) {
                            return apply_tier2(&out, anchor_id, covered_last_id, &summary);
                        }
                    }
                }
            }
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

/// 扫描 span 内的 ToolCall，按工具名把 `path` 参数分入读/改集合。
/// `read_file` → `read`；`write_file`/`edit_file` → `modified`。就地累积，
/// 便于跨多次 compaction 叠加历史。
pub fn collect_file_paths(span: &[Message], read: &mut BTreeSet<String>, modified: &mut BTreeSet<String>) {
    for m in span {
        for it in &m.items {
            if let MessageItem::ToolCall { name, args, .. } = it {
                let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                    continue;
                };
                match name.as_str() {
                    "read_file" => {
                        read.insert(path.to_string());
                    }
                    "write_file" | "edit_file" => {
                        modified.insert(path.to_string());
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 把读/改文件集合渲染成附加在摘要末尾的块。两集合都空时返回空串（不占 token）。
pub fn render_file_blocks(read: &BTreeSet<String>, modified: &BTreeSet<String>) -> String {
    if read.is_empty() && modified.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n");
    if !read.is_empty() {
        s.push_str("<read-files>\n");
        for p in read {
            s.push_str(p);
            s.push('\n');
        }
        s.push_str("</read-files>\n");
    }
    if !modified.is_empty() {
        s.push_str("<modified-files>\n");
        for p in modified {
            s.push_str(p);
            s.push('\n');
        }
        s.push_str("</modified-files>\n");
    }
    s
}

/// Determine if tier-2 compaction should run after tier-1.
/// Returns the index of the first message that can be summarized (the oldest span
/// that is not the anchor goal and not the recent tail).
///
/// The anchor is the first user message (the original goal, never evicted).
/// The tail is the last ~window_size messages (preserved for ongoing conversation context).
/// Everything between them is eligible for summarization.
///
/// `working_set` is the tier-1 compacted working set. `threshold_pct` is the
/// fraction of the span-between-anchor-and-tail that must be exceeded (e.g. 0.5
/// means at least half of the eligible messages must exist). Returns the index
/// (into `working_set`) of the first message to summarize, i.e. `anchor + 1`,
/// or `None` when there is nothing to summarize.
pub fn tier2_should_run(
    working_set: &[&Message],
    threshold_pct: f64,
    window_size: usize,
) -> Option<usize> {
    let anchor = working_set.iter().position(|m| m.role == Role::User)?;
    let tail_start = working_set.len().saturating_sub(window_size);
    // The eligible span is [anchor+1, tail_start). It must be non-empty.
    let span_start = anchor + 1;
    if span_start >= tail_start {
        return None;
    }
    // Check that the eligible span exceeds the threshold fraction of all messages
    // between anchor and tail.
    let total_eligible = tail_start - span_start;
    if total_eligible == 0 {
        return None;
    }
    // threshold_pct is a fraction of the total working set length; if the eligible
    // span is large enough relative to the working set, trigger.
    let working_len = working_set.len();
    if working_len == 0 {
        return None;
    }
    let ratio = total_eligible as f64 / working_len as f64;
    if ratio >= threshold_pct {
        Some(span_start)
    } else {
        None
    }
}

/// Build a summarization prompt for an LLM from a message span.
/// Instructs the LLM to extract: what was accomplished, what decisions were made,
/// what remaining work exists, and any important context.
pub fn build_summary_prompt(span: &[&Message]) -> String {
    let text = render_span(
        &span.iter().map(|m| (*m).clone()).collect::<Vec<_>>(),
    );
    format!(
        "You are a conversation summarizer. The following is a conversation span \
         between a user and an AI agent. Summarize the key information: what was \
         asked, what was done, what decisions were made, what files were changed, \
         and what remaining work exists. Be concise but comprehensive.\n\n\
         CONVERSATION:\n{text}\n\n\
         SUMMARY:"
    )
}

/// Perform tier-2 summarization: call the LLM to summarize the oldest conversation span.
/// Returns the summary text on success, or an error string.
pub fn tier2_summarize(
    provider: &dyn crate::provider::Provider,
    span: &[&Message],
    model: &str,
) -> Result<String, String> {
    let prompt = build_summary_prompt(span);
    let req = crate::provider::CompletionRequest {
        model: model.to_string(),
        messages: vec![crate::message::Message {
            id: 0,
            role: crate::message::Role::User,
            items: vec![crate::message::MessageItem::Text { text: prompt }],
        }],
        max_tokens: 1024,
        temperature: 0.3,
        tools: vec![],
    };
    match provider.complete(&req) {
        Ok(completion) => {
            let text = completion.message.items.iter().filter_map(|it| {
                if let crate::message::MessageItem::Text { text } = it {
                    Some(text.as_str())
                } else {
                    None
                }
            }).collect::<Vec<_>>().join("\n").trim().to_string();
            if text.is_empty() {
                return Err("tier-2 summary returned empty text".into());
            }
            Ok(text)
        }
        Err(e) => Err(format!("tier-2 LLM call failed: {e}")),
    }
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
        let out = working_set("gpt-4o", &msgs, 1_000_000, None, false);
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
        let out = working_set("gpt-4o", &msgs, 10, None, false);

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

    #[test]
    fn collect_file_paths_splits_read_and_modified_and_dedups() {
        use std::collections::BTreeSet;
        fn call(id: &str, name: &str, args: serde_json::Value) -> Message {
            msg(0, Role::Assistant, vec![MessageItem::ToolCall { id: id.into(), name: name.into(), args }])
        }
        let span = vec![
            call("c1", "read_file", json!({ "path": "a.rs" })),
            call("c2", "edit_file", json!({ "path": "b.rs", "old": "x", "new": "y" })),
            call("c3", "write_file", json!({ "path": "b.rs", "content": "z" })), // dup modified
            call("c4", "run_command", json!({ "cmd": "ls" })),                    // no path
            call("c5", "read_file", json!({})),                                  // missing path
        ];
        let mut read = BTreeSet::new();
        let mut modified = BTreeSet::new();
        collect_file_paths(&span, &mut read, &mut modified);
        assert_eq!(read.into_iter().collect::<Vec<_>>(), vec!["a.rs".to_string()]);
        assert_eq!(modified.into_iter().collect::<Vec<_>>(), vec!["b.rs".to_string()]);
    }

    #[test]
    fn render_file_blocks_omits_empty_and_formats_present() {
        use std::collections::BTreeSet;
        let empty = BTreeSet::new();
        assert_eq!(render_file_blocks(&empty, &empty), "");

        let read: BTreeSet<String> = ["a.rs".to_string(), "b.rs".to_string()].into_iter().collect();
        let modified: BTreeSet<String> = ["c.rs".to_string()].into_iter().collect();
        let s = render_file_blocks(&read, &modified);
        assert!(s.starts_with("\n\n"));
        assert!(s.contains("<read-files>\na.rs\nb.rs\n</read-files>"));
        assert!(s.contains("<modified-files>\nc.rs\n</modified-files>"));

        // 只有 read 非空时不渲染 modified 块。
        let only_read = render_file_blocks(&read, &empty);
        assert!(only_read.contains("<read-files>"));
        assert!(!only_read.contains("<modified-files>"));
    }

    // ── tier-2 tests ──────────────────────────────────────────────────────

    #[test]
    fn tier2_should_run_returns_none_when_below_threshold() {
        // 5 messages: anchor + 4 assistant. window_size=3 → tail_start=2, eligible span
        // [1, 2) = 1 message. eligible_len=1, working_len=5, ratio=0.2 < 0.5 → None.
        let anchor = msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]);
        let asst1 = msg(1, Role::Assistant, vec![MessageItem::Text { text: "a".into() }]);
        let asst2 = msg(2, Role::Assistant, vec![MessageItem::Text { text: "b".into() }]);
        let asst3 = msg(3, Role::Assistant, vec![MessageItem::Text { text: "c".into() }]);
        let asst4 = msg(4, Role::Assistant, vec![MessageItem::Text { text: "d".into() }]);

        let refs: Vec<&Message> = vec![&anchor, &asst1, &asst2, &asst3, &asst4];
        // window_size=3 → tail starts at index 2 (asst2, asst3, asst4 are tail).
        // Eligible span = [1, 2) = {asst1} = 1 msg. 1/5 = 0.2 < 0.5.
        let result = tier2_should_run(&refs, 0.5, 3);
        assert_eq!(result, None);
    }

    #[test]
    fn tier2_should_run_returns_index_when_above_threshold() {
        // 10 messages: anchor + 9 assistant. window_size=3 → tail_start=7.
        // Eligible span = [1, 7) = 6 messages. 6/10 = 0.6 >= 0.5 → Some(1).
        let anchor = msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]);
        let mut refs: Vec<Message> = vec![anchor];
        for i in 1..10 {
            refs.push(msg(i as u64, Role::Assistant, vec![MessageItem::Text { text: "x".into() }]));
        }
        let refs_ref: Vec<&Message> = refs.iter().collect();
        let result = tier2_should_run(&refs_ref, 0.5, 3);
        assert_eq!(result, Some(1));
    }

    #[test]
    fn tier2_should_run_none_when_tail_covers_almost_everything() {
        // 3 messages: anchor + 2 assistant. window_size=3 → tail_start=0, so
        // span_start=1 >= tail_start=0 → early return None (nothing eligible).
        let anchor = msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]);
        let asst1 = msg(1, Role::Assistant, vec![MessageItem::Text { text: "a".into() }]);
        let asst2 = msg(2, Role::Assistant, vec![MessageItem::Text { text: "b".into() }]);
        let refs: Vec<&Message> = vec![&anchor, &asst1, &asst2];
        let result = tier2_should_run(&refs, 0.0, 3);
        // tail_start = 0, anchor_start+1 = 1 >= 0 → None
        assert_eq!(result, None);
    }

    #[test]
    fn build_summary_prompt_contains_messages_and_has_expected_structure() {
        let msgs = vec![
            msg(1, Role::User, vec![MessageItem::Text { text: "hello".into() }]),
            msg(2, Role::Assistant, vec![MessageItem::Text { text: "world".into() }]),
        ];
        let refs: Vec<&Message> = msgs.iter().collect();
        let prompt = build_summary_prompt(&refs);
        assert!(prompt.contains("hello"));
        assert!(prompt.contains("world"));
        assert!(prompt.contains("conversation summarizer"));
        assert!(prompt.contains("CONVERSATION:"));
        assert!(prompt.contains("SUMMARY:"));
    }

    #[test]
    fn build_summary_prompt_renders_tool_calls_and_results() {
        let msgs = vec![
            msg(1, Role::Assistant, vec![
                MessageItem::ToolCall { id: "c1".into(), name: "read_file".into(), args: serde_json::json!({"path": "a.rs"}) },
            ]),
            msg(2, Role::Tool, vec![
                MessageItem::ToolResult { call_id: "c1".into(), output: "file content".into(), is_error: false },
            ]),
        ];
        let refs: Vec<&Message> = msgs.iter().collect();
        let prompt = build_summary_prompt(&refs);
        assert!(prompt.contains("[tool_call read_file]"));
        assert!(prompt.contains("[result:"));
        assert!(prompt.contains("file content"));
    }

    #[test]
    fn tier2_summarize_uses_stub_provider() {
        use crate::provider::stub::StubClient;
        let msgs = vec![
            msg(1, Role::User, vec![MessageItem::Text { text: "hello".into() }]),
        ];
        let refs: Vec<&Message> = msgs.iter().collect();
        let provider = StubClient;
        let result = tier2_summarize(&provider, &refs, "gpt-4o");
        // StubClient returns a deterministic text, so it should succeed.
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert!(!summary.is_empty());
        assert!(summary.contains("stub"));
    }

    #[test]
    fn working_set_tier2_integration() {
        // Build a long history that will trigger BOTH tier-1 and tier-2.
        // Use StubClient as the provider — it returns deterministic text containing "stub".
        let mut msgs = vec![msg(
            0,
            Role::User,
            vec![MessageItem::Text { text: "original goal".into() }],
        )];
        // Compactable region: many assistant messages so that after tier-1 the eligible
        // span ratio (eligible / working_len) >= 0.5, triggering tier2_should_run.
        for i in 1..8 {
            msgs.push(msg(i, Role::Assistant, vec![MessageItem::Text { text: "x".into() }]));
        }
        // A user message to define the end of the summarizable span.
        msgs.push(msg(8, Role::User, vec![MessageItem::Text { text: "next turn".into() }]));
        // Recent tail (should survive untouched) — RECENT_TAIL=6.
        for i in 9..15 {
            msgs.push(msg(i, Role::Assistant, vec![MessageItem::Text { text: "t".into() }]));
        }

        // Force compaction with a tiny window.
        let provider = crate::provider::stub::StubClient;
        let out = working_set("gpt-4o", &msgs, 10, Some(&provider), true);

        // Anchor survives.
        assert!(matches!(&out[0].items[0], MessageItem::Text { text } if text == "original goal"));
        // The summarizable span (ids 1..7) is replaced by a System summary.
        assert!(out.iter().any(|m| m.role == Role::System));
        // The summary contains the stub output.
        let sys = out.iter().find(|m| m.role == Role::System).unwrap();
        let sys_text = sys.items.iter().filter_map(|it| match it {
            MessageItem::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join(" ");
        assert!(sys_text.contains("stub"), "summary should contain StubClient output: {sys_text}");
        // Covered ids (1..=7) are gone.
        for id in 1..8 {
            assert!(!out.iter().any(|m| m.id == id), "id {id} should have been replaced by summary");
        }
        // Tail survives.
        assert!(out.iter().any(|m| m.id == 9));
        // User message (id 8) is the last user and should be the current-turn prompt, NOT covered.
        assert!(out.iter().any(|m| m.id == 8));
    }

    #[test]
    fn working_set_tier2_disabled_does_not_summarize() {
        // Same setup as above, but compaction_tier2=false → no System summary.
        let mut msgs = vec![msg(
            0,
            Role::User,
            vec![MessageItem::Text { text: "original goal".into() }],
        )];
        msgs.push(msg(1, Role::Assistant, vec![MessageItem::Text { text: "thinking".into() }]));
        msgs.push(msg(5, Role::User, vec![MessageItem::Text { text: "next turn".into() }]));
        for i in 6..12 {
            msgs.push(msg(i, Role::Assistant, vec![MessageItem::Text { text: "t".into() }]));
        }

        let provider = crate::provider::stub::StubClient;
        let out = working_set("gpt-4o", &msgs, 10, Some(&provider), false);

        // No System summary — tier-1 only.
        assert!(!out.iter().any(|m| m.role == Role::System));
        // Anchor survives.
        assert!(out.iter().any(|m| m.id == 0));
    }
}
