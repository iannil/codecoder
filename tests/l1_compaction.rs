// L1 — Compaction tier-1 (ADR 0023, §5.7).
//
// Black-box observation: the recorder captures the exact `messages` (the derived
// Context Working Set) sent to the provider on each `complete()` call. When the
// full history crosses COMPACTION_THRESHOLD (0.75) of the model window, tier-1:
//   - drops `Reasoning` items,
//   - elides old `ToolResult` bodies to `[elided by compaction: N chars]`,
//   - preserves the anchor (first user msg) and the most recent RECENT_TAIL (6) msgs.
// The persisted Session on disk is NEVER mutated — compaction is a derived view.
//
// CALIBRATION (read from src/):
//   - `run_steps` drives with model "test-model" → tokenizer::model_window falls to
//     the 128_000 default → threshold = 96_000 tokens.
//   - `"LOREM ".repeat(20_000)` ≈ 120_000 chars ≈ 20_000 tokens per read; 8 reads
//     ≈ 160_000 tokens of ToolResult bodies alone → well past 96_000.
//   - Placeholder string (compaction.rs:56): `[elided by compaction: {len} chars]`.

mod testkit;
use serde_json::json;
use testkit::*;

/// The exact placeholder prefix tier-1 writes over elided ToolResult bodies.
const ELIDED_PREFIX: &str = "[elided by compaction:";

#[test]
fn tier1_compaction_placeholders_old_tool_results_but_keeps_tail() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // A large payload so a few reads blow past the context threshold.
    let big = "LOREM ".repeat(20_000);
    ws.write("big.txt", &big);
    ws.write("tail_marker.txt", "TAIL_MARKER_UNIQUE");

    // Several read turns of big.txt (old bulk), then one read of the tail marker.
    let mut steps = Vec::new();
    let mut turns = Vec::new();
    for i in 0..8 {
        turns.push(assistant_tool_call(
            &format!("c{i}"),
            "read_file",
            json!({"path":"big.txt"}),
        ));
        turns.push(assistant_text("read"));
        steps.push(Step::Msg(format!("read {i}")));
    }
    turns.push(assistant_tool_call(
        "ct",
        "read_file",
        json!({"path":"tail_marker.txt"}),
    ));
    turns.push(assistant_text("read tail"));
    steps.push(Step::Msg("read tail".into()));

    let (p, rec) = ScriptedProvider::new(turns);
    let out = run_steps(ws.root(), p, rec, steps, PermPolicy::GrantOnce);

    let last = out.requests.last().expect("at least one provider request");
    let dump = format!("{:?}", last.messages);

    // (a) Near-tail content must be preserved verbatim.
    assert!(
        dump.contains("TAIL_MARKER_UNIQUE"),
        "near tail must survive compaction"
    );
    // (b) OLD big payloads must NOT appear in full anymore — their bodies were
    //     elided. Tier-1 keeps the most recent RECENT_TAIL (6) messages verbatim,
    //     so AT MOST ONE full big.txt body (the one inside the protected tail) may
    //     survive; the other 7 reads must be gone. Each full big.txt body yields a
    //     fixed number of non-overlapping "LOREM LOREM LOREM" matches, so the raw
    //     history (8 reads) would show ~8× that; a compacted view shows ≤ 1×.
    let one_body = ("LOREM ".repeat(20_000)).matches("LOREM LOREM LOREM").count();
    let big_occurrences = dump.matches("LOREM LOREM LOREM").count();
    assert!(
        big_occurrences <= one_body,
        "old ToolResult bodies must be placeholdered: last request holds {big_occurrences} \
         matches, more than the single tail body ({one_body}) tier-1 is allowed to keep"
    );
    // (c) The REAL placeholder string must be present, and MULTIPLE times — proving
    //     several old bodies were genuinely elided (not merely that history was cut).
    let placeholders = dump.matches(ELIDED_PREFIX).count();
    assert!(
        placeholders >= 2,
        "expected multiple elided ToolResult bodies via {ELIDED_PREFIX:?}, found {placeholders}"
    );

    // (d) Supporting signal: `Context{pct}` is emitted each iteration. NOTE: the
    //     reported pct is computed on the ALREADY-COMPACTED working set (agent.rs
    //     counts tokens on the post-`working_set` messages), so it reflects the
    //     shrunken size, not the full history that tripped the threshold — it will
    //     sit BELOW 75% precisely because compaction fired. We only assert a ctx%
    //     was reported and is substantial; (a)-(c) are the direct proof of tier-1.
    let max_pct = out
        .events
        .iter()
        .filter_map(|e| match e {
            codecoder::AgentEvent::Context { pct } => Some(*pct),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert!(
        max_pct >= 25,
        "expected a substantial post-compaction ctx% to be reported, peaked at {max_pct}%"
    );
}

#[test]
fn compaction_does_not_mutate_persisted_session() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let big = "LOREM ".repeat(20_000);
    ws.write("big.txt", &big);

    let mut steps = Vec::new();
    let mut turns = Vec::new();
    for i in 0..8 {
        turns.push(assistant_tool_call(
            &format!("c{i}"),
            "read_file",
            json!({"path":"big.txt"}),
        ));
        turns.push(assistant_text("r"));
        steps.push(Step::Msg(format!("r{i}")));
    }
    let (p, rec) = ScriptedProvider::new(turns);
    let out = run_steps(ws.root(), p, rec, steps, PermPolicy::GrantOnce);

    // Sanity: the run actually tripped compaction (otherwise this test is vacuous —
    // the point is that the DERIVED view compacted while the PERSISTED one did not).
    let last = out.requests.last().expect("at least one provider request");
    let last_dump = format!("{:?}", last.messages);
    assert!(
        last_dump.contains(ELIDED_PREFIX),
        "precondition: the derived working set must have compacted for this test to be meaningful"
    );

    // Persisted session is the full record (compaction is a derived working set).
    let dir = ws.root().join("sessions");
    let f = std::fs::read_dir(&dir)
        .expect("sessions dir")
        .flatten()
        .find(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .expect("a session-*.json file");
    let body = std::fs::read_to_string(f.path()).unwrap();

    // The full big payload survives on disk many times over (8 reads), and the
    // compaction placeholder must NOT have been persisted.
    let full = body.matches("LOREM").count();
    assert!(
        full > 100,
        "persisted session must retain full tool results, not the compacted view (found {full} LOREM tokens)"
    );
    assert!(
        !body.contains(ELIDED_PREFIX),
        "compaction placeholder must never be written to the persisted session"
    );
}

#[test]
fn tier2_summarizes_oldest_span_into_a_system_message() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // ~40k tokens each; 3 in history (~120k) exceeds the 96k threshold (0.75 * 128k
    // default window for "test-model") even after tier-1 (Text is not elided).
    let big = "LOREM ".repeat(20_000);
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_text(&format!("{big} OLD_TURN_MARK")), // turn 1 reply (in the middle)
        assistant_text(&big),                             // turn 2 reply
        assistant_text(&big),                             // turn 3 reply
        assistant_text("SUMMARY_MARK a concise summary"), // turn 4: the summary call
        assistant_text("final reply"),                    // turn 4: the actual turn reply
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![
            Step::Msg("ORIGINAL_GOAL".into()),
            Step::Msg("t2".into()),
            Step::Msg("t3".into()),
            Step::Msg("current CURRENT_MARK".into()),
        ],
        PermPolicy::GrantOnce,
    );

    // The summary request (issued inside context_working_set) is the one carrying the
    // tier-2 system prompt; select it by that marker (OLD_TURN_MARK alone is ambiguous —
    // the turn-1 reply is still in history on earlier normal turn requests too), then
    // assert the summary call actually rendered the old span.
    let summary_req = out
        .requests
        .iter()
        .find(|r| format!("{:?}", r.messages).contains("compacting an agent's conversation"))
        .expect("a summary call should have been issued");
    assert!(
        format!("{:?}", summary_req.messages).contains("OLD_TURN_MARK"),
        "the summary call must render the old span it is summarizing"
    );

    // The final actual-turn request replaces the middle with the summary System message.
    let last = out.requests.last().expect("at least one request");
    let dump = format!("{:?}", last.messages);
    assert!(dump.contains("SUMMARY_MARK"), "summary must be injected into the turn context");
    assert!(dump.contains("ORIGINAL_GOAL"), "anchor (first goal) must survive");
    assert!(!dump.contains("OLD_TURN_MARK"), "the summarized middle must be gone from the turn request");
}

