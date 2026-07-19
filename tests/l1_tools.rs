// L1 — FILE / SEARCH / DEV / EXECUTION tools + permission gating on writes (§5.2).
//
// Black-box: every tool is driven through the public `codecoder` API via the
// shared `testkit` harness and a deterministic `ScriptedProvider`. Arg keys below
// are calibrated against the REAL tool schemas (src/tool/{builtin,dev,search}.rs):
//   read_file/list_directory/write_file/edit_file/diff  -> "path"
//   write_file                                          -> "path" + "content"
//   edit_file                                           -> "path" + "old" + "new"
//   grep (text)                                         -> "pattern" (+ "path")
//   grep (AST)                                          -> "ast_query" (+ "lang", default rust)
//   run_command                                         -> "cmd"; perm key "run_command:<head>"
mod testkit;
use testkit::*;

use codecoder::{Message, MessageItem};
use serde_json::json;

/// Concatenate human-readable text from a slice of messages (public MessageItem
/// variants only). Small duplication of l1_kernel.rs's helper — fine across files.
#[allow(dead_code)]
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

fn seed(ws: &Workspace) {
    ws.write("AGENTS.md", "x");
}

// ---------------------------------------------------------------------------
// FILE tools: read / list / write (+Deny) / edit
// ---------------------------------------------------------------------------

