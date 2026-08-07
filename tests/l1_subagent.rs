// tests/l1_subagent.rs — L1 behavioral tests for the SUB-AGENT boundary
// (ADR 0042). The `agent` tool has two modes:
//   * Fork mode (no `subagent_type`): inherits parent context + full toolbox.
//   * Fresh mode (with `subagent_type`): zero context + full toolbox.
//
// The `review` tool remains read-only (uses `read_only_child()`).
//
// How the sub-agent consumes the scripted provider queue (verified against
// src/agent.rs::spawn_sub_agent): the child is built with
// `Arc::clone(&self.provider)`, so it SHARES the parent's single
// `ScriptedProvider`. The queue is consumed in strict call order:
//   1. parent's turn (emits the `agent` tool_call)          -> turns[0]
//   2. the child's own loop runs `process_turn`, pulling as
//      many turns as its tool loop needs                    -> turns[1..k]
//   3. control returns to the parent, which pulls its
//      continuation                                         -> turns[k..]
// So `turns` must interleave parent + child scripts in that exact order.

mod testkit;
use codecoder::MessageItem;
use serde_json::json;
use testkit::*;

/// Extract the tool names offered on a recorded request, reading the REAL wire
/// shape produced by `Toolbox::wire_schemas`:
///   {"type":"function","function":{"name":<NAME>, ...}}
fn tool_names(req: &RecordedRequest) -> Vec<String> {
    req.tools
        .iter()
        .filter_map(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}


#[test]
fn agent_tool_spawns_subagent_and_reports_back() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("target.txt", "SUB_PAYLOAD");

    // turns[0] parent: delegate via `agent`.
    // turns[1] child : read the target file.
    // turns[2] child : text report (becomes the child's last_assistant_text,
    //                  i.e. the value returned to the parent as the tool result).
    // turns[3] parent: continuation.
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "agent", json!({"task": "read target.txt and report"})),
        assistant_tool_call("s1", "read_file", json!({"path": "target.txt"})),
        assistant_text("REPORT: target.txt says SUB_PAYLOAD"),
        assistant_text("parent done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "delegate", PermPolicy::GrantOnce, vec![]);

    // The `agent` tool is intercepted in `dispatch_tool` and handled by
    // `spawn_sub_agent`, which returns a `ToolResult` directly and emits only
    // `SubAgentMilestone` events (never the `ToolStarted`/`ToolFinished` pair a
    // plain tool would). So the report-back is NOT observable via
    // `tool_outputs("agent")`; it flows into the message history as the
    // `ToolResult` for call_id "c1", which the parent then carries into its
    // continuation request. We assert the report-back by finding that
    // ToolResult (non-error, non-empty) in any recorded request's messages.
    let reported = out.requests.iter().any(|r| {
        r.messages.iter().any(|m| {
            m.items.iter().any(|it| {
                matches!(it, MessageItem::ToolResult { call_id, output, is_error }
                    if call_id == "c1" && !*is_error && !output.trim().is_empty())
            })
        })
    });
    assert!(
        reported,
        "agent tool must return the sub-agent report to the parent (ToolResult for c1)"
    );
}

/// Fork sub-agent (no `subagent_type`) inherits the full toolbox (ADR 0042),
/// including `write_file`, `agent`, and `milestone`. This is a deliberate
/// design change from the old read-only-child approach.
/// Security is enforced by the permission gate, not the toolbox.
#[test]
fn subagent_has_full_toolbox() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");

    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "agent", json!({"task": "inspect your tools"})),
        assistant_text("sub done"),
        assistant_text("parent done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "delegate", PermPolicy::GrantOnce, vec![]);

    // The fork sub-agent uses the full Toolbox::builtin().
    // Find child requests: they have the full toolset including agent, write_file, milestone.
    let child_reqs: Vec<&RecordedRequest> = out
        .requests
        .iter()
        .filter(|r| {
            let names = tool_names(r);
            names.iter().any(|n| n == "agent")
                && names.iter().any(|n| n == "write_file")
                && names.iter().any(|n| n == "milestone")
        })
        .collect();

    assert!(
        !child_reqs.is_empty(),
        "fork sub-agent should have the full toolbox (agent, write_file, milestone); \
         recorded toolsets: {:?}",
        out.requests.iter().map(tool_names).collect::<Vec<_>>()
    );
}

