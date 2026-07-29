# Unified Observability System for CodeCoder

**Date**: 2026-07-29
**Status**: Design Approved
**Author**: glm-5.2

## Background

CodeCoder already has a trace observability system (`TraceEmitter` + `.ccd.trace.ndjson` + `ReplayBuffer`), a headless background observer (`BgObserver` + `.ccd.bg.ndjson`), and a visual event router (`visual/event_router` + SSE). These systems were built independently at different times, resulting in three parallel observation streams with overlapping concerns and no unified data model.

The goal is to create a single, complete, fine-grained observation system that captures every execution event — LLM calls, tool execution, file operations, permission checks, user input, sub-agent calls, compaction, errors, retries, workgraph milestones — and makes them available both for human observation (real-time SSE + offline analysis) and for LLM self-observation (ReplayBuffer).

This design is inspired by [mindwalk](https://github.com/cosmtrek/mindwalk)'s architecture: a normalized trace of events, a deterministic citymap of the codebase, and an evaluation report. CodeCoder's observability system will produce a trace that mindwalk can consume directly.

## Design Overview

Seven interlocking modules, implemented incrementally:

1. **Event Type Expansion** — fill all gaps in the `EventKind` enum
2. **Observer Set Architecture** — upgrade `TraceEmitter` to a multi-observer dispatch
3. **Trace Data Model Enhancement** — richer NDJSON schema with standard metadata
4. **AgentLoop Integration Points** — instrument every key execution point
5. **BgObserver Unification** — merge two parallel systems into one
6. **Human Interface Enhancement** — new trace SSE endpoints + touch heatmap
7. **LLM Self-Observation Enhancement** — richer ReplayBuffer + structured output

## Module 1: Event Type Expansion

### New EventKind Variants

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    // === Existing (unchanged) ===
    FileTouch { path: String, touch: TouchType, lines: Option<[u32; 2]> },
    PermissionCheck { key: String, decision: PermissionDecision },
    UserMessage { source: MessageSource, summary: String },
    Notice { text: String },
    StreamDelta { text: String },
    SubAgentResult { agent_id: String, summary: String },
    WorkgraphStatus { total: usize, pending: usize, done: usize, needs_fix: usize },
    ExitCode { code: i32 },
    AgentGraphEdge(AgentGraphEdge),
    LlmFullInput { model: String, messages: Vec<serde_json::Value> },
    LlmFullOutput { model: String, content: String },
    CompactionDrop { span_id: String, dropped_bytes: u64, summary: String },

    // === New ===
    /// User manual input (distinct from system-injected messages)
    UserInput {
        source: MessageSource,   // Manual / Auto / Injected
        length: usize,
        preview: String,
    },
    /// Tool call start (with full args)
    ToolCallBegin {
        name: String,
        args: serde_json::Value,
    },
    /// Tool call end (with full output)
    ToolCallEnd {
        name: String,
        is_error: bool,
        output_size: usize,
        duration_ms: u64,
        output_preview: String,
    },
    /// Sub-agent lifecycle events
    SubAgentLifecycle {
        agent_id: String,
        status: SubAgentStatus,   // Spawned / Running / Done / Failed
        parent_span_id: String,
    },
    /// Context snapshot before/after compaction
    ContextSnapshot {
        before_bytes: u64,
        after_bytes: u64,
        dropped_events: usize,
    },
    /// Permission full context
    PermissionFull {
        key: String,
        decision: PermissionDecision,
        tool: String,
        headless: bool,
    },
    /// Workgraph milestone status change
    MilestoneStatus {
        id: u64,
        title: String,
        old_status: String,
        new_status: String,
    },
    /// Retry event (LLM or tool)
    RetryEvent {
        kind: String,       // "llm" | "tool"
        attempt: u32,
        max_retries: u32,
        error: String,
    },
    /// Process/thread identity
    ProcessIdentity {
        pid: u32,
        agent_type: String,  // "main" | "sub" | "bg" | "review"
        session_id: String,
    },
}
```

### New Supporting Types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Spawned,
    Running,
    Done,
    Failed,
}
```

### Backward Compatibility

