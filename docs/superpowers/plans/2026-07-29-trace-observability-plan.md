# Trace Observability System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a unified, structured event stream — Trace — emitted directly from the codecoder agent kernel, recording every execution step in an append-only NDJSON file.

**Architecture:** TraceEmitter embedded in AgentLoop → channel → TraceWriter dedicated thread → `.ccd.trace.ndjson` file. TraceReader for post-hoc query and LLM self-observation. CODECODER_TRACE=1 env var to enable.

**Tech Stack:** Rust + serde_json + std::sync::mpsc + std::thread

## Global Constraints

- All operations O(1) in TraceEmitter; zero overhead when disabled (None + `as_mut().map(...)`)
- Writer runs on dedicated thread, never blocks AgentLoop
- StreamDelta events off by default
- NDJSON format: `{"type":"s"|"e"|"p", ...}` with meta header line
- span_id: `sp_{session_id_short}_{counter}`
- File rotation at 10MB, keep 3 rotated files
- CODECODER_TRACE=1 env var to enable (default: disabled)
- span_stack to auto-infer parent_id (top of stack = current parent)
- All AgentEvent → TraceEvent translation handled in `on_agent_event`

---

### Task 1: Create `src/trace/types.rs` — TraceEvent data types

**Files:**
- Create: `src/trace/types.rs`

**Interfaces:**
- Produces: `TraceEvent`, `SpanKind`, `EventKind`, `TouchType`, `MessageSource`, `PermissionDecision`, `span_id()`, `now_ts()`

- [ ] **Step 1: Write the file**

```rust
//! Trace event types for the observability system (spec 2026-07-29).
//! NDJSON format: `{"type":"s"|"e"|"p", ...}` with meta header line.
use serde::Serialize;

/// Seconds since epoch with millisecond precision.
pub fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Generate a span_id: `sp_{session_id_short}_{counter}`.
/// session_id_short = first 12 hex chars of the root session ID.
pub fn span_id(session_id: &str, counter: u64) -> String {
    let short = if session_id.len() > 12 { &session_id[..12] } else { session_id };
    format!("sp_{short}_{counter:04}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Turn,
    LlmCall,
    ToolCall,
    SubAgent,
    Milestone,
    Reasoning,
    Compaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchType {
    Hit,
    Read,
    Edit,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    Manual,
    Auto,
    Injected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Granted,
    Denied,
    Cancelled,
    AutoGranted,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    FileTouch { path: String, touch: TouchType, lines: Option<[u32; 2]> },
    PermissionCheck { key: String, decision: PermissionDecision },
    UserMessage { source: MessageSource, summary: String },
    Notice { text: String },
    StreamDelta { text: String },
    SubAgentResult { agent_id: String, summary: String },
    WorkgraphStatus { total: usize, pending: usize, done: usize, needs_fix: usize },
    ExitCode { code: i32 },
}

/// A span start event.
#[derive(Debug, Clone, Serialize)]
pub struct SpanStart {
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub kind: SpanKind,
    pub ts: f64,
    pub meta: serde_json::Value,
}

/// A span end event.
#[derive(Debug, Clone, Serialize)]
pub struct SpanEnd {
    pub span_id: String,
    pub ts: f64,
    pub meta: serde_json::Value,
}

/// A point event (instantaneous, no duration).
#[derive(Debug, Clone, Serialize)]
pub struct PointEvent {
    pub kind: EventKind,
    pub ts: f64,
    pub meta: serde_json::Value,
}

/// One NDJSON line.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceEvent {
    S(SpanStart),
    E(SpanEnd),
    P(PointEvent),
}

impl TraceEvent {
    pub fn span_start(span_id: String, parent_id: Option<String>, kind: SpanKind, meta: serde_json::Value) -> Self {
        TraceEvent::S(SpanStart { span_id, parent_id, kind, ts: now_ts(), meta })
    }
    pub fn span_end(span_id: String, meta: serde_json::Value) -> Self {
        TraceEvent::E(SpanEnd { span_id, ts: now_ts(), meta })
    }
    pub fn point(kind: EventKind, meta: serde_json::Value) -> Self {
        TraceEvent::P(PointEvent { kind, ts: now_ts(), meta })
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src/trace/types.rs
git commit -m "feat(trace): add TraceEvent data types

Define SpanKind, EventKind, TouchType, MessageSource, PermissionDecision
and the NDJSON wire format (type:s/e/p). span_id generation uses
session_id_short + counter.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task 2: Create `src/trace/writer.rs` — TraceWriter with rotation

**Files:**
- Create: `src/trace/writer.rs`

**Interfaces:**
- Consumes: from Task 1: `TraceEvent`, `now_ts`
- Produces: `TraceWriter::spawn(root) -> Sender<TraceEvent>`

- [ ] **Step 1: Write the file**

```rust
//! Dedicated thread that drains TraceEvent channel and writes `.ccd.trace.ndjson`.
//! 10 MB rotation, keep 3 rotated files. Never blocks the AgentLoop.
use crate::trace::types::TraceEvent;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

