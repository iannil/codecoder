//! Integration test for the trace observability system.
//! Verifies that when CODECODER_TRACE=1, a trace file is created with
//! the expected NDJSON format during an actual agent turn.

#[test]
fn trace_file_created_when_enabled() {
    unsafe { std::env::set_var("CODECODER_TRACE", "1"); }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let provider = std::sync::Arc::new(codecoder::provider::stub::StubClient);
    let mut agent = codecoder::agent::AgentLoop::new(
        provider,
        "gpt-4o",
        1024,
        0.0,
        root.clone(),
    );

    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    agent.run_one_turn("hello".to_string(), &event_tx);

    let trace_path = root.join(".ccd.trace.ndjson");
    assert!(trace_path.exists(), "trace file should exist: {:?}", trace_path);

    let body = std::fs::read_to_string(&trace_path).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.len() >= 2, "expected at least 2 lines (meta + event), got: {}", lines.len());

    let meta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(meta["type"], "meta");
    assert_eq!(meta["version"], 1);

    // Verify we have a valid span_start event
    let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(ev["type"], "s", "first event should be span_start, got: {:?}", ev);

    unsafe { std::env::remove_var("CODECODER_TRACE"); }
}

#[test]
fn trace_file_not_created_when_disabled() {
    unsafe { std::env::remove_var("CODECODER_TRACE"); }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let provider = std::sync::Arc::new(codecoder::provider::stub::StubClient);
    let mut agent = codecoder::agent::AgentLoop::new(
        provider,
        "gpt-4o",
        1024,
        0.0,
        root.clone(),
    );

    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    agent.run_one_turn("hello".to_string(), &event_tx);

    let trace_path = root.join(".ccd.trace.ndjson");
    assert!(!trace_path.exists(), "trace file should NOT exist when disabled");
}

#[test]
fn trace_file_contains_valid_json_lines() {
    unsafe { std::env::set_var("CODECODER_TRACE", "1"); }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let provider = std::sync::Arc::new(codecoder::provider::stub::StubClient);
    let mut agent = codecoder::agent::AgentLoop::new(
        provider,
        "gpt-4o",
        1024,
        0.0,
        root.clone(),
    );

    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    agent.run_one_turn("hello".to_string(), &event_tx);

    let body = std::fs::read_to_string(root.join(".ccd.trace.ndjson")).unwrap();
    for (i, line) in body.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let v: serde_json::Value = serde_json::from_str(line)
            .expect(&format!("line {} is not valid JSON: {}", i, line));
        let type_str = v["type"].as_str().unwrap_or("");
        assert!(
            type_str == "meta" || type_str == "s" || type_str == "e" || type_str == "p",
            "line {} has unknown type {:?}: {}",
            i, type_str, line
        );
    }

    unsafe { std::env::remove_var("CODECODER_TRACE"); }
}