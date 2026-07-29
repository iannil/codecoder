# Unified Observability System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade CodeCoder's trace observability system from a single `TraceEmitter` to a multi-observer `ObserverSet` architecture, fill all event type gaps, unify BgObserver, and enhance both human and LLM self-observation interfaces.

**Architecture:** ObserverSet dispatches every event to multiple registered Observers (TraceWriter, BgObserver, ReplayBuffer, SSE). All event types are captured at AgentLoop integration points. The design is backward-compatible: existing NDJSON consumers continue to work.

**Tech Stack:** Rust, existing mpsc channels, serde_json, NDJSON files

**Spec:** `docs/superpowers/specs/2026-07-29-unified-observability-design.md`

## Global Constraints

- All new `EventKind` variants must be backward-compatible: add to the `#[serde(tag = ...)]` enum, do not remove existing variants
- `ObserverSet` must degrade gracefully when an observer panics or fails (soft_fail mode)
- All new code must compile with `cargo build` and pass `cargo test`
- Follow existing code patterns: `//!` module docs, unit tests in same file, integration tests in `tests/`
- No new async runtime dependencies (OS threads + channels only)

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `src/trace/observer_set.rs` | `ObserverSet` struct + `Observer` trait definitions |
| `src/trace/observers/trace_writer.rs` | `TraceWriterObserver` — wraps existing `TraceWriter` channel |
| `src/trace/observers/replay_buffer.rs` | `ReplayBufferObserver` — wraps existing `ReplayBuffer` |
| `src/trace/observers/bg_observer.rs` | `BgObserver` — writes `.ccd.bg.ndjson` + stderr |
| `src/trace/observers/sse_observer.rs` | `SseObserver` — pushes to `visual/event_router` |
| `src/trace/observers/mod.rs` | Re-exports all observers |

### Modified Files

| File | Changes |
|------|---------|
| `src/trace/types.rs` | Add new `EventKind` variants, `SubAgentStatus`, `MessageSource` extended |
| `src/trace/emitter.rs` | Refactor `TraceEmitter` into `ObserverSet`; keep thin wrapper for backward compat if needed |
| `src/trace/mod.rs` | Update re-exports: add `ObserverSet`, `Observer`; update `init_trace` to return `ObserverSet` |
| `src/trace/writer.rs` | Keep `TraceWriter::spawn` unchanged (background thread) |
| `src/trace/replay_buffer.rs` | Add `file_timeline`, `events_between`, `error_summary`, `to_structured_json` |
| `src/agent.rs` | Replace `trace_emitter: Option<TraceEmitter>` with `observer_set: ObserverSet`; add new emit calls at integration points |
| `src/bg_observer.rs` | Add `Observer` impl; keep `emit_external` for AgentLoop-external events |
| `src/background.rs` | Update `drain_bg_events` to use ObserverSet; remove manual BgObserver emit where AgentLoop handles it |
| `src/visual/event_router.rs` | Add `TracePoint` and `TraceSpan` variants to `ServerEvent` |
| `src/visual/http_server.rs` | Add `/api/v1/trace/stream` and `/api/v1/trace/touches` endpoints |
| `src/daemon/proto.rs` | Optionally add trace-related `ServerEvent` variants |
| `src/lib.rs` | Update module exports if needed |
| `tests/trace_integration.rs` | Update tests for new ObserverSet init |

---

## Phase 1: EventKind Expansion + ObserverSet Architecture

### Task 1.1: Expand EventKind with new variants

**Files:**
- Modify: `src/trace/types.rs:1-137`
- Test: inline in `src/trace/types.rs` (or separate test module)

**Interfaces:**
- Consumes: None (additive change)
- Produces: Extended `EventKind` enum with all new variants, `SubAgentStatus` enum, updated `TouchType` with `file_size`/`content_hash`/`language` fields?

- [ ] **Step 1: Add new supporting types**

```rust
// Add to src/trace/types.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Spawned,
    Running,
    Done,
    Failed,
}

// Extend TouchType
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchType {
    Hit,
    Read,
    Edit,
    Create,
    Delete,
}
```

- [ ] **Step 2: Add new EventKind variants to the existing enum**

```rust
// Add to EventKind enum (existing variants remain unchanged)

// === New variants ===
UserInput {
    source: MessageSource,   // Manual / Auto / Injected
    length: usize,
    preview: String,
},
ToolCallBegin {
    name: String,
    args: serde_json::Value,
},
ToolCallEnd {
    name: String,
    is_error: bool,
    output_size: usize,
    duration_ms: u64,
    output_preview: String,
},
SubAgentLifecycle {
    agent_id: String,
    status: SubAgentStatus,
    parent_span_id: String,
},
ContextSnapshot {
    before_bytes: u64,
    after_bytes: u64,
    dropped_events: usize,
},
PermissionFull {
    key: String,
    decision: PermissionDecision,
    tool: String,
    headless: bool,
},
MilestoneStatus {
    id: u64,
    title: String,
    old_status: String,
    new_status: String,
},
RetryEvent {
    kind: String,       // "llm" | "tool"
    attempt: u32,
    max_retries: u32,
    error: String,
},
ProcessIdentity {
    pid: u32,
    agent_type: String,
    session_id: String,
},
```