const ROTATE_SIZE: u64 = 10 * 1024 * 1024;
const MAX_ROTATED: usize = 3;

pub struct TraceWriter {
    rx: Receiver<TraceEvent>,
    ndjson: Option<std::fs::File>,
    ndjson_path: PathBuf,
    bytes_written: u64,
    initial_metadata_written: bool,
}

impl TraceWriter {
    /// Spawn a dedicated thread for writing trace events. Returns the channel Sender.
    /// File: `<root>/.ccd.trace.ndjson`
    pub fn spawn(root: &Path) -> Sender<TraceEvent> {
        let (tx, rx) = std::sync::mpsc::channel::<TraceEvent>();
        let path = root.join(".ccd.trace.ndjson");
        std::thread::spawn(move || {
            let mut writer = TraceWriter {
                rx,
                ndjson: None,
                ndjson_path: path,
                bytes_written: 0,
                initial_metadata_written: false,
            };
            writer.run();
        });
        tx
    }

    fn run(&mut self) {
        for event in self.rx {
            if !self.initial_metadata_written {
                self.initial_metadata_written = true;
                let header = serde_json::json!({
                    "type": "meta",
                    "version": 1,
                    "ts": crate::trace::types::now_ts(),
                    "pid": std::process::id(),
                });
                self.write_line(&header.to_string());
            }
            self.maybe_rotate();
            let json = match serde_json::to_string(&event) {
                Ok(j) => j,
                Err(e) => format!("{{\"type\":\"error\",\"msg\":\"serialize failed: {e}\"}}"),
            };
            self.write_line(&json);
        }
    }

    fn write_line(&mut self, line: &str) {
        let file = self.ndjson.get_or_insert_with(|| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.ndjson_path)
                .expect("failed to open trace file")
        });
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
        self.bytes_written += line.len() as u64 + 1;
    }

    fn maybe_rotate(&mut self) {
        if self.bytes_written < ROTATE_SIZE {
            return;
        }
        rotate_ndjson(&self.ndjson_path);
        self.ndjson = Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.ndjson_path)
                .expect("failed to reopen trace file after rotation"),
        );
        self.bytes_written = 0;
    }
}

fn rotate_ndjson(path: &Path) {
    let dir = path.parent().unwrap_or(Path::new("."));
    for i in (MAX_ROTATED..100).rev() {
        let old = dir.join(format!(".ccd.trace.{i}.ndjson"));
        let _ = std::fs::remove_file(&old);
    }
    for i in (1..MAX_ROTATED).rev() {
        let src = dir.join(format!(".ccd.trace.{i}.ndjson"));
        let dst = dir.join(format!(".ccd.trace.{}.ndjson", i + 1));
        let _ = std::fs::rename(&src, &dst);
    }
    let rotated = dir.join(".ccd.trace.1.ndjson");
    let _ = std::fs::rename(path, &rotated);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writer_creates_file_and_writes_meta() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let _ = tx.send(TraceEvent::span_start("sp_001".into(), None, crate::trace::types::SpanKind::Turn, serde_json::json!({})));
        drop(tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2, "meta + at least 1 event: {}", body);
        let meta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(meta["type"], "meta");
        assert_eq!(meta["version"], 1);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "s");
        assert_eq!(ev["span_id"], "sp_001");
    }

    #[test]
    fn writer_handles_span_end() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let _ = tx.send(TraceEvent::span_end("sp_001".into(), serde_json::json!({"duration_ms": 100})));
        drop(tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "e");
        assert_eq!(ev["meta"]["duration_ms"], 100);
    }

    #[test]
    fn writer_handles_point_event() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let _ = tx.send(TraceEvent::point(
            crate::trace::types::EventKind::Notice { text: "hello".into() },
            serde_json::json!({}),
        ));
        drop(tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "p");
        assert_eq!(ev["kind"], "notice");
    }
}
```

- [ ] **Step 2: Verify it compiles and tests pass**

Run: `cargo test trace::writer::tests -- --nocapture 2>&1 | tail -20`
Expected: all 3 tests pass

- [ ] **Step 3: Commit**

```bash
git add src/trace/writer.rs
git commit -m "feat(trace): add TraceWriter with NDJSON rotation

