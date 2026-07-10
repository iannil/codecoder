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