New variants are added to the existing `EventKind` enum. Consumers that match exhaustively will need updating, but the NDJSON format is forward-compatible: consumers that don't understand a new variant can skip it. The `#[serde(rename_all = "snake_case")]` tag ensures consistent serialization.

## Module 2: Observer Set Architecture

### Observer Trait

```rust
/// Observer trait — each implementation handles one consumer's subset of events.
pub trait Observer: Send {
    fn enabled(&self) -> bool { true }

    // Span lifecycle
    fn on_span_start(&mut self, _span: &SpanStart) {}
    fn on_span_end(&mut self, _span: &SpanEnd) {}

    // Point events (instantaneous)
    fn on_point(&mut self, _event: &PointEvent) {}

    // Batch flush called at end of each turn
    fn flush(&mut self) {}
}
```

### ObserverSet

```rust
/// ObserverSet — holds all registered observers, dispatches events to each.
pub struct ObserverSet {
    observers: Vec<Box<dyn Observer + Send>>,
    soft_fail: bool,
}

impl ObserverSet {
    pub fn new() -> Self;

    /// Register an observer. Order is deterministic: dispatch order = registration order.
    pub fn register(&mut self, observer: Box<dyn Observer + Send>);

    /// Dispatch a span start to all enabled observers.
    pub fn on_span_start(&mut self, span: &SpanStart);

    /// Dispatch a span end to all enabled observers.
    pub fn on_span_end(&mut self, span: &SpanEnd);

    /// Dispatch a point event to all enabled observers.
    pub fn on_point(&mut self, event: &PointEvent);

    /// Flush all observers (called at end of each turn).
    pub fn flush(&mut self);

    // Convenience methods matching existing AgentLoop patterns
    pub fn on_turn_start(&mut self) -> Option<String>;
    pub fn on_llm_call_start(&mut self, ...) -> Option<String>;
    pub fn on_tool_start(&mut self, ...) -> Option<String>;
    pub fn on_tool_end(&mut self, span_id: &str, ...);
}
```

### Built-in Observers

| Observer | Events Subscribed | Output | Consumer |
|----------|------------------|--------|----------|
| `TraceWriterObserver` | All events | `.ccd.trace.ndjson` (full) | Humans, offline analysis, mindwalk |
| `ReplayBufferObserver` | Summary subset | Ring buffer | LLM self-observation |
| `SseObserver` | Real-time subset | `visual/event_router` | Web UI SSE |
| `BgObserver` | Headless subset | `.ccd.bg.ndjson` + stderr | Headless run monitoring |

### TraceWriterObserver

```rust
pub struct TraceWriterObserver {
    /// Channel to the background writer thread (existing TraceWriter::spawn)
    tx: Sender<TraceEvent>,
    /// Whether to include full LLM input/output (CODECODER_TRACE_FULL gate)
    trace_full: bool,
    /// Whether to emit stream delta events (usually off)
    emit_stream_delta: bool,
}
```

### ReplayBufferObserver

```rust
pub struct ReplayBufferObserver {
    buffer: ReplayBuffer,
    subscribe_mask: EventMask,
    max_events: usize,
}

impl Observer for ReplayBufferObserver {
    fn on_point(&mut self, event: &PointEvent) {
        if !self.subscribe_mask.matches(&event.kind) { return; }
        self.buffer.push(ObservationEvent {
            ts: event.ts,
            kind: translate_event_kind(&event.kind),
        });
    }
}
```

### Integration with AgentLoop

Current:
```rust
// In AgentLoop::build:
self.trace_emitter = crate::trace::init_trace(&agent.root);
// Usage throughout:
self.trace_emitter.as_mut().map(|t| t.on_turn_start());
```

New:
```rust
// In AgentLoop::build:
self.observer_set = ObserverSet::new();
if let Some(tx) = TraceWriter::spawn(&agent.root) {
    self.observer_set.register(Box::new(TraceWriterObserver::new(tx)));
}
if let Some(rb) = &self.replay_buffer {
    self.observer_set.register(Box::new(ReplayBufferObserver::new(rb)));
}
// Usage:
self.observer_set.on_turn_start();
```

## Module 3: Trace Data Model Enhancement

### Meta Header (v2)