Dedicated thread drains channel and writes .ccd.trace.ndjson with
10MB rotation (keep 3). Meta header line written on first event.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task 3: Create `src/trace/emitter.rs` — TraceEmitter

**Files:**
- Create: `src/trace/emitter.rs`

**Interfaces:**
- Consumes: from Task 1: `TraceEvent`, `SpanKind`, `EventKind`, `TouchType`, `MessageSource`, `PermissionDecision`, `span_id`, `now_ts`
- Consumes: from Task 2: `Sender<TraceEvent>`
- Produces: `TraceEmitter` with `span_start`, `span_end`, `emit`, `on_agent_event`, `on_turn_start`, `on_llm_call_start`, `on_llm_call_end`, `on_tool_start`, `on_tool_end`

- [ ] **Step 1: Write the file**

```rust
//! TraceEmitter — embedded in AgentLoop, emits trace events to the writer channel.
//! O(1) operations, zero overhead when disabled (None + as_mut().map(...)).
use crate::trace::types::*;
use crate::agent::AgentEvent;
use std::sync::mpsc::Sender;

pub struct TraceEmitter {
    tx: Sender<TraceEvent>,
    span_stack: Vec<(String, SpanKind)>,
    next_span_id: u64,
    session_id_short: String,
    emit_stream_delta: bool,
}

impl TraceEmitter {
    /// Create a new TraceEmitter. `session_id` is the root session's ID string.
    pub fn new(tx: Sender<TraceEvent>, session_id: &str) -> Self {
        let short = if session_id.len() > 12 { &session_id[..12] } else { session_id };
        TraceEmitter {
            tx,
            span_stack: Vec::new(),
            next_span_id: 1,
            session_id_short: short.to_string(),
            emit_stream_delta: false,
        }
    }

    /// Start a span. Auto-inherits parent_id from top of span_stack.
    /// Returns the span_id.
    pub fn span_start(&mut self, kind: SpanKind, meta: serde_json::Value) -> String {
        let span_id = span_id(&self.session_id_short, self.next_span_id);
        self.next_span_id += 1;
        let parent_id = self.span_stack.last().map(|(id, _)| id.clone());
        let event = TraceEvent::span_start(span_id.clone(), parent_id, kind, meta);
        let _ = self.tx.send(event);
        self.span_stack.push((span_id.clone(), kind));
        span_id
    }

    /// End a span.
    pub fn span_end(&mut self, span_id: &str, meta: serde_json::Value) {
        let event = TraceEvent::span_end(span_id.into(), meta);
        let _ = self.tx.send(event);
        if let Some((id, _)) = self.span_stack.last() {
            if id == span_id {
                self.span_stack.pop();
            }
        }
    }

    /// Emit a point event.
    pub fn emit(&mut self, kind: EventKind, meta: serde_json::Value) {
        let event = TraceEvent::point(kind, meta);
        let _ = self.tx.send(event);
    }

    /// Translate an AgentEvent into trace events.
    pub fn on_agent_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::StreamDelta(text) => {
                if self.emit_stream_delta {
                    self.emit(EventKind::StreamDelta { text: text.clone() }, serde_json::json!({}));
                }
            }
            AgentEvent::Notice(text) => {
                self.emit(EventKind::Notice { text: text.clone() }, serde_json::json!({}));
            }
            AgentEvent::Reasoning(text) => {
                let preview: String = text.chars().take(200).collect();
                self.emit(EventKind::Notice { text: format!("reasoning: {preview}…") }, serde_json::json!({}));
            }
            AgentEvent::SubAgentMilestone(text) => {
                self.emit(EventKind::Notice { text: format!("sub-agent: {text}") }, serde_json::json!({}));
            }
            AgentEvent::Context { pct } => {
                self.emit(EventKind::Notice { text: format!("context: {pct}%") }, serde_json::json!({"pct": pct}));
            }
            AgentEvent::TokenUsage { .. } => {}
            _ => {}
        }
    }

    // --- Convenience wrappers for AgentLoop integration ---

    pub fn on_turn_start(&mut self) -> String {
        self.span_start(SpanKind::Turn, serde_json::json!({}))
    }

    pub fn on_llm_call_start(&mut self, model: &str, prompt_tokens: u32, prompt_preview: &str) -> String {
        self.span_start(SpanKind::LlmCall, serde_json::json!({
            "model": model,
            "prompt_tokens": prompt_tokens,
            "prompt_preview": prompt_preview,
        }))
    }

    pub fn on_llm_call_end(&mut self, span_id: &str, completion_tokens: u32, stop_reason: &str, duration_ms: u64) {
        self.span_end(span_id, serde_json::json!({
            "completion_tokens": completion_tokens,
            "stop_reason": stop_reason,
            "duration_ms": duration_ms,
        }))
    }

    pub fn on_tool_start(&mut self, name: &str, input_preview: &str) -> String {
        self.span_start(SpanKind::ToolCall, serde_json::json!({
            "tool": name,
            "input_preview": input_preview,
        }))
    }

    pub fn on_tool_end(&mut self, span_id: &str, is_error: bool, output_preview: &str, target_files: &[String]) {
        self.span_end(span_id, serde_json::json!({
            "is_error": is_error,
            "output_preview": output_preview,
            "target_files": target_files,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn make_emitter() -> (TraceEmitter, mpsc::Receiver<TraceEvent>) {
        let (tx, rx) = mpsc::channel();
        (TraceEmitter::new(tx, "sess_test123456"), rx)
    }

    #[test]
    fn span_start_emits_s_event() {
        let (mut e, rx) = make_emitter();
        let id = e.span_start(SpanKind::Turn, serde_json::json!({}));
        let ev = rx.recv().unwrap();
        match ev {
            TraceEvent::S(s) => {
                assert_eq!(s.span_id, id);
                assert!(s.parent_id.is_none());
                assert_eq!(s.kind, SpanKind::Turn);
            }
            _ => panic!("expected span start"),
        }
    }

    #[test]
    fn span_end_emits_e_event() {
        let (mut e, rx) = make_emitter();
        let id = e.span_start(SpanKind::ToolCall, serde_json::json!({}));
        let _ = rx.recv().unwrap();
        e.span_end(&id, serde_json::json!({"duration_ms": 50}));
        let ev = rx.recv().unwrap();
        match ev {
            TraceEvent::E(s) => { assert_eq!(s.span_id, id); }
            _ => panic!("expected span end"),
        }
    }

    #[test]
    fn point_emits_p_event() {
        let (mut e, rx) = make_emitter();
        e.emit(EventKind::Notice { text: "test".into() }, serde_json::json!({}));
        let ev = rx.recv().unwrap();
        match ev {
            TraceEvent::P(p) => {
                match &p.kind {
                    EventKind::Notice { text } => assert_eq!(text, "test"),
                    _ => panic!("expected notice"),
                }
            }
            _ => panic!("expected point event"),
        }
    }

    #[test]
    fn span_stack_auto_parents_nested_spans() {
        let (mut e, rx) = make_emitter();
        let turn_id = e.span_start(SpanKind::Turn, serde_json::json!({}));
        let _ = rx.recv().unwrap();
        let llm_id = e.span_start(SpanKind::LlmCall, serde_json::json!({}));
        let ev = rx.recv().unwrap();
        match ev {
            TraceEvent::S(s) => {
                assert_eq!(s.parent_id, Some(turn_id.clone()));
            }
            _ => panic!("expected span start"),
        }
        e.span_end(&llm_id, serde_json::json!({}));
        let _ = rx.recv().unwrap();
        e.span_end(&turn_id, serde_json::json!({}));
        let _ = rx.recv().unwrap();
    }

    #[test]
    fn on_agent_event_notice() {
        let (mut e, rx) = make_emitter();
        e.on_agent_event(&AgentEvent::Notice("hello".into()));
        let got = rx.recv().unwrap();
        match got {
            TraceEvent::P(p) => {
                match &p.kind {
                    EventKind::Notice { text } => assert_eq!(text, "hello"),
                    _ => panic!("expected notice"),
                }
            }
            _ => panic!("expected point event"),
        }
    }

    #[test]
    fn on_agent_event_context() {
        let (mut e, rx) = make_emitter();
        e.on_agent_event(&AgentEvent::Context { pct: 42 });
        let got = rx.recv().unwrap();
        match got {
            TraceEvent::P(p) => { assert_eq!(p.meta["pct"], 42); }
            _ => panic!("expected point event"),
        }
    }

    #[test]
    fn span_id_format() {
        let id = span_id("sess_test123456", 1);
        assert_eq!(id, "sp_sess_test123_0001", "got: {id}");
    }

    #[test]
    fn span_id_truncates_long_session_id() {
        let id = span_id("session_abcdef1234567890", 42);
        assert_eq!(id.len(), 22);
        assert!(id.starts_with("sp_session_abcd"));
    }
}
```

