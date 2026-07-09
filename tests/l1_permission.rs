// L1 — Permission scope semantics (§5.5, ADR 0005).
//
// Real semantics confirmed against src/ (do not edit src/):
//
//   * write_file reports `Permission::Ask { key: "write_file" }` — keyed by TOOL
//     ONLY, never by path (src/tool/builtin.rs:151). So two write_file calls to
//     different paths share the exact same permission key "write_file".
//
//   * The permission gate (src/agent.rs:381-397) grants BOTH AlwaysThisSession
//     and AlwaysThisProject into the same in-memory `SessionAllowlist`. Once a
//     key is in the allowlist, `allows()` short-circuits the prompt. Hence for a
//     stable key: GrantSession/GrantProject → 1 prompt for 2 calls; GrantOnce →
//     never grants into the allowlist → 2 prompts.
//
//   * AlwaysThisProject has NO disk-persistence path in the codebase: no code
//     writes `codecoder.json`. permission.rs only *documents* a persisted
//     project allowlist "keyed identically" — but nothing implements it. See the
//     REVEALS note on grant_project_persists_allowlist_to_disk below.

mod testkit;
use serde_json::json;
use testkit::*;

/// Two write_file calls (different paths, same "write_file" key) + closing text.
fn two_writes() -> Vec<codecoder::Message> {
    vec![
        assistant_tool_call("c1", "write_file", json!({"path": "a.txt", "content": "1"})),
        assistant_tool_call("c2", "write_file", json!({"path": "b.txt", "content": "2"})),
        assistant_text("done"),
    ]
}

#[test]
fn grant_session_suppresses_second_prompt() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(two_writes());
    let out = run_turn(ws.root(), p, rec, "writes", PermPolicy::GrantSession, vec![]);
    // "write_file" is a stable key across both calls; AlwaysThisSession grants it
    // into the session allowlist on the first prompt, suppressing the second.
    assert_eq!(
        out.permission_keys().len(),
        1,
        "AlwaysThisSession must suppress the 2nd write_file prompt (key is tool-only)"
    );
    assert_eq!(out.permission_keys(), vec!["write_file".to_string()]);
    assert!(
        ws.exists("a.txt") && ws.exists("b.txt"),
        "both writes must have executed"
    );
}

#[test]
fn grant_once_prompts_each_time() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(two_writes());
    let out = run_turn(ws.root(), p, rec, "writes", PermPolicy::GrantOnce, vec![]);
    // Once never enters the allowlist, so every call re-prompts.
    assert_eq!(
        out.permission_keys().len(),
        2,
        "Once must prompt for each write_file call"
    );
    assert_eq!(
        out.permission_keys(),
        vec!["write_file".to_string(), "write_file".to_string()]
    );
    assert!(
        ws.exists("a.txt") && ws.exists("b.txt"),
        "both writes must have executed"
    );
}

// REVEALS: AlwaysThisProject does NOT persist an allowlist to disk. The
// permission gate (src/agent.rs:381-397) treats AlwaysThisProject exactly like
// AlwaysThisSession — it grants into the in-memory `SessionAllowlist` and
// nothing more. No code path in the codebase writes `codecoder.json` (or any
// other file) for a project-scoped grant, so a fresh process would re-prompt.
// permission.rs only *documents* a persisted project allowlist "keyed
// identically"; the implementation is absent. The assertion below encodes the
// TRUE ADR-0005 intent (project scope must survive to disk) and is kept intact;
// the test is ignored until the persistence gap is closed.
#[test]
#[ignore = "REVEALS: AlwaysThisProject never persists to disk — no codecoder.json is written (ADR 0005 project-scope persistence unimplemented)"]
fn grant_project_persists_allowlist_to_disk() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(two_writes());
    let out = run_turn(ws.root(), p, rec, "writes", PermPolicy::GrantProject, vec![]);
    assert!(
        ws.exists("codecoder.json"),
        "AlwaysThisProject must persist a project allowlist to codecoder.json"
    );
    assert!(
        ws.read("codecoder.json").contains("write_file"),
        "persisted allowlist must contain the granted write_file key"
    );
    let _ = out;
}