```json
{
  "type": "meta",
  "version": 2,
  "ts": 1712345678.123,
  "pid": 12345,
  "session_id": "session-1712345678000",
  "agent_type": "main",
  "harness": "codecoder",
  "model": "gpt-4o",
  "root": "/Users/me/project",
  "git_commit": "abc123def",
  "git_dirty": false
}
```

### Span Standard Meta Fields

| Field | Description | Always Present |
|-------|-------------|---------------|
| `turn_seq` | Turn sequence number within session | For Turn spans |
| `user_message` | The user message that triggered this turn | For Turn spans |
| `message_source` | `manual`, `auto`, `injected` | For Turn spans |
| `model` | Model name | For LlmCall spans |
| `prompt_tokens` | Input token count | For LlmCall spans |
| `completion_tokens` | Output token count | For LlmCall end |
| `stop_reason` | `stop`, `tool_use`, `length` | For LlmCall end |
| `duration_ms` | Execution duration | For all spans |
| `retry_attempt` | Which attempt (0 = first) | For LlmCall spans |
| `tool_calls` | Names of tools called | For LlmCall end |
| `is_error` | Whether the operation failed | For ToolCall end |
| `output_size` | Result size in bytes | For ToolCall end |
| `target_files` | Files touched by the tool | For ToolCall end |

### Point Event Standard Meta

```json
{
  "type": "p",
  "ts": 1712345680.000,
  "kind": { "file_touch": { "path": "src/main.rs", "touch": "read", "lines": [10, 50] } },
  "meta": {
    "span_id": "sp_sess_abc_0003",
    "category": "file_io",
    "severity": "info"
  }
}
```

### FileTouch Enhancement

```json
{
  "kind": {
    "file_touch": {
      "path": "src/main.rs",
      "touch": "edit",
      "lines": [10, 50],
      "file_size": 12345,
      "content_hash": "sha256:abc123...",
      "language": "rust"
    }
  }
}
```

### mindwalk Trace Mapping

| mindwalk field | CodeCoder source |
|----------------|-----------------|
| `event.seq` | Sequential emit order from NDJSON |
| `event.tool` | `ToolCallBegin.name` or span kind |
| `event.action` | Derived from `FileTouch.touch` |
| `event.targets` | Aggregated from `FileTouch` events |
| `event.isError` | `ToolCallEnd.is_error` |
| `event.summary` | Extracted from span meta |
| `mark.type` | `UserInput`, `CompactionDrop`, `SubAgentLifecycle` |
| `stats.fovea` | Count of distinct files read |
| `stats.edited` | Count of distinct files edited |

## Module 4: AgentLoop Integration Points

### Turn Lifecycle

| Code Location | Event | Status |
|---------------|-------|--------|
| `process_turn` entry | `SpanStart { kind: Turn }` | ✅ Existing |
| After `self.append(Role::User, ...)` | `UserInput { source: Manual, length, preview }` | ❌ New |
| `process_turn` normal exit | `SpanEnd { kind: Turn }` | ✅ Existing |
| `process_turn` cancelled | `Notice { text: "cancelled" }` | ✅ Existing |

### LLM Call Lifecycle

| Code Location | Event | Status |
|---------------|-------|--------|
| Before `complete_retrying` | `SpanStart { kind: LlmCall }` | ✅ Existing |
| Before `complete_retrying` | `LlmFullInput` (gated by CODECODER_TRACE_FULL) | ✅ Existing |
| Before retry | `RetryEvent { kind: "llm", attempt, max_retries, error }` | ❌ New |
| After LLM response | `SpanEnd { kind: LlmCall }` | ✅ Existing |
| After LLM response | `LlmFullOutput` (gated) | ✅ Existing |

### Tool Execution Lifecycle

| Code Location | Event | Status |
|---------------|-------|--------|
| `dispatch_tool` entry | `SpanStart { kind: ToolCall, meta: { full_args } }` | ✅ Span exists, need full args |
| Before `toolbox.get(name).unwrap().run(...)` | `FileTouch` for known file tools | ❌ New |
| After tool run | `SpanEnd { kind: ToolCall }` | ✅ Existing |
| `spawn_sub_agent` | `SubAgentLifecycle { Spawned }` → `Running` → `Done/Failed` | ❌ New |
| Headless auto-deny | `PermissionFull { Denied, headless: true }` | ❌ New |