- [ ] **Step 3: Run tests to verify existing serialization still works**

Run: `cargo test trace::types -v`
Expected: All existing tests pass

- [ ] **Step 4: Commit**

```bash
git add src/trace/types.rs
git commit -m "feat(trace): expand EventKind with new variants for unified observability"
```

### Task 1.2: Create Observer trait + ObserverSet

**Files:**
- Create: `src/trace/observer_set.rs`
- Modify: `src/trace/mod.rs` (add `pub mod observer_set`, re-exports)
- Test: inline in `src/trace/observer_set.rs`

**Interfaces:**
- Consumes: `src/trace/types.rs`'s `SpanStart`, `SpanEnd`, `PointEvent`, `TraceEvent`
- Produces: `Observer` trait, `ObserverSet` struct with `register()`, `on_span_start()`, `on_span_end()`, `on_point()`, `flush()`, and convenience methods matching `TraceEmitter`'s API

- [ ] **Step 1: Write the failing test**

```rust
// In src/trace/observer_set.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::types::*;

    #[test]
    fn observer_set_dispatches_to_registered_observers() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx: tx.clone() }));
        let span = SpanStart {
            span_id: "sp_001".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_span_start(&span);
        let received = rx.recv().unwrap();
        assert_eq!(received, "span_start:sp_001");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test observer_set::tests::observer_set_dispatches_to_registered_observers -v`
Expected: FAIL with "module not found" or "struct not defined"

- [ ] **Step 3: Write minimal Observer trait + ObserverSet**

```rust
// src/trace/observer_set.rs

use crate::trace::types::*;

/// Observer trait — each implementation handles one consumer's subset of events.
pub trait Observer: Send {
    fn enabled(&self) -> bool { true }
    fn on_span_start(&mut self, _span: &SpanStart) {}
    fn on_span_end(&mut self, _span: &SpanEnd) {}
    fn on_point(&mut self, _event: &PointEvent) {}
    fn flush(&mut self) {}
}

/// ObserverSet — holds all registered observers, dispatches events to each.
pub struct ObserverSet {
    observers: Vec<Box<dyn Observer + Send>>,
    soft_fail: bool,
}

impl ObserverSet {
    pub fn new() -> Self {
        ObserverSet { observers: Vec::new(), soft_fail: true }
    }

    pub fn register(&mut self, observer: Box<dyn Observer + Send>) {
        self.observers.push(observer);
    }

    pub fn on_span_start(&mut self, span: &SpanStart) {
        for obs in &mut self.observers {
            if obs.enabled() {
                obs.on_span_start(span);
            }
        }
    }

    pub fn on_span_end(&mut self, span: &SpanEnd) {
        for obs in &mut self.observers {
            if obs.enabled() {
                obs.on_span_end(span);
            }
        }
    }

    pub fn on_point(&mut self, event: &PointEvent) {
        for obs in &mut self.observers {
            if obs.enabled() {
                obs.on_point(event);
            }
        }
    }

    pub fn flush(&mut self) {
        for obs in &mut self.observers {
            if obs.enabled() {
                obs.flush();
            }
        }
    }

    // Convenience methods matching existing TraceEmitter API
    pub fn on_turn_start(&mut self) -> Option<String> {
        let span_id = crate::trace::types::span_id("sess", 0); // caller overrides
        let span = SpanStart {
            span_id: span_id.clone(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: crate::trace::types::now_ts(),
            meta: serde_json::json!({}),
        };
        self.on_span_start(&span);
        Some(span_id)
    }

    pub fn on_llm_call_start(&mut self, model: &str, prompt_tokens: u32, prompt_preview: &str) -> Option<String> {
        let span_id = format!("llm_{}", crate::trace::types::now_ts());
        let span = SpanStart {
            span_id: span_id.clone(),
            parent_id: None,
            kind: SpanKind::LlmCall,
            ts: crate::trace::types::now_ts(),
            meta: serde_json::json!({
                "model": model,
                "prompt_tokens": prompt_tokens,
                "prompt_preview": prompt_preview,
            }),
        };
        self.on_span_start(&span);
        Some(span_id)
    }

    pub fn on_llm_call_end(&mut self, span_id: &str, completion_tokens: u32, stop_reason: &str, duration_ms: u64) {
        let span = SpanEnd {
            span_id: span_id.into(),
            ts: crate::trace::types::now_ts(),
            meta: serde_json::json!({
                "completion_tokens": completion_tokens,
                "stop_reason": stop_reason,
                "duration_ms": duration_ms,
            }),
        };
        self.on_span_end(&span);
    }

    pub fn on_tool_start(&mut self, name: &str, input_preview: &str, full_input: Option<&str>) -> Option<String> {
        let span_id = format!("tool_{}", crate::trace::types::now_ts());
        let preview = full_input.unwrap_or(input_preview);
        let span = SpanStart {
            span_id: span_id.clone(),
            parent_id: None,
            kind: SpanKind::ToolCall,
            ts: crate::trace::types::now_ts(),
            meta: serde_json::json!({
                "tool": name,
                "input_preview": preview,
            }),
        };
        self.on_span_start(&span);
        Some(span_id)
    }

    pub fn on_tool_end(&mut self, span_id: &str, is_error: bool, output_preview: &str, target_files: &[String]) {
        let span = SpanEnd {
            span_id: span_id.into(),
            ts: crate::trace::types::now_ts(),
            meta: serde_json::json!({
                "is_error": is_error,
                "output_preview": output_preview,
                "target_files": target_files,
            }),
        };
        self.on_span_end(&span);
    }
}

// TestObserver for testing
#[cfg(test)]
struct TestObserver {
    tx: std::sync::mpsc::Sender<String>,
}

#[cfg(test)]
impl Observer for TestObserver {
    fn on_span_start(&mut self, span: &SpanStart) {
        let _ = self.tx.send(format!("span_start:{}", span.span_id));
    }
}

#[cfg(test)]
impl Default for ObserverSet {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test observer_set::tests -v`
Expected: PASS