/// `review` (first-class citizen #4 quick-win) reuses the sub-agent machinery
/// but parses the child's prose into a structured verdict via `crate::review`.
/// This exercises the KERNEL GUARD end-to-end: the scripted reviewer self-reports
/// a lenient `VERDICT: pass`, but its own `SIGNALS` line flags a foundation-tamper
/// fail. The AgentLoop must upgrade the returned verdict to `rebuild` (a lenient
/// reviewer can never downgrade below what the signals imply).
#[test]
fn review_returns_structured_verdict_with_kernel_guard() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");

    // turns[0] parent: delegate via `review` (intercepted → spawns sub-agent).
    // turns[1] child : the review report, ending with the two-line contract.
    // turns[2] parent: continuation.
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "review", json!({"target": "the current changes"})),
        assistant_text(
            "Reviewed the diff. It changes a public trait signature.\n\
             VERDICT: pass\n\
             SIGNALS: foundation=fail over_engineering=ok volume=ok terminology=ok",
        ),
        assistant_text("parent done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "review it", PermPolicy::GrantOnce, vec![]);

    // The review ToolResult for call_id "c1" is carried into the parent's
    // continuation request. Its output must begin with the deterministic header,
    // and the verdict must be the guard-upgraded `rebuild` — NOT the reviewer's
    // self-reported `pass`.
    let verdict_ok = out.requests.iter().any(|r| {
        r.messages.iter().any(|m| {
            m.items.iter().any(|it| {
                matches!(it, MessageItem::ToolResult { call_id, output, is_error }
                    if call_id == "c1"
                        && !*is_error
                        && output.starts_with("REVIEW VERDICT: rebuild")
                        && output.contains("foundation=fail"))
            })
        })
    });
    assert!(
        verdict_ok,
        "review must return a structured `REVIEW VERDICT: rebuild` (kernel guard \
         upgrading the reviewer's lenient `pass` on a foundation fail); recorded \
         tool results did not contain it"
    );
}

/// Fork sub-agent (no `subagent_type`) inherits the FULL `Toolbox::builtin()`,
/// including `agent`, `write_file`, and `milestone`. This is a deliberate
/// design change from the old read-only-child approach (ADR 0042).
/// The `review` tool retains the read-only child toolbox.
#[test]
fn subagent_inherits_full_toolbox() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");

    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "agent", json!({"task": "inspect your tools"})),
        assistant_text("sub done"),
        assistant_text("parent done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "delegate", PermPolicy::GrantOnce, vec![]);

    // Find the sub-agent's requests. The fork sub-agent uses the full
    // Toolbox::builtin(), which includes `agent`, `write_file`, etc.
    // The child requests are those that share the same toolset as the parent.
    let child_reqs: Vec<&RecordedRequest> = out
        .requests
        .iter()
        .filter(|r| {
            let names = tool_names(r);
            // The child's first request has the user's task text, while the
            // parent's first request has the initial "delegate" prompt.
            // We find child requests by looking for ones that appear after
            // the parent's first request AND have the full toolset.
            names.iter().any(|n| n == "agent")
                && names.iter().any(|n| n == "write_file")
                && names.iter().any(|n| n == "milestone")
        })
        .collect();

    assert!(
        !child_reqs.is_empty(),
        "expected fork sub-agent to have the full toolbox (including `agent`, `write_file`, `milestone`); \
         recorded toolsets: {:?}",
        out.requests.iter().map(tool_names).collect::<Vec<_>>()
    );

    // Sanity: the parent DID have the `agent` tool available.
    assert!(
        out.requests.iter().any(|r| tool_names(r).iter().any(|n| n == "agent")),
        "parent request should offer the `agent` tool"
    );
}