- [ ] **Step 2: Verify it compiles and tests pass**

Run: `cargo test trace::emitter::tests -- --nocapture 2>&1 | tail -30`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/trace/emitter.rs
git commit -m "feat(trace): add TraceEmitter with span stack and AgentEvent mapping"
```

### Task 4: Create `src/trace/reader.rs` — TraceReader

**Files:**
- Create: `src/trace/reader.rs`

**Interfaces:**
- Consumes: from Task 1: `TraceEvent`, `SpanKind`, `EventKind`, `TouchType`
- Produces: `TraceReader` with `read_tree`, `recent_events`, `render_for_llm`

- [ ] **Step 1: Write the file**

```rust
//! TraceReader — reads `.ccd.trace.ndjson` and provides structured queries.
use crate::trace::types::*;
use std::collections::HashMap;
use std::path::Path;

pub struct SpanNode {
    pub span: SpanStart,
    pub end: Option<SpanEnd>,
    pub children: Vec<SpanNode>,
    pub events: Vec<PointEvent>,
}

pub struct TraceReader {
    path: std::path::PathBuf,
}

impl TraceReader {
    pub fn new(path: &Path) -> Self {
        TraceReader { path: path.to_path_buf() }
    }

    pub fn read_tree(&self) -> std::io::Result<(serde_json::Value, Vec<SpanNode>)> {
        let body = std::fs::read_to_string(&self.path)?;
        let mut meta = serde_json::json!({});
        let mut spans_by_id: HashMap<String, SpanNode> = HashMap::new();
        let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
        let mut point_events: Vec<PointEvent> = Vec::new();
        let mut root_spans: Vec<String> = Vec::new();
        let mut span_order: Vec<String> = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let type_str = v["type"].as_str().unwrap_or("");
            if type_str == "meta" { meta = v; continue; }
            match type_str {
                "s" => {
                    if let Some(span) = parse_span_start(&v) {
                        let id = span.span_id.clone();
                        spans_by_id.entry(id.clone()).or_insert_with(|| SpanNode { span, end: None, children: Vec::new(), events: Vec::new() });
                        span_order.push(id.clone());
                        if let Some(parent) = v.get("parent_id").and_then(|p| p.as_str()) {
                            children_of.entry(parent.to_string()).or_default().push(id);
                        } else {
                            root_spans.push(id);
                        }
                    }
                }
                "e" => {
                    if let Some(span_id) = v.get("span_id").and_then(|s| s.as_str()) {
                        if let Some(node) = spans_by_id.get_mut(span_id) {
                            node.end = Some(parse_span_end(&v));
                        }
                    }
                }
                "p" => { point_events.push(parse_point_event(&v)); }
                _ => {}
            }
        }

