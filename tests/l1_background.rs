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
    // Trust gating (ADR 0028) refuses to load codecoder.json for an untrusted
    // headless run — otherwise a cloned repo's malicious pre-authorization would
    // execute. A legitimate background operator opts in via CODECODER_DEFAULT_TRUST.
    unsafe { std::env::set_var("CODECODER_DEFAULT_TRUST", "always") };
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "write_file", json!({"path": "ok.txt", "content": "WROTE_BG"})),
        assistant_text("wrote it"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "write a file".into(),
    ).expect("bg run");
    unsafe { std::env::remove_var("CODECODER_DEFAULT_TRUST") };
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

// Hardening (whole-branch review): a fork sub-agent spawned FROM a headless parent
// inherits the full toolbox (ADR 0042), so write_file IS available. However, in
// headless mode without a pre-authorizing allowlist, the permission gate denies
// the Ask-keyed tool, so the file never lands on disk. The sub-agent SHARES the
// parent's ScriptedProvider queue, so scripts interleave:
//   turns[0] parent: delegate via `agent`
//   turns[1] child : attempt write_file (denied by permission gate, not toolbox)
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
    // Fork mode emits "forked" (not "started") as the initial milestone.
    assert!(out.events.iter().any(|e| e.contains("sub-agent") && e.contains("forked")),
        "parent should have spawned a fork sub-agent (SubAgentMilestone forked): {:?}", out.events);
    // Core property: a fork sub-agent from a headless parent without a pre-authorizing
    // allowlist cannot write. write_file is Ask-keyed, so the permission gate denies
    // it in headless mode — no file lands on disk.
    assert!(!ws.exists("sub_hacked.txt"),
        "fork sub-agent from a headless parent must not be able to write files without allowlist");
}
