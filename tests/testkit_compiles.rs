mod testkit;
#[test]
fn testkit_builds() {
    let (_p, rec) = testkit::ScriptedProvider::new(vec![testkit::assistant_text("hi")]);
    assert_eq!(rec.lock().unwrap().len(), 0);
}

/// End-to-end proof that the driver drives the real AgentLoop: seed a workspace
/// with AGENTS.md, script one assistant text turn, and assert the turn completes
/// with the streamed text surfaced.
#[test]
fn driver_smoke() {
    use testkit::{PermPolicy, ScriptedProvider, Workspace, assistant_text, run_turn};

    let ws = Workspace::new();
    ws.write("AGENTS.md", "# Test Agent\nYou are a test agent.\n");

    let (provider, recorder) = ScriptedProvider::new(vec![assistant_text("hi")]);
    let out = run_turn(
        ws.root(),
        provider,
        recorder,
        "say hi",
        PermPolicy::GrantOnce,
        vec![],
    );

    assert!(
        matches!(out.events.last(), Some(codecoder::AgentEvent::TurnComplete)),
        "expected last event to be TurnComplete, got {:?}",
        out.events.last().map(std::mem::discriminant)
    );
    assert!(
        out.stream_text().contains("hi"),
        "expected streamed text to contain \"hi\", got {:?}",
        out.stream_text()
    );
    assert!(
        !out.requests.is_empty(),
        "expected the provider to have been called at least once"
    );
}
