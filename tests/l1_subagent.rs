// tests/l1_subagent.rs — L1 behavioral tests for the SUB-AGENT boundary
// (ADR 0019, §5.4). The `agent` tool spawns a depth-1 read-only sub-agent that
// reports back to the parent. Two invariants are exercised:
//   * read-only enforcement — the child holds only the 9 `Permission::None`
//     tools; it cannot write files;
//   * depth-lock 1 — the child's toolset excludes `agent`, so it cannot spawn
//     another sub-agent.
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

#[test]
fn subagent_cannot_write_files() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");

    // The child shares the queue. It is scripted to attempt `write_file`, which
    // is NOT in `Toolbox::read_only_child()`. `dispatch_tool` returns an error
    // ToolResult ("unknown tool: write_file") and never touches disk. The child
    // then emits its report; the parent continues.
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "agent", json!({"task": "attempt a write"})),
        assistant_tool_call("s1", "write_file", json!({"path": "hacked.txt", "content": "X"})),
        assistant_text("sub done"),
        assistant_text("parent done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "delegate-write", PermPolicy::GrantOnce, vec![]);

    // Read-only enforcement: the write must not have landed on disk.
    assert!(
        !ws.exists("hacked.txt"),
        "read-only sub-agent must not be able to write files"
    );
    let _ = out;
}

#[test]
fn subagent_toolset_excludes_agent_tool() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");

    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "agent", json!({"task": "inspect your tools"})),
        assistant_text("sub done"),
        assistant_text("parent done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "delegate", PermPolicy::GrantOnce, vec![]);

    // Discrimination: parent vs sub-agent requests are told apart by their
    // offered toolset. The parent runs the full `Toolbox::builtin()`, whose wire
    // schemas INCLUDE `agent`; a depth-1 child runs `Toolbox::read_only_child()`,
    // whose schemas never include `agent`. So the sub-agent request(s) are
    // exactly those whose `tool_names` lacks `agent`. We additionally require the
    // set to be non-empty and to contain a known read-only tool (`read_file`) so
    // we are asserting against a real child toolset, not an empty/degenerate one.
    let child_reqs: Vec<&RecordedRequest> = out
        .requests
        .iter()
        .filter(|r| {
            let names = tool_names(r);
            !names.is_empty()
                && !names.iter().any(|n| n == "agent")
                && names.iter().any(|n| n == "read_file")
        })
        .collect();

    assert!(
        !child_reqs.is_empty(),
        "expected at least one request issued on behalf of the sub-agent; \
         recorded toolsets: {:?}",
        out.requests.iter().map(tool_names).collect::<Vec<_>>()
    );

    for r in &child_reqs {
        let names = tool_names(r);
        // Depth-lock 1: the child must not be offered `agent`.
        assert!(
            !names.iter().any(|n| n == "agent"),
            "sub-agent must not be offered the `agent` tool (depth-lock 1); got {:?}",
            names
        );
        // Read-only: the child must not be offered write tools either.
        assert!(
            !names.iter().any(|n| n == "write_file"),
            "sub-agent must not be offered the `write_file` tool (read-only); got {:?}",
            names
        );
    }

    // Sanity: the parent DID have the `agent` tool available (else the discrimination
    // above would be vacuous — every request would trivially lack `agent`).
    assert!(
        out.requests.iter().any(|r| tool_names(r).iter().any(|n| n == "agent")),
        "parent request should offer the `agent` tool; recorded toolsets: {:?}",
        out.requests.iter().map(tool_names).collect::<Vec<_>>()
    );
}