        let mut roots = Vec::new();
        for root_id in &root_spans {
            if let Some(node) = spans_by_id.remove(root_id) {
                roots.push(build_tree(node, &mut spans_by_id, &children_of));
            }
        }
        let mut remaining: Vec<String> = spans_by_id.keys().cloned().collect();
        remaining.sort_by(|a, b| {
            span_order.iter().position(|x| x == a).unwrap_or(0)
                .cmp(&span_order.iter().position(|x| x == b).unwrap_or(0))
        });
        for orphan_id in remaining {
            if let Some(node) = spans_by_id.remove(&orphan_id) {
                roots.push(node);
            }
        }

        for pe in &point_events {
            for root in &mut roots {
                attach_event(root, pe);
            }
        }

        Ok((meta, roots))
    }

    pub fn recent_events(&self, n: usize) -> std::io::Result<Vec<String>> {
        let body = std::fs::read_to_string(&self.path)?;
        let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = if lines.len() > n { lines.len() - n } else { 1 };
        Ok(lines[start..].iter().map(|s| (*s).to_string()).collect())
    }

    pub fn render_for_llm(&self, max_events: usize) -> std::io::Result<String> {
        let (_meta, tree) = self.read_tree()?;
        let mut output = String::new();
        output.push_str("## Trace\n\n");
        let mut count = 0usize;
        for root in &tree {
            output.push_str(&render_node(root, 0, &mut count, max_events));
            if count >= max_events { break; }
        }
        output.push_str("\n### 文件 touch 汇总\n");
        let mut file_touches: Vec<String> = Vec::new();
        collect_file_touches(&tree, &mut file_touches);
        for ft in file_touches.iter().take(20) {
            output.push_str(&format!("  {ft}\n"));
        }
        Ok(output)
    }
}