- [ ] **Step 5: Update src/trace/mod.rs**

```rust
// Add to src/trace/mod.rs
pub mod observer_set;
pub use observer_set::{Observer, ObserverSet};
```

- [ ] **Step 6: Commit**

```bash
git add src/trace/observer_set.rs src/trace/mod.rs
git commit -m "feat(trace): add Observer trait and ObserverSet for multi-observer dispatch"
```

### Task 1.3: Create TraceWriterObserver

**Files:**
- Create: `src/trace/observers/trace_writer.rs`
- Create: `src/trace/observers/mod.rs`
- Modify: `src/trace/mod.rs` (add `pub mod observers`)
- Test: inline in `src/trace/observers/trace_writer.rs`

**Interfaces:**
- Consumes: `Observer` trait, `TraceWriter::spawn(root) -> Sender<TraceEvent>`
- Produces: `TraceWriterObserver` implements `Observer`, wraps existing `TraceWriter` channel

- [ ] **Step 1: Write the test**

```rust
// In src/trace/observers/trace_writer.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::types::*;
    use tempfile::tempdir;

    #[test]
    fn trace_writer_observer_creates_file() {
        let dir = tempdir().unwrap();
        let tx = crate::trace::writer::TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, false);
        let span = SpanStart {
            span_id: "sp_test".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 1.0,
            meta: serde_json::json!({}),
        };
        obs.on_span_start(&span);
        obs.flush();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let path = dir.path().join(".ccd.trace.ndjson");
        assert!(path.exists());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test trace_writer::tests -v`
Expected: FAIL

- [ ] **Step 3: Implement TraceWriterObserver**

```rust
// src/trace/observers/trace_writer.rs

use crate::trace::types::*;
use crate::trace::observer_set::Observer;
use std::sync::mpsc::Sender;

pub struct TraceWriterObserver {
    tx: Sender<TraceEvent>,
    trace_full: bool,
    emit_stream_delta: bool,
}

impl TraceWriterObserver {
    pub fn new(tx: Sender<TraceEvent>, trace_full: bool) -> Self {
        TraceWriterObserver { tx, trace_full, emit_stream_delta: false }
    }
}

impl Observer for TraceWriterObserver {
    fn on_span_start(&mut self, span: &SpanStart) {
        let _ = self.tx.send(TraceEvent::S(span.clone()));
    }

    fn on_span_end(&mut self, span: &SpanEnd) {
        let _ = self.tx.send(TraceEvent::E(span.clone()));
    }

    fn on_point(&mut self, event: &PointEvent) {
        let _ = self.tx.send(TraceEvent::P(event.clone()));
    }
}
```

- [ ] **Step 4: Create src/trace/observers/mod.rs**

```rust
pub mod trace_writer;
pub use trace_writer::TraceWriterObserver;
```

- [ ] **Step 5: Update src/trace/mod.rs**

```rust
pub mod observers;
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test trace_writer::tests -v`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/trace/observers/
git commit -m "feat(trace): add TraceWriterObserver wrapping existing TraceWriter channel"
```

### Task 1.4: Create ReplayBufferObserver

**Files:**
- Create: `src/trace/observers/replay_buffer.rs`
- Modify: `src/trace/observers/mod.rs`
- Test: inline in `src/trace/observers/replay_buffer.rs`

**Interfaces:**
- Consumes: `Observer` trait, `EventKind` from `types.rs`, `ReplayBuffer` from `replay_buffer.rs`
- Produces: `ReplayBufferObserver` implements `Observer`, translates `EventKind` → `ObservationKind` and pushes to ring buffer

- [ ] **Step 1: Write the test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::types::*;

    #[test]
    fn replay_buffer_observer_collects_events() {
        let mut obs = ReplayBufferObserver::new(100);
        let event = PointEvent {
            ts: 1.0,
            kind: EventKind::Notice { text: "hello".into() },
            meta: serde_json::json!({}),
        };
        obs.on_point(&event);
        assert_eq!(obs.len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Implement ReplayBufferObserver**

```rust
// src/trace/observers/replay_buffer.rs

