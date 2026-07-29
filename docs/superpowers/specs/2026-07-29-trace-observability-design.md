# Trace Observability System — Design Spec

**Date:** 2026-07-29
**Status:** Draft
**Priority:** B (Post-hoc replay) → C (LLM self-observation) → D (Debugging) → A (Human real-time)

## Overview

Design a unified, structured event stream — **Trace** — emitted directly from the codecoder agent kernel, recording every relevant execution step in an append-only NDJSON file. This trace serves as the single source of truth for post-hoc replay, LLM self-observation, debugging, and (eventually) real-time monitoring.

The design is **purely internal to codecoder** — there is no integration with mindwalk or any external tool. mindwalk's model (Event stream, file touches, spans) was used as reference only.

## Core Concepts

### Span

A **Span** represents an operation with a clear start and end:
- `Turn` — one `process_turn` iteration
- `LlmCall` — one LLM chat completion request/response
- `ToolCall` — one tool invocation (read_file, write_file, run_command, etc.)
- `SubAgent` — lifecycle of a sub-agent spawned via the `agent` tool
- `Milestone` — execution of one workgraph milestone
- `Reasoning` — LLM reasoning text (model chain-of-thought)
- `Compaction` — context compaction operation

### Point Event

A **Point Event** represents an instantaneous occurrence with no duration:
- `FileTouch` — a file was touched (read, edited, created, deleted, or hit by search)
- `PermissionCheck` — a permission check result
- `UserMessage` — a user message arrived (with source classification)
- `Notice` — a system notice
- `StreamDelta` — streaming text delta (optional, off by default)
- `SubAgentResult` — sub-agent result summary
- `WorkgraphStatus` — workgraph state change
- `ExitCode` — headless exit code

### Span Tree

Spans form a tree through `parent_id`. A `Turn` span is the root; every LLM call, tool call, and sub-agent within that turn is a child. This allows LLM self-observation to navigate the execution structure at any granularity.

## Data Model

### Types

```rust
// src/trace/types.rs

pub enum SpanKind {
    Turn,
    LlmCall,
    ToolCall,
    SubAgent,
    Milestone,
    Reasoning,
    Compaction,
}

pub enum TouchType {
    Hit,    // weak evidence: grep/glob matched
    Read,
    Edit,
    Create,
    Delete,
}

pub enum MessageSource {
    Manual,   // user typed it
    Auto,     // auto-injected (headless, follow-up)
    Injected, // harness-injected (<system-reminder> etc.)
}

pub enum PermissionDecision {
    Granted,
    Denied,
    Cancelled,
    AutoGranted,  // pre-authorized via codecoder.json allowlist
}

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
```

### Wire Format

NDJSON file `.ccd.trace.ndjson` in the project root directory. Each line is a complete JSON object:

```json
{"type":"s","span_id":"sp_abc_0001","parent_id":null,"kind":"turn","ts":1712345678.001,"meta":{}}
{"type":"s","span_id":"sp_abc_0002","parent_id":"sp_abc_0001","kind":"llm_call","ts":1712345678.002,"meta":{"model":"gpt-4o","prompt_tokens":1500}}
{"type":"e","span_id":"sp_abc_0002","ts":1712345679.124,"meta":{"completion_tokens":450,"duration_ms":1122,"stop_reason":"tool_calls"}}
{"type":"p","kind":"permission_check","ts":1712345679.126,"meta":{"key":"read_file","decision":"auto_granted"}}
{"type":"s","span_id":"sp_abc_0003","parent_id":"sp_abc_0001","kind":"tool_call","ts":1712345679.127,"meta":{"tool":"read_file","input_preview":"file_path: src/main.rs"}}
{"type":"e","span_id":"sp_abc_0003","ts":1712345679.200,"meta":{"is_error":false,"output_preview":"fn main() { ... }","target_files":["src/main.rs"],"duration_ms":73}}
{"type":"e","span_id":"sp_abc_0001","ts":1712345679.400,"meta":{"duration_ms":1399}}
```

- `"s"` = span_start
- `"e"` = span_end
- `"p"` = point_event

The file starts with a meta header line:
```json
{"type":"meta","version":1,"ts":1712345678.000,"pid":12345}
```

### span_id Generation

Format: `sp_{session_id_short}_{counter}`

- `session_id_short` = first 12 hex chars of the root session ID
- `counter` = 4-digit zero-padded counter (0001, 0002, ...)

This ensures uniqueness across sessions and even across concatenated trace files.

## File Rotation

Same pattern as `BgObserver`:

- Rotate at 10 MB
- Keep 3 rotated files: `.ccd.trace.1.ndjson`, `.ccd.trace.2.ndjson`, `.ccd.trace.3.ndjson`
- Current file: `.ccd.trace.ndjson`

## Components

### 1. `TraceEmitter` (`src/trace/emitter.rs`)