fn parse_span_start(v: &serde_json::Value) -> Option<SpanStart> {
    let span_id = v["span_id"].as_str()?.to_string();
    let parent_id = v.get("parent_id").and_then(|p| p.as_str()).map(|s| s.to_string());
    let kind = match v["kind"].as_str()? {
        "turn" => SpanKind::Turn,
        "llm_call" => SpanKind::LlmCall,
        "tool_call" => SpanKind::ToolCall,
        "sub_agent" => SpanKind::SubAgent,
        "milestone" => SpanKind::Milestone,
        "reasoning" => SpanKind::Reasoning,
        "compaction" => SpanKind::Compaction,
        _ => return None,
    };
    Some(SpanStart { span_id, parent_id, kind, ts: v["ts"].as_f64().unwrap_or(0.0), meta: v.get("meta").cloned().unwrap_or_default() })
}

fn parse_span_end(v: &serde_json::Value) -> SpanEnd {
    SpanEnd { span_id: v["span_id"].as_str().unwrap_or("").to_string(), ts: v["ts"].as_f64().unwrap_or(0.0), meta: v.get("meta").cloned().unwrap_or_default() }
}

fn parse_point_event(v: &serde_json::Value) -> PointEvent {
    PointEvent { kind: EventKind::Notice { text: v["kind"].as_str().unwrap_or("notice").to_string() }, ts: v["ts"].as_f64().unwrap_or(0.0), meta: v.get("meta").cloned().unwrap_or_default() }
}

fn build_tree(node: SpanNode, remaining: &mut HashMap<String, SpanNode>, children_of: &HashMap<String, Vec<String>>) -> SpanNode {
    let mut node = node;
    if let Some(child_ids) = children_of.get(&node.span.span_id).cloned() {
        for child_id in child_ids {
            if let Some(child) = remaining.remove(&child_id) {
                node.children.push(build_tree(child, remaining, children_of));
            }
        }
    }
    node
}

fn attach_event(node: &mut SpanNode, event: &PointEvent) {
    if event.ts >= node.span.ts && node.end.as_ref().map(|e| event.ts <= e.ts).unwrap_or(true) {
        node.events.push(event.clone());
        return;
    }
    for child in &mut node.children { attach_event(child, event); }
}

fn render_node(node: &SpanNode, depth: usize, count: &mut usize, max: usize) -> String {
    if *count >= max { return String::new(); }
    *count += 1;
    let indent = "  ".repeat(depth);
    let kind_str = format!("{:?}", node.span.kind);
    let duration = node.end.as_ref().and_then(|e| e.meta.get("duration_ms").and_then(|v| v.as_f64())).map(|d| format!(" ({:.0}ms)", d)).unwrap_or_default();
    let tool_info = node.span.meta.get("tool").and_then(|v| v.as_str()).map(|t| format!(" [{t}]")).unwrap_or_default();
    let mut out = format!("{indent}{kind_str}{duration}{tool_info}\n");
    if let Some(preview) = node.span.meta.get("input_preview").and_then(|v| v.as_str()) {
        out.push_str(&format!("{indent}  输入: {preview}\n"));
    }
    for child in &node.children { out.push_str(&render_node(child, depth + 1, count, max)); }
    out
}