use crate::trace::types::*;
use crate::trace::observer_set::Observer;
use crate::trace::replay_buffer::{ObservationEvent, ObservationKind, ReplayBuffer};

pub struct ReplayBufferObserver {
    buffer: ReplayBuffer,
    subscribe_mask: Option<Vec<&'static str>>, // None = subscribe to all
}

impl ReplayBufferObserver {
    pub fn new(capacity: usize) -> Self {
        ReplayBufferObserver {
            buffer: ReplayBuffer::new_with_capacity(capacity),
            subscribe_mask: None,
        }
    }

    pub fn len(&self) -> usize { self.buffer.len() }

    pub fn set_subscribe_mask(&mut self, kinds: Vec<&'static str>) {
        self.subscribe_mask = Some(kinds);
    }
}

impl Observer for ReplayBufferObserver {
    fn on_point(&mut self, event: &PointEvent) {
        let kind_str = format!("{:?}", event.kind);
        if let Some(ref mask) = self.subscribe_mask {
            if !mask.iter().any(|m| kind_str.starts_with(m)) {
                return;
            }
        }
        // Translate EventKind to ObservationKind
        let obs_kind = translate_to_observation(&event.kind);
        if let Some(ok) = obs_kind {
            self.buffer.push(ObservationEvent { ts: event.ts, kind: ok });
        }
    }
}

fn translate_to_observation(kind: &EventKind) -> Option<ObservationKind> {
    match kind {
        EventKind::Notice { text } => Some(ObservationKind::Notice { text: text.clone() }),
        EventKind::FileTouch { path, touch, .. } => {
            let touch_str = match touch {
                TouchType::Read | TouchType::Hit => "read",
                TouchType::Edit | TouchType::Create => "edit",
                TouchType::Delete => "delete",
            };
            Some(ObservationKind::FileTouch { path: path.clone(), touch: touch_str.into() })
        }
        EventKind::PermissionCheck { key, decision } => {
            Some(ObservationKind::Permission { key: key.clone(), granted: *decision == PermissionDecision::Granted || *decision == PermissionDecision::AutoGranted })
        }
        EventKind::PermissionFull { key, decision, .. } => {
            Some(ObservationKind::Permission { key: key.clone(), granted: *decision == PermissionDecision::Granted || *decision == PermissionDecision::AutoGranted })
        }
        EventKind::RetryEvent { kind, attempt, .. } => {
            Some(ObservationKind::Error { message: format!("retry {kind} attempt #{attempt}") })
        }
        EventKind::SubAgentLifecycle { agent_id, status, .. } => {
            let status_str = format!("{:?}", status);
            Some(ObservationKind::SubAgent { label: agent_id.clone(), status: status_str })
        }
        EventKind::CompactionDrop { dropped_bytes, .. } => {
            Some(ObservationKind::Compaction { dropped_bytes: *dropped_bytes })
        }
        EventKind::UserMessage { source: _, summary } => {
            Some(ObservationKind::UserMessage { summary: summary.clone() })
        }
        EventKind::UserInput { source: _, preview, .. } => {
            Some(ObservationKind::UserMessage { summary: preview.clone() })
        }
        EventKind::MilestoneStatus { id, title, old_status, new_status } => {
            Some(ObservationKind::Notice { text: format!("milestone #{id} ({title}): {old_status} → {new_status}") })
        }
        _ => None,
    }
}
```

- [ ] **Step 4: Update src/trace/observers/mod.rs**

```rust
pub mod replay_buffer;
pub use replay_buffer::ReplayBufferObserver;
```

- [ ] **Step 5: Run test to verify it passes**

- [ ] **Step 6: Commit**

```bash
git add src/trace/observers/replay_buffer.rs
git commit -m "feat(trace): add ReplayBufferObserver that translates EventKind to ObservationKind"
```

### Task 1.5: Create SSE Observer

**Files:**
- Create: `src/trace/observers/sse_observer.rs`
- Modify: `src/trace/observers/mod.rs`
- Test: inline in `src/trace/observers/sse_observer.rs`

**Interfaces:**
- Consumes: `Observer` trait, `EventKind` from `types.rs`
- Produces: `SseObserver` implements `Observer`, maintains file touch heatmap

- [ ] **Step 1: Write the test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::types::*;

    #[test]
    fn sse_observer_tracks_touches() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 1.0, kind: EventKind::FileTouch { path: "src/main.rs".into(), touch: TouchType::Read, lines: None },
            meta: serde_json::json!({}),
        });
        obs.on_point(&PointEvent {
            ts: 2.0, kind: EventKind::FileTouch { path: "src/main.rs".into(), touch: TouchType::Edit, lines: None },
            meta: serde_json::json!({}),
        });
        let heatmap = obs.heatmap();
        assert_eq!(heatmap.len(), 1);
        assert_eq!(heatmap["src/main.rs"].reads, 1);
        assert_eq!(heatmap["src/main.rs"].edits, 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Implement SseObserver**

```rust
// src/trace/observers/sse_observer.rs

