// L1 — Background Agent (ADR 0026). Black-box: drive the headless runner with a
// ScriptedProvider and assert on the returned BgOutcome + disk side effects.
mod testkit;
use testkit::*;
use serde_json::json;
use std::sync::Arc;

#[test]
fn background_runs_task_and_reports_tool_use() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("data.txt", "PAYLOAD_BG");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "read_file", json!({"path": "data.txt"})),
        assistant_text("I read PAYLOAD_BG"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "read data.txt".into(),
    ).expect("bg run");
    assert!(out.tool_calls.iter().any(|t| t == "read_file"), "read_file should run: {:?}", out.tool_calls);
    // A session was persisted (ADR 0004).
    assert!(ws.root().join("sessions").read_dir().map(|mut d| d.next().is_some()).unwrap_or(false),
        "a session file must be written");
}

#[test]
fn background_denies_unauthorized_write() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "write_file", json!({"path": "hacked.txt", "content": "x"})),
        assistant_text("tried to write"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "write a file".into(),
    ).expect("bg run");
    // Core safety property: unauthorized Ask-tool is denied, no file, turn still completes.
    assert!(!ws.exists("hacked.txt"), "unauthorized write must be denied");
    assert!(out.denied.iter().any(|d| d.contains("write_file")), "denial must be recorded: {:?}", out.denied);
}

#[test]
fn background_allows_preauthorized_write() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // Pre-authorize write_file in the project allowlist (as a prior session would).
    ws.write("codecoder.json", "{\"allowlist\":[\"write_file\"]}");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "write_file", json!({"path": "ok.txt", "content": "WROTE_BG"})),
        assistant_text("wrote it"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "write a file".into(),
    ).expect("bg run");
    assert!(ws.exists("ok.txt"), "pre-authorized write must succeed");
    assert_eq!(ws.read("ok.txt"), "WROTE_BG");
    assert!(!out.denied.iter().any(|d| d.contains("write_file")), "authorized write must not be denied");
}

// Hardening (whole-branch review): lock in the security property for the exec
// tool at a path currently covered only structurally. Mirrors
// `background_denies_unauthorized_write` but for `run_command`.
#[test]
fn background_denies_unauthorized_run_command() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // No codecoder.json => run_command (Permission::Ask) is not pre-authorized.
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "run_command", json!({"cmd": "touch ran.flag"})),
        assistant_text("tried to run a command"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "run a command".into(),
    ).expect("bg run");
    // (a) The command must not have run — no side-effect file.
    assert!(!ws.exists("ran.flag"), "unauthorized run_command must not execute");
    // (b) The denial must be observable.
    assert!(out.denied.iter().any(|d| d.contains("run_command")),
        "run_command denial must be recorded: {:?}", out.denied);
    // (c) Implicit: `.expect("bg run")` above proves the run returned Ok (no hang).
}

// Hardening (whole-branch review): a sub-agent spawned FROM a headless parent is
// still read-only — the depth-1 child's toolbox has no write_file at all, so the
// attempt errors as an unknown tool and never touches disk. Mirrors
// `subagent_cannot_write_files` (l1_subagent.rs) but drives it through the
// headless runner. The sub-agent SHARES the parent's ScriptedProvider queue
// (verified against src/agent.rs::spawn_sub_agent), so scripts interleave:
//   turns[0] parent: delegate via `agent`
//   turns[1] child : attempt write_file (refused — absent from read_only_child)
//   turns[2] child : text report (returned to the parent as the tool result)
//   turns[3] parent: closing text
#[test]
fn background_subagent_from_headless_cannot_write() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "agent", json!({"task": "attempt a write"})),
        assistant_tool_call("s1", "write_file", json!({"path": "sub_hacked.txt", "content": "X"})),
        assistant_text("sub done"),
        assistant_text("parent done"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "delegate a write".into(),
    ).expect("bg run");
    // The sub-agent spawned successfully. The `agent` tool is intercepted in
    // dispatch_tool and emits SubAgentMilestone events (never the ToolStarted
    // pair a plain tool emits), so it is observable in `out.events`, not
    // `out.tool_calls`. Requiring "started" proves the child actually ran — so
    // the disk assertion below is not vacuous (a never-spawned child would also
    // leave the file absent).
    assert!(out.events.iter().any(|e| e.contains("sub-agent") && e.contains("started")),
        "parent should have spawned a sub-agent (SubAgentMilestone started): {:?}", out.events);
    // Core property: a read-only sub-agent spawned from a headless parent cannot
    // write. write_file is not in Toolbox::read_only_child(), so no file lands.
    assert!(!ws.exists("sub_hacked.txt"),
        "read-only sub-agent from a headless parent must not be able to write files");
}
