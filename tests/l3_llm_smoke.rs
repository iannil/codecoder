// L3 — REAL-LLM smoke (opt-in). Requires a live model: CODECODER_API_KEY set
// and RUN_LLM_SMOKE=1. `#[ignore]`d so it never runs (or fails) in the default
// `cargo test`; it also early-returns when RUN_LLM_SMOKE is unset, so even an
// explicit `-- --ignored` run without the flag is a no-op rather than a failure.
//
// This is the priority smoke: it drives the SAME AgentLoop + real builtin tools
// as L1, but swaps the ScriptedProvider for a real provider selected by
// `select_provider(&cfg)`. The only observable asserted is the filesystem —
// the real model must choose `write_file` to satisfy the request.

mod testkit;
use testkit::*;

#[test]
#[ignore = "requires real LLM: RUN_LLM_SMOKE=1 + CODECODER_API_KEY"]
fn real_llm_can_create_a_file() {
    // Opt-in gate: absent the flag this is an intentional no-op (not a failure),
    // so `cargo test -- --ignored` on a box without a key stays green.
    if std::env::var("RUN_LLM_SMOKE").is_err() {
        return;
    }

    let ws = Workspace::new();
    ws.write(
        "AGENTS.md",
        "You are a coding agent. Use the available tools to fulfill requests. \
         When asked to create a file, call write_file.",
    );

    // `from_env()` picks up the real key/model/base; `select_provider` returns
    // the OpenAI client (or the scripted provider if CODECODER_SCRIPT is set —
    // callers running this layer should NOT set CODECODER_SCRIPT).
    let cfg = codecoder::Config::from_env();
    let provider = codecoder::select_provider(&cfg);

    // Real provider does not populate a recorder; a throwaway satisfies the
    // shared `run_turn` signature. We assert only on the filesystem face.
    let recorder = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let out = run_turn(
        ws.root(),
        provider,
        recorder,
        "Create a file named hello.txt containing exactly: HELLO",
        PermPolicy::GrantSession,
        vec![],
    );

    assert!(
        ws.exists("hello.txt"),
        "real model failed to drive write_file (hello.txt not created); \
         stream = {:?}",
        out.stream_text()
    );
}