use std::collections::HashMap;
use crate::trace::types::*;
use crate::trace::observer_set::Observer;

#[derive(Debug, Clone, Default, Serialize)]
pub struct FileTouchStats {
    pub reads: u64,
    pub edits: u64,
    pub hits: u64,
    pub last_touch_ts: f64,
    pub last_touch: String,
}

pub struct SseObserver {
    touches: HashMap<String, FileTouchStats>,
    // Channel to event_router (set during registration)
    router_tx: Option<std::sync::mpsc::Sender<()>>,
}

impl SseObserver {
    pub fn new() -> Self {
        SseObserver { touches: HashMap::new(), router_tx: None }
    }

    pub fn heatmap(&self) -> &HashMap<String, FileTouchStats> {
        &self.touches
    }
}

impl Observer for SseObserver {
    fn on_point(&mut self, event: &PointEvent) {
        if let EventKind::FileTouch { path, touch, .. } = &event.kind {
            let stats = self.touches.entry(path.clone()).or_default();
            match touch {
                TouchType::Read => { stats.reads += 1; }
                TouchType::Edit | TouchType::Create => { stats.edits += 1; }
                TouchType::Hit => { stats.hits += 1; }
                TouchType::Delete => {}
            }
            stats.last_touch_ts = event.ts;
            stats.last_touch = format!("{:?}", touch);
        }
    }
}
```

- [ ] **Step 4: Update src/trace/observers/mod.rs**

```rust
pub mod sse_observer;
pub use sse_observer::SseObserver;
```

- [ ] **Step 5: Run test to verify it passes**

- [ ] **Step 6: Commit**

```bash
git add src/trace/observers/sse_observer.rs
git commit -m "feat(trace): add SseObserver with file touch heatmap tracking"
```

---

## Phase 2: AgentLoop Integration

### Task 2.1: Replace trace_emitter with observer_set in AgentLoop

**Files:**
- Modify: `src/agent.rs` (multiple locations: struct field, build method, all usage sites)
- Test: existing tests in `src/agent.rs` + `tests/trace_integration.rs`

**Interfaces:**
- Consumes: `ObserverSet` from `trace::observer_set`, `TraceWriterObserver`, `ReplayBufferObserver`
- Produces: AgentLoop with `observer_set: ObserverSet` instead of `trace_emitter: Option<TraceEmitter>`

- [ ] **Step 1: Replace struct field in AgentLoop**

```rust
// In AgentLoop struct (src/agent.rs, line ~240)
// CHANGE:
//   trace_emitter: Option<crate::trace::TraceEmitter>,
// TO:
//   observer_set: crate::trace::observer_set::ObserverSet,
```

- [ ] **Step 2: Update build method**

```rust
// In AgentLoop::build (src/agent.rs, line ~374-408)
// CHANGE:
//   agent.trace_emitter = crate::trace::init_trace(&agent.root);
// TO:
//   let mut observer_set = crate::trace::observer_set::ObserverSet::new();
//   if let Some(tx) = crate::trace::writer::TraceWriter::spawn(&agent.root) {
//       let trace_full = std::env::var("CODECODER_TRACE_FULL")
//           .map(|v| v == "1" || v == "true")
//           .unwrap_or(false);
//       observer_set.register(Box::new(
//           crate::trace::observers::TraceWriterObserver::new(tx, trace_full)
//       ));
//   }
//   agent.observer_set = observer_set;
```

- [ ] **Step 3: Replace all `self.trace_emitter.as_mut().map(|t| t.on_turn_start())` calls**

There are 9 usage sites in `process_turn` and `dispatch_tool`. Replace each:

```rust
// BEFORE:
let turn_span = self.trace_emitter.as_mut().map(|t| t.on_turn_start());
// AFTER:
let turn_span = self.observer_set.on_turn_start();
```

The key change: `observer_set.on_*()` methods are no-ops if no observers are registered (ObserverSet handles this internally), so we don't need `Option<ObserverSet>` wrapping.

- [ ] **Step 4: Keep replay_buffer as a separate path (for now)**

In Phase 2, `replay_buffer` still exists as a separate field on `AgentLoop`. The `ReplayBufferObserver` added in Phase 1 is optional — AgentLoop can continue to push directly to `ReplayBuffer` as it does today. The ReplayBufferObserver migration happens in Phase 6.

- [ ] **Step 5: Update tests/trace_integration.rs**

The integration test calls `codecoder::trace::init_trace()` which no longer exists. Update to construct a `TraceWriterObserver` via the new path:

```rust
// Update test to use the new ObserverSet init
// The AgentLoop now registers TraceWriterObserver internally during build()
// when CODECODER_TRACE=1 is set — verify via the ObserverSet's enabled path
```

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add src/agent.rs tests/trace_integration.rs
git commit -m "refactor(agent): replace trace_emitter with ObserverSet in AgentLoop"
```