#[test]
fn read_file_returns_content() {
    let ws = Workspace::new();
    seed(&ws);
    ws.write("a.txt", "CONTENT_A");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "read_file",
        json!({"path": "a.txt"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "read", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.tool_outputs("read_file")
            .iter()
            .any(|(err, o)| !err && o.contains("CONTENT_A")),
        "read_file must return file contents; got {:?}",
        out.tool_outputs("read_file")
    );
}

#[test]
fn list_directory_lists_entries() {
    let ws = Workspace::new();
    seed(&ws);
    ws.write("sub/one.txt", "1");
    ws.write("sub/two.txt", "2");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "list_directory",
        json!({"path": "sub"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "ls", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.tool_outputs("list_directory")
            .iter()
            .any(|(err, o)| !err && o.contains("one.txt") && o.contains("two.txt")),
        "list_directory must enumerate directory entries; got {:?}",
        out.tool_outputs("list_directory")
    );
}

/// The `milestone` tool (Work Graph, first-class citizen #2) drives a durable,
/// dependency-ordered graph through the real AgentLoop and lands on disk. This
/// exercises the scheduling invariant end-to-end: #2 stays behind #1 until #1 is
/// done, then `next` yields #2 — and the state is persisted to `workgraph.json`.
#[test]
fn milestone_work_graph_schedules_and_persists() {
    let ws = Workspace::new();
    seed(&ws);
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "milestone", json!({"action": "add", "title": "data model"})),
        assistant_tool_call("c2", "milestone", json!({"action": "add", "title": "logic", "deps": [1]})),
        assistant_tool_call("c3", "milestone", json!({"action": "next"})),
        assistant_tool_call("c4", "milestone", json!({"action": "done", "id": 1, "verdict": "pass"})),
        assistant_tool_call("c5", "milestone", json!({"action": "next"})),
        assistant_text("done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "plan it", PermPolicy::GrantOnce, vec![]);

    // The two `next` calls bracket the `done`: first ready is #1, then #2.
    let nexts = out.tool_outputs("milestone");
    assert!(
        nexts.iter().any(|(err, o)| !err && o.contains("#1 data model")),
        "first `next` should surface #1; outputs: {nexts:?}"
    );
    assert!(
        nexts.iter().any(|(err, o)| !err && o.contains("#2 logic")),
        "after #1 done, `next` should surface #2; outputs: {nexts:?}"
    );

    // State persisted to disk: #1 done (records the verdict), #2 pending.
    let raw = ws.read("workgraph.json");
    let wg: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let nodes = wg["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["status"], "done");
    assert_eq!(nodes[0]["verdict"], "pass");
    assert_eq!(nodes[1]["status"], "pending");
    assert_eq!(nodes[1]["deps"], json!([1]));
}

#[test]
fn write_file_asks_permission_then_lands_on_disk() {
    let ws = Workspace::new();
    seed(&ws);
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "write_file",
        json!({"path": "out.txt", "content": "WROTE_IT"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "write", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.permission_keys().iter().any(|k| k.contains("write_file")),
        "write_file must emit a permission request; keys were {:?}",
        out.permission_keys()
    );
    assert_eq!(ws.read("out.txt"), "WROTE_IT");
}

#[test]
fn write_file_denied_leaves_no_file() {
    let ws = Workspace::new();
    seed(&ws);
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "write_file",
        json!({"path": "no.txt", "content": "X"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "write", PermPolicy::Deny, vec![]);
    // Positive controls — prove the GUARD fired, not that the tool was simply
    // never reached (a hang or mis-dispatch would also leave the file absent):
    //   (a) the turn ran to completion (did not hit the 5s driver timeout),
    //   (b) the Ask gate actually asked before denying (a write_file perm key),
    // and only THEN (c) the absence assertion is meaningful.
    assert!(
        out.completed,
        "turn must run to completion, not hang; a timed-out turn also leaves the file absent"
    );
    assert!(
        out.permission_keys().iter().any(|k| k.contains("write_file")),
        "denied write must still pass through the Ask gate (write_file perm key); keys were {:?}",
        out.permission_keys()
    );
    assert!(
        !ws.exists("no.txt"),
        "denied write must not create the file"
    );
    // NOTE: on Deny the agent returns the denial `ToolResult` and short-circuits
    // BEFORE emitting the `ToolStarted`/`ToolFinished` pair (src/agent.rs), so
    // `out.tool_outputs("write_file")` is intentionally empty — we do not assert
    // on it here (see brief's optional clause).
}

#[test]
fn edit_file_asks_permission_then_replaces_text() {
    let ws = Workspace::new();
    seed(&ws);
    ws.write("e.txt", "hello OLD world");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "edit_file",
        json!({"path": "e.txt", "old": "OLD", "new": "NEW"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "edit", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.permission_keys().iter().any(|k| k.contains("edit_file")),
        "edit_file must emit a permission request; keys were {:?}",
        out.permission_keys()
    );
    assert_eq!(ws.read("e.txt"), "hello NEW world");
}

// ---------------------------------------------------------------------------
// SEARCH tools: glob / grep (text) / grep (AST)
// ---------------------------------------------------------------------------

#[test]
fn glob_matches_files_by_pattern() {
    let ws = Workspace::new();
    seed(&ws);
    ws.write("src/keep.rs", "fn a() {}");
    ws.write("src/other.txt", "nope");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "glob",
        json!({"pattern": "**/*.rs"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "glob", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.tool_outputs("glob")
            .iter()
            .any(|(err, o)| !err && o.contains("keep.rs") && !o.contains("other.txt")),
        "glob must match by pattern and exclude non-matches; got {:?}",
        out.tool_outputs("glob")
    );
}

#[test]
fn grep_finds_text_matches() {
    let ws = Workspace::new();
    seed(&ws);
    ws.write("src/x.rs", "fn needle_fn() {}");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "grep",
        json!({"pattern": "needle_fn", "path": "."}),
    )]);
    let out = run_turn(ws.root(), p, rec, "grep", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.tool_outputs("grep")
            .iter()
            .any(|(err, o)| !err && o.contains("needle_fn")),
        "grep (text) must return matching lines; got {:?}",
        out.tool_outputs("grep")
    );
}

#[test]
fn grep_ast_query_matches_function() {
    let ws = Workspace::new();
    seed(&ws);
    ws.write("src/y.rs", "fn target() {}\nfn other() {}");
    // AST-mode grep (tree-sitter). Real schema: `ast_query` string + optional
    // `lang` (defaults to rust). The query captures function-item names.
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "grep",
        json!({"ast_query": "(function_item name: (identifier) @n)", "lang": "rust", "path": "src"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "ast", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.tool_outputs("grep")
            .iter()
            .any(|(err, o)| !err && o.contains("target")),
        "grep (AST) must match the seeded Rust function; got {:?}",
        out.tool_outputs("grep")
    );
}

// ---------------------------------------------------------------------------
// EXECUTION: run_command (class-scoped permission key)
// ---------------------------------------------------------------------------

#[test]
fn run_command_permission_keyed_by_class_and_runs() {
    let ws = Workspace::new();
    seed(&ws);
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "run_command",
        json!({"cmd": "touch ran.flag"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "run", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.permission_keys()
            .iter()
            .any(|k| k.starts_with("run_command")),
        "run_command permission key must be class-scoped (run_command:<class>); keys were {:?}",
        out.permission_keys()
    );
    assert!(
        ws.exists("ran.flag"),
        "granted command should have executed"
    );
}

#[test]
fn run_command_denied_does_not_execute() {
    let ws = Workspace::new();
    seed(&ws);
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "run_command",
        json!({"cmd": "touch denied.flag"}),
    )]);
    let out = run_turn(ws.root(), p, rec, "run", PermPolicy::Deny, vec![]);
    // Positive controls (see write_file_denied_leaves_no_file): prove the turn
    // completed and the Ask gate fired, so absence reflects a real denial rather
    // than a hung/mis-dispatched turn.
    assert!(
        out.completed,
        "turn must run to completion, not hang; a timed-out turn also leaves the file absent"
    );
    assert!(
        out.permission_keys()
            .iter()
            .any(|k| k.starts_with("run_command")),
        "denied run_command must still pass through the Ask gate (run_command perm key); keys were {:?}",
        out.permission_keys()
    );
    assert!(
        !ws.exists("denied.flag"),
        "denied run_command must not execute the command"
    );
}

// ---------------------------------------------------------------------------
// DEV: diff (Permission::None, over a git working tree)
// ---------------------------------------------------------------------------

#[test]
fn diff_shows_working_tree_changes() {
    let ws = Workspace::new();
    seed(&ws);
    // `git diff` (unstaged) compares the working tree against the index, so the
    // baseline must be tracked (staged) first. git_init only inits+configs — it
    // makes no commit — so we stage V1 explicitly, then overwrite with V2.
    ws.write("tracked.txt", "V1\n");
    ws.git_init();
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(ws.root())
        .status()
        .unwrap();
    ws.write("tracked.txt", "V2_CHANGED\n");
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "diff",
        json!({}),
    )]);
    let out = run_turn(ws.root(), p, rec, "diff", PermPolicy::GrantOnce, vec![]);
    assert!(
        out.tool_outputs("diff")
            .iter()
            .any(|(err, o)| !err && o.contains("V2_CHANGED")),
        "diff must surface the working-tree change; got {:?}",
        out.tool_outputs("diff")
    );
}
