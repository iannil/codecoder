// L1 — INTERACTION / local scratch (§5.8).
//
// Two behaviors, exercised against the real AgentLoop + real builtin tools:
//  1. `ask_user`: the user's answer round-trips back to the provider as a
//     ToolResult on a subsequent request (the model "hears" the reply).
//  2. `memory` set: persists a `memory/<key>` file on disk (filesystem-as-self).
//
// Schema calibration (against src/):
//  - `ask_user` arg key is `question` (not `prompt`) — src/tool/builtin.rs:707,
//    src/agent.rs:495. The AskUser *event* carries the text in its `prompt`
//    field, but the tool *argument* is `question`.
//  - `memory` args are `action`/`key`/`value` with actions get/set/list/delete
//    — src/tool/dev.rs:238. Each key persists to the bare file `memory/<key>`
//    (no extension) — src/memory.rs:16 `dir(root).join(key)`.

mod testkit;
use serde_json::json;
use testkit::*;

use codecoder::{Message, MessageItem};

/// Extract concatenated text from the public `MessageItem` variants — more
/// robust than a `{:?}` Debug string. Copied from tests/l1_kernel.rs.
fn dump(msgs: &[Message]) -> String {
    let mut s = String::new();
    for m in msgs {
        for item in &m.items {
            match item {
                MessageItem::Text { text } => {
                    s.push_str(text);
                    s.push('\n');
                }
                MessageItem::Reasoning { text } => {
                    s.push_str(text);
                    s.push('\n');
                }
                MessageItem::ToolCall { name, args, .. } => {
                    s.push_str(name);
                    s.push(' ');
                    s.push_str(&args.to_string());
                    s.push('\n');
                }
                MessageItem::ToolResult { output, .. } => {
                    s.push_str(output);
                    s.push('\n');
                }
            }
        }
    }
    s
}

#[test]
fn ask_user_answer_reaches_next_request() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // ANSWER_ZED originates ONLY from the answers vec — it never appears in the
    // scripted assistant messages or the user message below, so any occurrence
    // in a later request proves the answer was fed back to the provider.
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "ask_user", json!({ "question": "your name?" })),
        assistant_text("thanks"),
    ]);
    let out = run_turn(
        ws.root(),
        p,
        rec,
        "ask",
        PermPolicy::GrantOnce,
        vec!["ANSWER_ZED".into()],
    );

    let fed_back = out.requests.iter().any(|r| dump(&r.messages).contains("ANSWER_ZED"));
    assert!(
        fed_back,
        "ask_user answer must be fed back to the provider on a later request; \
         requests = {}",
        out.requests
            .iter()
            .map(|r| dump(&r.messages))
            .collect::<Vec<_>>()
            .join(" || ")
    );
}

#[test]
fn memory_tool_writes_kv_file() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call(
            "c1",
            "memory",
            json!({ "action": "set", "key": "k1", "value": "V1_PERSISTED" }),
        ),
        assistant_text("saved"),
    ]);
    let out = run_turn(ws.root(), p, rec, "remember", PermPolicy::GrantOnce, vec![]);

    assert!(
        ws.exists("memory/k1"),
        "memory set must persist the bare file memory/<key>"
    );
    assert!(
        ws.read("memory/k1").contains("V1_PERSISTED"),
        "persisted memory file must contain the set value"
    );
    let _ = out;
}