### Task 2.2: Add new event emissions at AgentLoop integration points

**Files:**
- Modify: `src/agent.rs` (add emit calls at each integration point)
- Test: existing tests + new assertions in agent tests

- [ ] **Step 1: Add UserInput emission at turn start**

```rust
// In process_turn, after self.append(Role::User, ...) (line ~871)
self.observer_set.on_point(&PointEvent {
    ts: crate::trace::types::now_ts(),
    kind: EventKind::UserInput {
        source: crate::trace::types::MessageSource::Manual,
        length: text.len(),
        preview: text.chars().take(200).collect(),
    },
    meta: serde_json::json!({}),
});
```

- [ ] **Step 2: Add RetryEvent emission in complete_retrying**

```rust
// In complete_retrying, before retry (line ~801)
self.observer_set.on_point(&PointEvent {
    ts: crate::trace::types::now_ts(),
    kind: EventKind::RetryEvent {
        kind: "llm".into(),
        attempt: attempt,
        max_retries: MAX_RETRIES,
        error: msg.clone(),
    },
    meta: serde_json::json!({}),
});
```

- [ ] **Step 3: Add PermissionFull emission in dispatch_tool**

After permission decision (lines ~1329-1360), emit the full context:

```rust
// After permission grant/deny decision
self.observer_set.on_point(&PointEvent {
    ts: crate::trace::types::now_ts(),
    kind: EventKind::PermissionFull {
        key: key.clone(),
        decision: /* resolved decision */,
        tool: name.to_string(),
        headless: self.headless,
    },
    meta: serde_json::json!({}),
});
```

- [ ] **Step 4: Add SubAgentLifecycle emissions in spawn_sub_agent_text**

```rust
// In spawn_sub_agent_text, before thread::spawn
self.observer_set.on_point(&PointEvent {
    ts: crate::trace::types::now_ts(),
    kind: EventKind::SubAgentLifecycle {
        agent_id: format!("sub_{}", std::process::id()),
        status: SubAgentStatus::Spawned,
        parent_span_id: String::new(),
    },
    meta: serde_json::json!({}),
});
// After thread::spawn (Running state)
// After handle.join() (Done or Failed state)
```

- [ ] **Step 5: Add ContextSnapshot emission in context_working_set**

```rust
// In context_working_set, after tier-1 compaction, before tier-2
self.observer_set.on_point(&PointEvent {
    ts: now_ts(),
    kind: EventKind::ContextSnapshot {
        before_bytes: before_size,
        after_bytes: after_size,
        dropped_events: dropped_count,
    },
    meta: serde_json::json!({}),
});
```

- [ ] **Step 6: Add MilestoneStatus emission in drive_workgraph**

```rust
// In drive_workgraph, after WorkGraph::set_status
self.observer_set.on_point(&PointEvent {
    ts: now_ts(),
    kind: EventKind::MilestoneStatus {
        id: milestone_id,
        title: title.clone(),
        old_status: "in_progress".into(),
        new_status: verdict_str.into(),
    },
    meta: serde_json::json!({}),
});
```

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
git add src/agent.rs
git commit -m "feat(agent): emit new event kinds at all AgentLoop integration points"
```

---

## Phase 3: Trace Data Model Enhancement

### Task 3.1: Update meta header to v2

**Files:**
- Modify: `src/trace/writer.rs:37-48` (meta header generation)
- Test: `tests/trace_integration.rs` (update assertions)

- [ ] **Step 1: Update meta header to include session_id, agent_type, model, git info**

```rust
// In TraceWriter::run, after initial_metadata_written check
let header = serde_json::json!({
    "type": "meta",
    "version": 2,
    "ts": crate::trace::types::now_ts(),
    "pid": std::process::id(),
    "session_id": "unknown",  // TODO: pass from ObserverSet
    "agent_type": "main",
    "harness": "codecoder",
    "model": "unknown",
    "root": self.ndjson_path.parent().map(|p| p.to_string_lossy()),
});
```

- [ ] **Step 2: Update integration test to accept version 2**

```rust
// In tests/trace_integration.rs
// assert_eq!(meta["version"], 1); → assert_eq!(meta["version"], 2);
```

- [ ] **Step 3: Run tests**

Run: `cargo test trace_integration -v`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/trace/writer.rs tests/trace_integration.rs
git commit -m "feat(trace): bump NDJSON meta header to v2 with richer metadata fields"
```

