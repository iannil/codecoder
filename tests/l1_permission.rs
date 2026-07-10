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

// AlwaysThisProject persists the granted key to `<root>/codecoder.json` (ADR
// 0005) so the grant survives a fresh process; the permission gate consults the
// loaded project allowlist alongside the in-memory session set. This test pins
// that persistence: the file must exist and contain the granted key.
#[test]
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

// The other half of ADR 0005: a persisted project grant is LOADED on a fresh
// agent and suppresses the prompt. Seed codecoder.json as a prior session would,
// then run under Deny — if the gate prompted, Deny would block the write and no
// file would appear. It must not prompt.
#[test]
fn persisted_project_grant_is_loaded_and_suppresses_prompt() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("codecoder.json", "{\"allowlist\":[\"write_file\"]}");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "write_file", json!({"path": "a.txt", "content": "1"})),
        assistant_text("done"),
    ]);
    let out = run_turn(ws.root(), p, rec, "write", PermPolicy::Deny, vec![]);
    assert!(
        out.permission_keys().is_empty(),
        "a loaded project grant must suppress the prompt; got {:?}",
        out.permission_keys()
    );
    assert!(
        ws.exists("a.txt"),
        "the write must proceed under the loaded project grant, even under a Deny policy"
    );
}

// Ceiling rule (ADR 0022): a Shell-environment capability grant may never reach
// project scope. Even under GrantProject, a `run_capability:<name>@shell` key
// must be capped to the session set and never written to codecoder.json.
#[test]
fn shell_capability_project_grant_capped_not_persisted() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call(
            "c1",
            "generate_capability",
            json!({
                "name": "capx", "description": "d", "environment": "shell",
                "lifecycle": "one_shot", "script": "echo hi"
            }),
        ),
        assistant_tool_call("c2", "run_capability", json!({"name": "capx"})),
        assistant_text("done"),
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![Step::Msg("make".into()), Step::Reload, Step::Msg("run".into())],
        PermPolicy::GrantProject,
    );
    // The @shell run_capability prompt actually fired (grant path exercised)...
    assert!(
        out.permission_keys()
            .iter()
            .any(|k| k.contains("run_capability") && k.contains("@shell")),
        "expected the @shell run_capability prompt; got {:?}",
        out.permission_keys()
    );
    // ...yet no @shell key may be persisted to the project allowlist.
    if ws.exists("codecoder.json") {
        assert!(
            !ws.read("codecoder.json").contains("@shell"),
            "ceiling rule violated: an @shell key reached codecoder.json"
        );
    }
}