#[test]
fn tier2_degrades_to_tier1_when_summary_is_empty() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let big = "LOREM ".repeat(20_000);
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_text(&format!("{big} OLD_TURN_MARK")),
        assistant_text(&big),
        assistant_text(&big),
        assistant_text(""),            // turn 4: summary call returns EMPTY → degrade
        assistant_text("final reply"), // turn 4: actual reply
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![
            Step::Msg("ORIGINAL_GOAL".into()),
            Step::Msg("t2".into()),
            Step::Msg("t3".into()),
            Step::Msg("current CURRENT_MARK".into()),
        ],
        PermPolicy::GrantOnce,
    );

    // Empty summary → no System summary injected; the turn still completes on tier-1.
    let last = out.requests.last().expect("at least one request");
    let dump = format!("{:?}", last.messages);
    assert!(!dump.contains("先前对话摘要"), "empty summary must not be injected");
    assert!(
        out.events.iter().any(|e| matches!(e, codecoder::AgentEvent::TurnComplete)),
        "the turn must still complete under graceful degrade"
    );

    // Positive control: tier-2 must have ATTEMPTED the summary (a regression that
    // skipped tier-2 entirely would otherwise pass this degrade test vacuously).
    assert!(
        out.requests.iter().any(|r| format!("{:?}", r.messages).contains("compacting an agent's conversation")),
        "the summary call must be attempted before degrading to tier-1"
    );
}
