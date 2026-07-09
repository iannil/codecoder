// L1 — Session persistence + forward migration on resume (ADR 0004, §5.6).
//
// Real schema (src/session.rs, src/message.rs), calibrated from a live turn:
//   - version field is `schema_version` (u32), CURRENT `SCHEMA_VERSION == 1`.
//   - `Session` = { schema_version, model, token_count, messages }.
//   - `Message`     = { id: u64, role: "user"|"assistant"|"system"|"tool",
//                       items: [MessageItem] }.
//   - `MessageItem` is internally tagged: `#[serde(tag = "item",
//                       rename_all = "snake_case")]`, so a text item serializes
//                       as {"item":"text","text":"..."} (NOT {"Text":{...}}).
//   - Files persist at `sessions/session-<stamp>.json`, autosaved on every append.
//   - Forward-migration chain (`Session::load` → `migrate`): version 0 -> 1 is a
//     registered step (identity transform). So a fixture at `schema_version: 0`
//     genuinely drives the migration loop (`while version < SCHEMA_VERSION`),
//     not merely a same-version round-trip.
mod testkit;
use testkit::*;

/// Test 1: a completed turn must persist a session JSON that carries the
/// version field and the user's message text — the real field names.
#[test]
fn turn_persists_session_json_with_version() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, rec) = ScriptedProvider::new(vec![assistant_text("hi there")]);
    let _ = run_turn(
        ws.root(),
        p,
        rec,
        "hello world msg",
        PermPolicy::GrantOnce,
        vec![],
    );

    let dir = ws.root().join("sessions");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .expect("sessions/ dir must exist after a turn")
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert!(
        !files.is_empty(),
        "a session JSON must be written after a turn"
    );

    let body = std::fs::read_to_string(files[0].path()).unwrap();
    // Real version field name is `schema_version` (superset-matches "version").
    assert!(
        body.contains("\"schema_version\""),
        "session must carry the schema_version field; got:\n{body}"
    );
    assert!(
        body.contains("hello world msg"),
        "session must persist the user message text; got:\n{body}"
    );
}

/// Test 2: `/resume` (AgentCommand::Resume) must load a hand-authored
/// OLDER-version session fixture, forward-migrate it through the real chain
/// (schema_version 0 -> 1), adopt it as the live session, and — on the next
/// turn — replay the migrated history (OLD_MSG) into the provider request.
///
/// The fixture is authored in the REAL current schema but stamped at the OLDER
/// version (0) so `Session::load` runs the migration loop rather than a
/// same-version deserialize. This exercises a genuine multi-version migration
/// (0 -> 1), not just a round-trip.
#[test]
fn resume_migrates_older_session_fixture() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");

    // Older-version fixture: schema_version 0 (current is 1), real message shape.
    ws.write(
        "sessions/session-old.json",
        r#"{
  "schema_version": 0,
  "model": "gpt-4o",
  "token_count": 0,
  "messages": [
    { "id": 0, "role": "user", "items": [ { "item": "text", "text": "OLD_MSG" } ] }
  ]
}"#,
    );

    let (p, rec) = ScriptedProvider::new(vec![assistant_text("resumed")]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![Step::Resume, Step::Msg("continue".into())],
        PermPolicy::GrantOnce,
    );

    // After resume, the migrated history (OLD_MSG) must be replayed to the
    // provider on the subsequent turn.
    assert!(
        out.requests
            .iter()
            .any(|r| format!("{:?}", r.messages).contains("OLD_MSG")),
        "resume must load + forward-migrate (v0->v1) + replay the older session; \
         provider request message-sets were:\n{:#?}",
        out.requests
            .iter()
            .map(|r| format!("{:?}", r.messages))
            .collect::<Vec<_>>()
    );
}