### Compaction Lifecycle

| Code Location | Event | Status |
|---------------|-------|--------|
| tier-1 compaction | `CompactionDrop` | ✅ Existing |
| tier-1 → tier-2 transition | `ContextSnapshot { before, after }` | ❌ New |
| tier-2 failure → fallback | `Notice` | ❌ New |

### Permission Lifecycle

| Code Location | Event | Status |
|---------------|-------|--------|
| User grants permission | `PermissionCheck { Granted }` | ❌ New |
| User denies | `PermissionCheck { Denied }` | ❌ New |
| After user reply | `PermissionFull` with full context | ❌ New |

### Workgraph Lifecycle

| Code Location | Event | Status |
|---------------|-------|--------|
| Milestone status change | `MilestoneStatus { id, title, old, new }` | ❌ New |
| Auto-advance start | `WorkgraphStatus` | ✅ Existing |
| Auto-advance complete | `WorkgraphStatus` | ✅ Existing |

## Module 5: BgObserver Unification

### Current State

Two independent systems write separate NDJSON files:

```
BgObserver → .ccd.bg.ndjson (events: {kind, msg, ...})
TraceEmitter → .ccd.trace.ndjson (events: {type: s|e|p, ...})
```

### Unified State

Both systems derive from the same ObserverSet:

```
AgentLoop
  └─ ObserverSet.dispatch(event)
       ├─ TraceWriterObserver → .ccd.trace.ndjson (full)
       ├─ BgObserver → .ccd.bg.ndjson + stderr (summary)
       ├─ ReplayBufferObserver → ring buffer (LLM)
       └─ SseObserver → visual/event_router (SSE)
```

### BgObserver Output Format (unchanged for backward compat)

```json
{"kind":"tool_started","msg":"read_file","ts":1712345678.123,"args":"path=src/main.rs"}
{"kind":"tool_finished","msg":"read_file","ts":1712345678.200,"is_error":false,"duration_ms":77}
{"kind":"milestone","msg":"#3: 实现登录功能","ts":1712345700.000,"old":"pending","new":"done"}
{"kind":"llm_call","msg":"gpt-4o","ts":1712345680.000,"prompt_tokens":4500}
{"kind":"denied","msg":"run_command:npm install","ts":1712345690.000,"tool":"Bash","headless":true}
{"kind":"retry","msg":"llm call attempt 1/3","ts":1712345681.500,"error":"rate limit exceeded"}
```

### External Events

BgObserver retains a lightweight `emit_external()` method for events outside AgentLoop (e.g., `background.rs` run start/end, budget exhaustion):

```rust
impl BgObserver {
    /// Events emitted outside AgentLoop (run start/end, budget, etc.)
    pub fn emit_external(&mut self, kind: &str, msg: &str, extra: Option<Value>);
}
```

### Migration Path

1. Phase 1: BgObserver keeps independent; register a new thin BgObserver in ObserverSet
2. Phase 2: BgObserver switches to ObserverSet for AgentLoop events; `emit_external` for others
3. Phase 3: Remove standalone BgObserver; fully via ObserverSet

## Module 6: Human Interface Enhancement

### New Trace SSE Endpoints

```
GET /api/v1/trace/stream
  → SSE stream of raw TraceEvent JSON
  → Supports Last-Event-Id for catch-up
  → Connected via SseObserver

GET /api/v1/trace/snapshot
  → Full trace snapshot from .ccd.trace.ndjson

GET /api/v1/trace/events?since=<ts>&limit=N
  → Windowed event list

GET /api/v1/trace/touches
  → File touch heatmap
  {files: [{path: "src/main.rs", reads: 5, edits: 2, last_touch: "edit", ts: ...}]}
```

### Existing ServerEvent Enhancement

Add trace-related variants to `ServerEvent` for backward-compatible consumption:

```rust
pub enum ServerEvent {
    // ... existing variants unchanged ...

    // New
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

### File Touch Heatmap (SseObserver)

The `SseObserver` maintains a `HashMap<String, FileTouchAccumulator>` that is updated on each `FileTouch` event. The heatmap is periodically pushed via SSE, keeping the UI updated without polling.

## Module 7: LLM Self-Observation Enhancement

### Current ReplayBuffer Coverage

```rust
// Already covered
TurnStart, LlmCall, LlmEnd, ToolCall, ToolEnd,
FileTouch, Permission, Error, SubAgent, Compaction,
UserMessage, Notice
```

### New Coverage (from ObserverSet)

```rust
// New
RetryEvent { attempt, max_retries, error }
MilestoneStatus { id, title, old, new }
PermissionFull { key, decision, tool }
SubAgentLifecycle { agent_id, status }
ContextSnapshot { before, after }
```

### Enhanced Self-Observation Output

```markdown
## Previous Turn Trace (5.2s, 3 LLM calls, 4 tools, 0 errors)

### Tool Call Sequence
  1. read_file: path=src/main.rs → Success
  2. grep: pattern="fn foo" path=src/ → Success

### File Touches
  src/main.rs: [read, 5 hits]
  src/lib.rs: [edit, lines 10-42]

### Retries
  llm call #2: retried 1x (rate limit, 200ms backoff)

### Token Usage
  Model: gpt-4o | 4,500 prompt + 1,200 completion = 5,700 total

### Summary Stats
  - Files read: 3, Files edited: 1
  - Errors: 0, Retries: 1
  - Sub-agents spawned: 0
  - Permission denials: 1 (run_command:npm install)
```

### Structured JSON Output

New method `to_structured_json()` returns a machine-readable version:

```json
{
  "duration_ms": 5200,
  "llm_calls": 3,
  "tools": 4,
  "errors": 0,
  "retries": 1,
  "files_read": ["src/main.rs", "src/lib.rs"],
  "files_edited": ["src/lib.rs"],
  "permissions": [{"key": "run_command:npm install", "granted": false}],
  "subagents": []
}
```

### Query Interface

```rust
impl ReplayBuffer {
    pub fn file_timeline(&self, path: &str) -> Vec<FileTouchEvent>;
    pub fn events_between(&self, start_ts: f64, end_ts: f64) -> Vec<&ObservationEvent>;
    pub fn error_summary(&self) -> ErrorSummary;
    pub fn to_structured_json(&self) -> serde_json::Value;
}
```

### Injection Strategy

- **Per-turn injection** (existing): self-observation from previous turn injected as System message in `context_working_set`
- **On-demand query** (new): LLM can call `reason` tool to request specific trace details
- **Verbosity control** (new): `CODECODER_SELF_OBSERVE_VERBOSE=1` controls detail level (concise/standard/verbose)

## Implementation Order

| Phase | Modules | Effort | Dependencies |
|-------|---------|--------|-------------|
| 1 | 1 (EventKind expansion) + 2 (ObserverSet) | Medium | None |
| 2 | 4 (AgentLoop integration) | Medium | Phase 1 |
| 3 | 3 (Trace data model) | Small | Phase 1 |
| 4 | 5 (BgObserver unification) | Medium | Phase 1, 2 |
| 5 | 6 (Human interface) | Small | Phase 2, 3 |
| 6 | 7 (LLM self-observation) | Small | Phase 1, 2 |

Phase 1 is the critical foundation: once ObserverSet is in place and EventKind covers all types, every subsequent phase is wiring rather than structural change.

## Key Design Decisions

1. **Single event source, multiple consumers**: ObserverSet dispatches once; each observer decides what to consume. This guarantees consistency across all outputs.

2. **Backward compatibility**: Existing NDJSON format `TraceEvent { S/E/P }` is extended (not replaced). New fields are optional. Existing consumers continue to work.

3. **BgObserver format preserved**: The `.ccd.bg.ndjson` format stays human-readable. It's now derived from structured events rather than manually emitted.

4. **No pull-based Event Store**: Rejected in favor of the lighter ObserverSet pattern. The existing push-based architecture (`event_tx`) is kept.

5. **mindwalk compatibility**: The enhanced trace NDJSON can be converted to mindwalk's `Trace` schema with a simple adapter (no complex parsing).