fn collect_file_touches(tree: &[SpanNode], out: &mut Vec<String>) {
    for node in tree {
        for event in &node.events {
            if let EventKind::FileTouch { path, touch, .. } = &event.kind {
                out.push(format!("{}: [{:?}]", path, touch));
            }
        }
        collect_file_touches(&node.children, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn read_tree_simple_span() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        std::fs::write(&path, r#"{"type":"meta","version":1,"ts":1000.0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1100.0,"meta":{"duration_ms":100}}
"#).unwrap();
        let reader = TraceReader::new(&path);
        let (meta, tree) = reader.read_tree().unwrap();
        assert_eq!(meta["version"], 1);
        assert_eq!(tree.len(), 1);
        assert!(tree[0].end.is_some());
    }

    #[test]
    fn read_tree_nested_spans() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        std::fs::write(&path, r#"{"type":"meta","version":1,"ts":1000.0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"llm_call","ts":1000.1,"meta":{}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{"duration_ms":900}}
{"type":"e","span_id":"sp_001","ts":1001.0,"meta":{"duration_ms":1000}}
"#).unwrap();
        let reader = TraceReader::new(&path);
        let (_meta, tree) = reader.read_tree().unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 1);
    }

    #[test]
    fn read_tree_handles_point_events() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        std::fs::write(&path, r#"{"type":"meta","version":1,"ts":1000.0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"notice","ts":1000.5,"meta":{"text":"hello"}}
{"type":"e","span_id":"sp_001","ts":1100.0,"meta":{}}
"#).unwrap();
        let reader = TraceReader::new(&path);
        let (_meta, _tree) = reader.read_tree().unwrap();
    }

    #[test]
    fn recent_events_returns_last_n() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        let mut content = r#"{"type":"meta","version":1,"ts":1000.0}
"#.to_string();
        for i in 0..10 {
            content.push_str(&format!(r#"{{"type":"s","span_id":"sp_{i:04}","parent_id":null,"kind":"turn","ts":{}.0,"meta":{{}}}}
"#, 1000.0 + i as f64));
        }
        std::fs::write(&path, &content).unwrap();
        let reader = TraceReader::new(&path);
        let events = reader.recent_events(3).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn render_for_llm_includes_tool_info() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        std::fs::write(&path, r#"{"type":"meta","version":1,"ts":1000.0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"tool_call","ts":1000.1,"meta":{"tool":"read_file","input_preview":"file_path: src/main.rs"}}
{"type":"e","span_id":"sp_002","ts":1000.2,"meta":{"is_error":false,"output_preview":"fn main() { ... }"}}
{"type":"e","span_id":"sp_001","ts":1001.0,"meta":{}}
"#).unwrap();
        let reader = TraceReader::new(&path);
        let rendered = reader.render_for_llm(100).unwrap();
        assert!(rendered.contains("ToolCall"), "expected ToolCall in render, got: {rendered}");
    }
}
```

- [ ] **Step 2: Verify it compiles and tests pass**

Run: `cargo test trace::reader::tests -- --nocapture 2>&1 | tail -30`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/trace/reader.rs
git commit -m "feat(trace): add TraceReader with tree reconstruction and LLM render"
```

### Task 5: Create `src/trace/mod.rs` — module entry + init_trace

**Files:**
- Create: `src/trace/mod.rs`

**Interfaces:**
- Consumes: from Task 1: re-exports types
- Consumes: from Task 2: `TraceWriter`
- Consumes: from Task 3: `TraceEmitter`
- Consumes: from Task 4: `TraceReader`
- Produces: `init_trace(root) -> Option<TraceEmitter>`

- [ ] **Step 1: Write the file**

```rust
//! Trace observability system (spec 2026-07-29).
//! Enable with `CODECODER_TRACE=1` env var.
//! Writes to `<root>/.ccd.trace.ndjson`.
pub mod types;
pub mod emitter;
pub mod writer;
pub mod reader;

pub use types::*;
pub use emitter::TraceEmitter;
pub use writer::TraceWriter;
pub use reader::TraceReader;

pub fn init_trace(root: &std::path::Path) -> Option<TraceEmitter> {
    let enabled = std::env::var("CODECODER_TRACE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if !enabled { return None; }
    let tx = TraceWriter::spawn(root);
    let session_id = root.file_stem().and_then(|s| s.to_str()).unwrap_or("agent").to_string();
    Some(TraceEmitter::new(tx, &session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_trace_returns_none_when_env_not_set() {
        unsafe { std::env::remove_var("CODECODER_TRACE"); }
        assert!(init_trace(&tempdir().unwrap().path()).is_none());
    }

    #[test]
    fn init_trace_returns_some_when_env_is_1() {
        unsafe { std::env::set_var("CODECODER_TRACE", "1"); }
        assert!(init_trace(&tempdir().unwrap().path()).is_some());
        unsafe { std::env::remove_var("CODECODER_TRACE"); }
    }

    #[test]
    fn init_trace_returns_some_when_env_is_true() {
        unsafe { std::env::set_var("CODECODER_TRACE", "true"); }
        assert!(init_trace(&tempdir().unwrap().path()).is_some());
        unsafe { std::env::remove_var("CODECODER_TRACE"); }
    }
}
```

- [ ] **Step 2: Verify it compiles and tests pass**

Run: `cargo test trace::tests -- --nocapture 2>&1 | tail -20`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/trace/mod.rs
git commit -m "feat(trace): add module entry with init_trace"
```

### Task 6: Register `trace` module in `src/lib.rs` and add config

**Files:**
- Modify: `src/lib.rs` — add `pub mod trace`
- Modify: `src/config.rs` — add `trace_enabled` field

- [ ] **Step 1: Add `pub mod trace` to `src/lib.rs`** after `pub mod tool;`

- [ ] **Step 2: Add config field** — after `pub max_ledger_lines: u32`, add `pub trace_enabled: bool`. In `from_env()`, add `trace_enabled: env("CODECODER_TRACE").map(|v| v == "1" || v == "true").unwrap_or(false),`. Add `"CODECODER_TRACE"` to `DOTENV_ALLOWED_KEYS`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/config.rs
git commit -m "feat(trace): register trace module and add config"
```

### Task 7: Integrate TraceEmitter into AgentLoop

**Files:**
- Modify: `src/agent.rs` — add `trace_emitter: Option<TraceEmitter>` field, instrument `process_turn`

- [ ] **Step 1: Add `trace_emitter` field** to `AgentLoop` struct (after `shared_registry`). In `build()`, add `trace_emitter: crate::trace::init_trace(&root),`.

- [ ] **Step 2: Instrument `process_turn`** — add `let turn_span = self.trace_emitter.as_mut().map(|t| t.on_turn_start());` at top of loop body. Add `llm_call` span around LLM completion. Add `tool_call` span around tool dispatch. Add `span_end` for turn_span before `TurnComplete`.

- [ ] **Step 3: Instrument `drain_steer`** — emit `UserMessage` event.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check 2>&1 | head -30`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/agent.rs
git commit -m "feat(trace): integrate TraceEmitter into AgentLoop"
```

### Task 8: Integrate TraceEmitter into background.rs

**Files:**
- Modify: `src/background.rs` — add trace emitter to drain_bg_events

- [ ] **Step 1: Modify `drain_bg_events`** to accept `trace: Option<&mut crate::trace::TraceEmitter>` parameter. Forward AgentEvent to trace.

- [ ] **Step 2: Update all callers** of `drain_bg_events` to pass `None` (or a real trace emitter).

- [ ] **Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -30`
Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add src/background.rs
git commit -m "feat(trace): integrate TraceEmitter into background drain"
```

### Task 9: Full integration test

**Files:**
- Create: `tests/trace_integration.rs`

- [ ] **Step 1: Write the integration test**

```rust
//! Integration test for the trace observability system.
use std::sync::Arc;

#[test]
fn trace_file_created_when_enabled() {
    unsafe { std::env::set_var("CODECODER_TRACE", "1"); }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let provider = Arc::new(crate::provider::stub::StubClient);
    let mut agent = crate::agent::AgentLoop::new(provider, "gpt-4o", 1024, 0.0, root.clone());
    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    agent.run_one_turn("hello".to_string(), &event_tx);
    let trace_path = root.join(".ccd.trace.ndjson");
    assert!(trace_path.exists(), "trace file should exist: {:?}", trace_path);
    let body = std::fs::read_to_string(&trace_path).unwrap();
    let first = body.lines().filter(|l| !l.trim().is_empty()).next().expect("at least meta line");
    let meta: serde_json::Value = serde_json::from_str(first).unwrap();
    assert_eq!(meta["type"], "meta");
    assert_eq!(meta["version"], 1);
    let ev_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(ev_count >= 2, "expected at least 2 events, got: {ev_count}");
    unsafe { std::env::remove_var("CODECODER_TRACE"); }
}

#[test]
fn trace_file_not_created_when_disabled() {
    unsafe { std::env::remove_var("CODECODER_TRACE"); }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let provider = Arc::new(crate::provider::stub::StubClient);
    let mut agent = crate::agent::AgentLoop::new(provider, "gpt-4o", 1024, 0.0, root.clone());
    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    agent.run_one_turn("hello".to_string(), &event_tx);
    let trace_path = root.join(".ccd.trace.ndjson");
    assert!(!trace_path.exists(), "trace file should NOT exist when disabled");
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test trace_integration -- --nocapture 2>&1 | tail -20`
Expected: both tests pass

- [ ] **Step 3: Commit**

```bash
git add tests/trace_integration.rs
git commit -m "test(trace): add integration tests for trace file creation"
```