### Task 3.2: Add file_size and content_hash to FileTouch

**Files:**
- Modify: `src/trace/types.rs` (FileTouch struct)
- Modify: `src/trace/emitter.rs` (FileTouch emission points)
- Test: existing tests

- [ ] **Step 1: Add fields to FileTouch EventKind variant**

```rust
// Extend existing FileTouch variant
FileTouch {
    path: String,
    touch: TouchType,
    lines: Option<[u32; 2]>,
    file_size: Option<u64>,
    content_hash: Option<String>,
    language: Option<String>,
},
```

- [ ] **Step 2: Update all emission sites to populate new fields**

Check all places where `EventKind::FileTouch { ... }` is constructed and add the new optional fields.

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/trace/types.rs
git commit -m "feat(trace): add file_size, content_hash, language to FileTouch events"
```

---

## Phase 4: BgObserver Unification

### Task 4.1: Implement Observer trait for BgObserver

**Files:**
- Modify: `src/bg_observer.rs` (add `Observer` impl, add `emit_external`)
- Test: existing tests in `src/bg_observer.rs`

- [ ] **Step 1: Add Observer impl to BgObserver**

```rust
// In src/bg_observer.rs

use crate::trace::observer_set::Observer;
use crate::trace::types::*;

impl Observer for BgObserver {
    fn on_point(&mut self, event: &PointEvent) {
        match &event.kind {
            EventKind::ToolCallBegin { name, .. } => {
                self.emit("tool_started", name);
            }
            EventKind::ToolCallEnd { name, is_error, .. } => {
                if *is_error {
                    self.emit("tool_error", name);
                } else {
                    self.emit("tool_finished", name);
                }
            }
            EventKind::MilestoneStatus { id, title, old_status, new_status } => {
                self.emit("milestone", &format!("#{id} ({title}): {old_status} → {new_status}"));
            }
            EventKind::PermissionFull { key, decision, tool, headless } => {
                if matches!(decision, PermissionDecision::Denied) {
                    self.emit("denied", &format!("{tool}:{key}"));
                }
            }
            EventKind::RetryEvent { kind, attempt, .. } => {
                self.emit("retry", &format!("{kind} attempt #{attempt}"));
            }
            EventKind::Notice { text } => {
                self.emit("notice", text);
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 2: Add emit_external method**

```rust
impl BgObserver {
    /// For events emitted outside AgentLoop (run start/end, budget, etc.)
    pub fn emit_external(&mut self, kind: &str, msg: &str) {
        self.emit(kind, msg);
    }

    pub fn emit_external_with_data(&mut self, kind: &str, msg: &str, data: Option<serde_json::Value>) {
        self.emit_with_data(kind, msg, data);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test bg_observer -v`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/bg_observer.rs
git commit -m "feat(trace): implement Observer trait for BgObserver, add emit_external"
```

### Task 4.2: Update background.rs to use ObserverSet

**Files:**
- Modify: `src/background.rs` (simplify `drain_bg_events`, remove manual BgObserver emits handled by AgentLoop)
- Test: existing tests

- [ ] **Step 1: Register BgObserver in the AgentLoop's ObserverSet**

In `background.rs`, when creating the AgentLoop, register the BgObserver:

```rust
// In run_background_cfg, after creating agent
agent.observer_set.register(Box::new(
    crate::bg_observer::BgObserver::start_run(&root)
));
```

- [ ] **Step 2: Simplify drain_bg_events**

Remove the manual `obs.emit("tool_started", ...)` and `obs.emit("tool_error", ...)` calls since AgentLoop's ObserverSet now handles them. Keep only the BgOutcome accumulation logic.

- [ ] **Step 3: Keep external events (run start/end, budget) via emit_external**

```rust
// For external events
obs.emit_external("mission_state", &format!("{:?}", out.mission_state));
obs.emit_external("seed", "empty workgraph — attempting to seed from AGENTS.md...");
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/background.rs
git commit -m "refactor(background): unify BgObserver emission through ObserverSet, simplify drain_bg_events"
```

---

## Phase 5: Human Interface Enhancement

### Task 5.1: Add trace SSE endpoint to http_server

**Files:**
- Modify: `src/visual/http_server.rs` (add `/api/v1/trace/stream` and `/api/v1/trace/touches`)
- Test: manual testing with curl

- [ ] **Step 1: Add `/api/v1/trace/stream` SSE endpoint**

```rust
// In src/visual/http_server.rs, register new route
// GET /api/v1/trace/stream → SSE stream of TraceEvent JSON
// Uses TraceStream::follow to tail .ccd.trace.ndjson
```

- [ ] **Step 2: Add `/api/v1/trace/touches` endpoint**

```rust
// GET /api/v1/trace/touches → JSON file touch heatmap
// Reads from SseObserver's heatmap state
```

- [ ] **Step 3: Add trace-related ServerEvent variants**

```rust
// In src/daemon/proto.rs or src/visual/event_router.rs
// Add TracePoint and TraceSpan variants
pub enum ServerEvent {
    // ... existing ...
    TracePoint {
        span_id: String,
        kind: String,
        path: Option<String>,
        summary: String,
        ts: f64,
    },
    TraceSpan {
        span_id: String,
        parent_id: Option<String>,
        kind: String,
        meta: serde_json::Value,
        ts: f64,
    },
}
```

- [ ] **Step 4: Build check**

Run: `cargo build`
Expected: Build succeeds

- [ ] **Step 5: Commit**

```bash
git add src/visual/http_server.rs src/daemon/proto.rs
git commit -m "feat(visual): add trace SSE endpoint and touch heatmap API"
```

---

## Phase 6: LLM Self-Observation Enhancement

### Task 6.1: Add query methods to ReplayBuffer

**Files:**
- Modify: `src/trace/replay_buffer.rs` (add `file_timeline`, `events_between`, `error_summary`, `to_structured_json`)
- Test: inline in `src/trace/replay_buffer.rs`

- [ ] **Step 1: Add file_timeline method**

```rust
pub fn file_timeline(&self, path: &str) -> Vec<&ObservationEvent> {
    self.buffer.iter().filter(|e| matches!(&e.kind, ObservationKind::FileTouch { path: p, .. } if p == path)).collect()
}
```

- [ ] **Step 2: Add events_between method**

```rust
pub fn events_between(&self, start_ts: f64, end_ts: f64) -> Vec<&ObservationEvent> {
    self.buffer.iter().filter(|e| e.ts >= start_ts && e.ts <= end_ts).collect()
}
```

- [ ] **Step 3: Add error_summary method**

```rust
pub fn error_summary(&self) -> ErrorSummary {
    let mut summary = ErrorSummary::default();
    for event in &self.buffer {
        if let ObservationKind::Error { message } = &event.kind {
            summary.errors.push(message.clone());
        }
        if let ObservationKind::Compaction { dropped_bytes } = &event.kind {
            summary.dropped_bytes += dropped_bytes;
        }
    }
    summary
}
```

- [ ] **Step 4: Add to_structured_json method**

```rust
pub fn to_structured_json(&self) -> serde_json::Value {
    let mut files_read = Vec::new();
    let mut files_edited = Vec::new();
    let mut errors = Vec::new();
    let mut llm_calls = 0u32;
    let mut tools = 0u32;

    for event in &self.buffer {
        match &event.kind {
            ObservationKind::FileTouch { path, touch } => {
                if touch == "read" || touch == "hit" { files_read.push(path.clone()); }
                if touch == "edit" || touch == "create" { files_edited.push(path.clone()); }
            }
            ObservationKind::LlmCall { .. } => llm_calls += 1,
            ObservationKind::ToolCall { .. } => tools += 1,
            ObservationKind::Error { message } => errors.push(message.clone()),
            _ => {}
        }
    }

    files_read.sort(); files_read.dedup();
    files_edited.sort(); files_edited.dedup();

    serde_json::json!({
        "llm_calls": llm_calls,
        "tools": tools,
        "errors": errors.len(),
        "files_read": files_read,
        "files_edited": files_edited,
    })
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test replay_buffer -v`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/trace/replay_buffer.rs
git commit -m "feat(trace): add query methods and structured JSON output to ReplayBuffer"
```

### Task 6.2: Enhance self-observation output with retries, milestones, permissions

**Files:**
- Modify: `src/trace/replay_buffer.rs` (update `to_self_observation`)
- Test: inline in `src/trace/replay_buffer.rs`

- [ ] **Step 1: Update to_self_observation to include retries, milestones, permission details**

Add new sections to the existing output:

```rust
// After "### Tool Call Sequence" section
// Add "### Retries" section
// Add "### Permission Checks" section (expand from existing)
// Add "### Summary Stats" section at the end
```

- [ ] **Step 2: Run tests**

Run: `cargo test replay_buffer -v`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/trace/replay_buffer.rs
git commit -m "feat(trace): enhance self-observation output with retries, milestones, and summary stats"
```

---

## Spec Coverage Check

| Spec Section | Tasks |
|-------------|-------|
| Module 1: Event Type Expansion | 1.1 (all new EventKind variants) |
| Module 2: Observer Set | 1.2 (Observer trait + ObserverSet), 1.3 (TraceWriterObserver), 1.4 (ReplayBufferObserver), 1.5 (SseObserver) |
| Module 3: Trace Data Model | 3.1 (meta header v2), 3.2 (file details) |
| Module 4: AgentLoop Integration | 2.1 (replace trace_emitter), 2.2 (new emissions) |
| Module 5: BgObserver Unification | 4.1 (Observer impl), 4.2 (background.rs) |
| Module 6: Human Interface | 5.1 (SSE endpoints, ServerEvent variants) |
| Module 7: LLM Self-Observation | 6.1 (query methods), 6.2 (enhanced output) |