Embedded in `AgentLoop`. Provides:
- `span_start(kind, meta)` — returns span_id, auto-pushes to span stack
- `span_end(span_id, meta)` — pauses, auto-injects duration_ms
- `emit(kind, meta)` — emits a point event
- `on_agent_event(&AgentEvent)` — translates AgentEvent variants to trace events
- Convenience wrappers: `on_turn_start()`, `on_llm_call_start/end()`, `on_tool_start/end()`

Key design:
- `span_stack: Vec<(String, SpanKind)>` — auto-infer parent_id from top of stack
- `next_span_id: u64` — counter for span_id generation
- `emit_stream_delta: bool` — off by default to avoid noise
- All operations are O(1); channel send is non-blocking

### 2. `TraceWriter` (`src/trace/writer.rs`)

Dedicated thread that drains the channel and writes to `.ccd.trace.ndjson`.

- `spawn(root: &Path) -> Sender<TraceEvent>` — creates thread, returns channel sender
- Handles file rotation at 10 MB
- Writes meta header line on first write
- flushes after each line (real-time `tail -f` support)

### 3. `TraceReader` (`src/trace/reader.rs`)

Reads `.ccd.trace.ndjson` and provides structured query capabilities.

- `read_tree()` — full span tree reconstruction
- `recent_events(n)` — last N events for quick glance
- `filter_by_kind(kind)` — filter by span type
- `filter_by_file(path)` — find all events touching a specific file
- `render_for_llm(max_events, details)` — render as text summary for LLM consumption

### 4. Init Function (`src/trace/mod.rs`)

```rust
pub fn init_trace(root: &Path) -> Option<TraceEmitter>
```

- Reads `CODECODER_TRACE=1` env var or `codecoder.json` config
- Spawns TraceWriter thread
- Returns TraceEmitter (or None if disabled)

## AgentLoop Integration

### Modification Points

**`src/agent.rs`** — `AgentLoop` struct:
- Add `trace_emitter: Option<TraceEmitter>` field
- In `process_turn()`:
  - `on_turn_start()` at entry
  - `on_llm_call_start/end()` wrapping each LLM call
  - `on_tool_start/end()` wrapping each tool call
  - `span_end()` at exit

**`src/background.rs`** — `drain_bg_events()`:
- Pass `Option<&mut TraceEmitter>` alongside existing `BgObserver`
- Call `on_agent_event()` for each AgentEvent

**`src/config.rs`**:
- Add `trace_enabled: bool` field (default false)

### Performance

- O(1) operations in TraceEmitter; no heap allocation per event beyond the JSON serialization in the writer thread
- Channel is mpsc — writer thread handles all I/O, never blocks AgentLoop
- Zero overhead when disabled (None + `as_mut().map(...)`)

## LLM Self-Observation Format

`TraceReader::render_for_llm()` produces:

```
## Trace: 重构 main.rs (2026-07-29 14:23:00)
⏱ 1.4s | 8 events | 1 LLM call (1950 tokens) | 2 tools

### Turn #1 (1.4s)
  LlmCall (1.1s, 1500→450 tokens, model: gpt-4o)
    → ToolCall: read_file
      · 文件: src/main.rs
      · 75ms · 成功
    → ToolCall: write_file
      · 文件: src/main.rs
      · 149ms · 成功 (42 bytes)

### 文件 touch 汇总
  src/main.rs: [Read, Edit]
```

This format is designed to be injected into the LLM's system prompt or context when the agent needs to reason about its own past execution.

## Existing BgObserver Relationship

- **BgObserver** is preserved unchanged — it handles headless mode real-time status output
- **TraceEmitter** is a parallel, complementary system
- BgObserver: high-level "what's happening now" for human `tail -f`
- Trace: full structured execution history for replay and self-observation
- Future: BgObserver could be simplified to consume TraceEmitter's real-time stream

## File Changes Summary

| File | Action | Description |
|------|--------|-------------|
| `src/trace/mod.rs` | Create | Module entry + `init_trace()` |
| `src/trace/types.rs` | Create | `TraceEvent`, `SpanKind`, `EventKind`, etc. |
| `src/trace/emitter.rs` | Create | `TraceEmitter` |
| `src/trace/writer.rs` | Create | `TraceWriter` |
| `src/trace/reader.rs` | Create | `TraceReader` |
| `src/lib.rs` | Modify | Add `pub mod trace` |
| `src/agent.rs` | Modify | Add `trace_emitter` field, instrument `process_turn` |
| `src/background.rs` | Modify | Pass trace emitter to `drain_bg_events` |
| `src/config.rs` | Modify | Add `trace_enabled` config field |

## Test Plan

1. **Unit tests for TraceEmitter**: span start/end nesting, parent_id assignment, AgentEvent translation
2. **Unit tests for TraceWriter**: file creation, rotation, concurrent writes
3. **Unit tests for TraceReader**: tree reconstruction, file touch aggregation, LLM render output
4. **Integration**: verify trace file is created when `CODECODER_TRACE=1`, verify file content matches expected format
5. **Performance**: verify no measurable impact on AgentLoop throughput when trace is disabled