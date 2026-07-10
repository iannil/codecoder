// L1 — KERNEL / turn loop (behavioral coverage §5.1).
//
// Black-box: everything here goes through the public `codecoder` API and the
// shared `testkit` harness. We drive the REAL `AgentLoop` with a deterministic
// `ScriptedProvider` and assert on the events it emits and the requests it makes
// back to the provider.
mod testkit;
use testkit::*;

use codecoder::{AgentEvent, Message, MessageItem};
use serde_json::json;

/// Concatenate all human-readable text carried by a slice of messages: assistant
/// `Text`, tool-call args, and `ToolResult` outputs. Matching on the public
/// `MessageItem` variants is more robust than a `{:?}` Debug string and stays
/// pure black-box (public API only).
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
fn turn_completes_and_streams() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "I am CodeCoder-under-test MARKER_XYZ.");
    let (p, rec) = ScriptedProvider::new(vec![assistant_text("hello world")]);
    let out = run_turn(ws.root(), p, rec, "hi", PermPolicy::GrantOnce, vec![]);
    assert!(
        matches!(out.events.last(), Some(AgentEvent::TurnComplete)),
        "turn must end with TurnComplete; got {:?}",
        out.events.last().map(std::mem::discriminant)
    );
    assert!(
        out.stream_text().contains("hello world"),
        "streamed text must surface the assistant reply; got {:?}",
        out.stream_text()
    );
}

#[test]
fn system_prompt_injects_agents_md() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "MARKER_XYZ identity line.");
    let (p, rec) = ScriptedProvider::new(vec![assistant_text("ok")]);
    let out = run_turn(ws.root(), p, rec, "hi", PermPolicy::GrantOnce, vec![]);
    assert!(
        !out.requests.is_empty(),
        "provider must be called at least once"
    );
    // AGENTS.md content must reach the provider as system context. Extract text
    // from the public MessageItem variants rather than relying on Debug.
    let joined = dump(&out.requests[0].messages);
    assert!(
        joined.contains("MARKER_XYZ"),
        "AGENTS.md identity not injected into the system prompt; request text was:\n{joined}"
    );
}

#[test]
fn multi_iteration_tool_loop_feeds_result_back() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("data.txt", "PAYLOAD_42");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "read_file", json!({"path": "data.txt"})),
        assistant_text("I read it."),
    ]);
    let out = run_turn(
        ws.root(),
        p,
        rec,
        "read data.txt",
        PermPolicy::GrantOnce,
        vec![],
    );
    // A second provider call must happen, carrying the tool result of the first
    // read_file call back into the conversation.
    assert!(
        out.requests.len() >= 2,
        "expected a second provider call after the tool result; got {} request(s)",
        out.requests.len()
    );
    let second = dump(&out.requests[1].messages);
    assert!(
        second.contains("PAYLOAD_42"),
        "tool result (file contents) not fed back into the next request; second request text was:\n{second}"
    );
    // And the tool actually ran successfully (belt-and-braces on the event side).
    let reads = out.tool_outputs("read_file");
    assert!(
        reads.iter().any(|(is_err, o)| !is_err && o.contains("PAYLOAD_42")),
        "read_file did not emit a successful ToolFinished with the payload; got {reads:?}"
    );
}

#[test]
fn tool_loop_caps_at_max_iterations() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("data.txt", "Y");
    // Script far more tool calls than MAX_TOOL_ITERATIONS (12 in src/agent.rs);
    // the loop must stop on its own rather than servicing all 50.
    let turns: Vec<_> = (0..50)
        .map(|i| assistant_tool_call(&format!("c{i}"), "read_file", json!({"path": "data.txt"})))
        .collect();
    let (p, rec) = ScriptedProvider::new(turns);
    let out = run_turn(ws.root(), p, rec, "loop", PermPolicy::GrantOnce, vec![]);
    // MAX_TOOL_ITERATIONS = 12, so at most 12 provider calls. Threshold = cap + 2.
    assert!(
        out.requests.len() < 14,
        "tool loop did not cap at MAX_TOOL_ITERATIONS; got {} provider calls",
        out.requests.len()
    );
    // The guard trips the loop; the turn still terminates cleanly.
    assert!(
        matches!(out.events.last(), Some(AgentEvent::TurnComplete)),
        "capped turn must still end with TurnComplete; got {:?}",
        out.events.last().map(std::mem::discriminant)
    );
}

/// Cooperative cancellation: script a long `run_command` (`sleep 5`), trip the
/// agent's real cancel token the instant the tool starts, and require the turn to
/// return promptly (< 3s) without the long command reporting a successful finish.
///
/// Cooperative cancellation of an in-flight `run_command` (ADR 0016).
/// `run_command` spawns its child and polls the cancel token, killing the child
/// when the turn is cancelled; the tool loop then ends at its next top-of-loop
/// cancel check. A `sleep 5` must therefore be interrupted well within the 3s
/// cooperative-cancellation budget rather than blocking the agent thread.
///
/// NOTE: this exercises the token→tool path only. Delivering `Cancel` from the
/// TUI to the token mid-turn is a separate, still-unwired path (the run loop
/// blocks on `process_turn`); this test flips the shared token directly, as a
/// correctly-wired front end would.
#[test]
fn cancel_interrupts_long_running_command() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "run_command",
        json!({"cmd": "sleep 5"}),
    )]);
    let out = run_turn_with_cancel(
        ws.root(),
        p,
        rec,
        "run a slow command",
        PermPolicy::GrantOnce,
    );

    // The cooperative-cancellation contract: the turn returns promptly once
    // cancelled, well before the 5s the command would otherwise take.
    assert!(
        out.elapsed < std::time::Duration::from_secs(3),
        "cancellation did not return the turn promptly; took {:?}",
        out.elapsed
    );
    // And the long command must NOT have reported a successful finish.
    let finished = out.tool_outputs("run_command");
    assert!(
        !finished.iter().any(|(is_err, _)| !is_err),
        "expected no successful ToolFinished for the cancelled command; got {finished:?}"
    );
}
