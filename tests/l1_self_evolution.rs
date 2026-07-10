// L1 — SELF-EVOLUTION loop (behavioral coverage §5.3).
//
// Black-box: every assertion goes through the public `codecoder` API and the
// shared `testkit` harness. We drive the REAL `AgentLoop` across MULTIPLE steps
// (via `run_steps`) so a skill/prompt/capability authored in an early step is
// visible after a `Reload` and can be activated in a later step.
//
// Real tool schemas/paths (calibrated against src/tool/builtin.rs, src/registry.rs,
// src/capability.rs — NOT the brief's guesses):
//   generate_skill      {name, content}                       -> skills/<name>.md
//   generate_prompt     {name, content}                       -> prompts/<name>.md
//   promote_prompt      {name}                                 -> prompts/<name>.md => skills/<name>.md
//                                                                  (errors + no clobber if skill exists)
//   generate_capability {name, description, environment:"shell",
//                        lifecycle:"one_shot", script}         -> capabilities/<name>/manifest.json (+ run.sh)
//   run_capability      {name}   (Shell: `sh -c <entry>` with cwd = capabilities/<name>)
mod testkit;
use testkit::*;
use serde_json::json;

// generate_skill writes skills/<name>.md; after Reload, use_skill injects the
// FULL body text into a later provider request (ADR 0020).
#[test]
fn generate_skill_writes_file_then_use_skill_injects_body() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![
        // turn 1: author a skill
        assistant_tool_call(
            "c1",
            "generate_skill",
            json!({"name": "greet", "content": "# Greet\nSKILL_BODY_MARKER"}),
        ),
        // turn 2 (after Reload): activate it
        assistant_tool_call("c2", "use_skill", json!({"name": "greet"})),
        assistant_text("used"),
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![
            Step::Msg("make skill".into()),
            Step::Reload,
            Step::Msg("use it".into()),
        ],
        PermPolicy::GrantOnce,
    );
    assert!(
        ws.exists("skills/greet.md"),
        "generate_skill must write skills/greet.md"
    );
    assert!(ws.read("skills/greet.md").contains("SKILL_BODY_MARKER"));
    // use_skill injects full text → it must surface in a later provider request
    // (the tool_result body is fed back to the model).
    let injected = out
        .requests
        .iter()
        .any(|r| format!("{:?}", r.messages).contains("SKILL_BODY_MARKER"));
    assert!(
        injected,
        "use_skill did not inject skill body into context"
    );
}

// generate_prompt lands a draft in prompts/; promote_prompt moves it to skills/
// and removes the draft (ADR 0025). Same-thread, no Reload needed between the two
// tool calls (both operate directly on disk).
#[test]
fn generate_prompt_then_promote_moves_draft_to_skill() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call(
            "c1",
            "generate_prompt",
            json!({"name": "draft1", "content": "DRAFT_BODY"}),
        ),
        assistant_tool_call("c2", "promote_prompt", json!({"name": "draft1"})),
        assistant_text("done"),
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![Step::Msg("draft".into()), Step::Msg("promote".into())],
        PermPolicy::GrantOnce,
    );
    // generate_prompt ran successfully (no error ToolResult).
    assert!(
        out.tool_outputs("generate_prompt")
            .iter()
            .all(|(err, _)| !*err),
        "generate_prompt must not error; got {:?}",
        out.tool_outputs("generate_prompt")
    );
    // After promotion the draft is gone and the skill exists with the draft body.
    assert!(
        !ws.exists("prompts/draft1.md"),
        "promote must remove the draft"
    );
    assert!(
        ws.exists("skills/draft1.md"),
        "promote must create the skill"
    );
    assert!(
        ws.read("skills/draft1.md").contains("DRAFT_BODY"),
        "promote (verbatim) must carry the draft body into the skill"
    );
}

// promote_prompt onto an existing skill name must surface an error ToolResult AND
// leave the pre-existing skill untouched (no clobber) — both are real behavior
// (ADR 0025).
#[test]
fn promote_prompt_name_collision_errors() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("skills/dup.md", "EXISTING");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "generate_prompt", json!({"name": "dup", "content": "D"})),
        assistant_tool_call("c2", "promote_prompt", json!({"name": "dup"})),
        assistant_text("done"),
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![Step::Msg("draft".into()), Step::Msg("promote".into())],
        PermPolicy::GrantOnce,
    );
    assert!(
        out.tool_outputs("promote_prompt")
            .iter()
            .any(|(err, _)| *err),
        "colliding promote must surface an error ToolResult"
    );
    assert_eq!(
        ws.read("skills/dup.md"),
        "EXISTING",
        "collision must not clobber existing skill"
    );
}

// generate_capability writes capabilities/<name>/manifest.json (+ run.sh from
// `script`); after Reload, run_capability(Shell) executes the entry in the
// capability directory (cwd = capabilities/<name>), so the flag lands there.
#[test]
fn generate_capability_writes_manifest_and_runs_shell() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_tool_call(
            "c1",
            "generate_capability",
            json!({
                "name": "echoer",
                "description": "touches a flag file",
                "environment": "shell",
                "lifecycle": "one_shot",
                "script": "touch cap_ran.flag"
            }),
        ),
        assistant_tool_call("c2", "run_capability", json!({"name": "echoer"})),
        assistant_text("done"),
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![
            Step::Msg("make".into()),
            Step::Reload,
            Step::Msg("run".into()),
        ],
        PermPolicy::GrantOnce,
    );
    assert!(
        ws.exists("capabilities/echoer/manifest.json"),
        "generate_capability must write capabilities/echoer/manifest.json"
    );
    // `script` is written to run.sh and entry becomes `sh run.sh`.
    assert!(
        ws.exists("capabilities/echoer/run.sh"),
        "generate_capability(script) must write run.sh"
    );
    // run_capability(Shell) runs with cwd = capabilities/echoer, so `touch
    // cap_ran.flag` creates the flag inside that directory.
    assert!(
        ws.exists("capabilities/echoer/cap_ran.flag"),
        "run_capability(Shell) must execute the script (flag lands in the capability dir)"
    );
    // The permission key must be scoped by name + environment (ADR 0018/0022).
    assert!(
        out.permission_keys()
            .iter()
            .any(|k| k == "run_capability:echoer@shell"),
        "run_capability must gate on run_capability:echoer@shell; got {:?}",
        out.permission_keys()
    );
}

// Cooperative cancellation reaches a long-running shell Capability too (ADR 0016):
// run_capability runs `sh -c <entry>` through the shared run_shell_cancellable
// helper, so flipping the token mid-run kills the child. Pre-seed a slow shell
// capability on disk (run_capability reads the manifest directly — no reload
// needed), then cancel it right after it starts.
#[test]
fn cancel_interrupts_long_running_capability() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write(
        "capabilities/slow/manifest.json",
        r#"{"name":"slow","description":"sleeps","environment":"shell","lifecycle":"one_shot","entry":"sleep 5"}"#,
    );
    let (p, rec) = ScriptedProvider::new(vec![assistant_tool_call(
        "c1",
        "run_capability",
        json!({"name": "slow"}),
    )]);
    let out = run_turn_with_cancel(
        ws.root(),
        p,
        rec,
        "run the slow capability",
        PermPolicy::GrantOnce,
    );

    assert!(
        out.elapsed < std::time::Duration::from_secs(3),
        "cancelling an in-flight shell capability must return promptly; took {:?}",
        out.elapsed
    );
    let finished = out.tool_outputs("run_capability");
    assert!(
        !finished.iter().any(|(is_err, _)| !is_err),
        "the cancelled capability must not report a successful finish; got {finished:?}"
    );
}
