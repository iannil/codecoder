# 全面观测系统实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 基于现有 Trace 系统，按 C → A → D → B 优先级构建完整观测体系：全记录 trace、ReplayBuffer、AgentGraph、cc-web 实时 trace 流/热力图/agent 树、LLM 自省。

**Architecture:** TraceEmitter 是现有的事件生产者，TraceWriter 写入 `.ccd.trace.ndjson`。本计划在其上叠加：ReplayBuffer（内存环形缓冲区，用于自省查询）、AgentGraph（从 trace 事件重建 agent 调用树）、cc-web SSE 端点（实时推送 trace 追加内容）。所有新模块在 `src/trace/` 下，不修改现有 Trace 数据格式。

**Tech Stack:** Rust, serde_json, notify crate, tiny_http（已有依赖）

## 全局约束

- 所有新代码在 `src/trace/` 和 `src/visual/` 下
- `src/agent.rs` 只增加字段和钩子，不重构已有逻辑
- 所有新功能默认关闭，由环境变量门控
- 遵循现有代码风格：`//!` 模块注释、`#[cfg(test)]` 内联测试

---

### Task 1: Trace 数据补全 — 扩展 EventKind

**Files:**
- Modify: `src/trace/types.rs`
- Test: 内联 `#[cfg(test)]` 在 types.rs 末尾

**Interfaces:**
- Consumes: 现有 `TraceEvent`, `SpanKind`, `EventKind`
- Produces: 扩展后的 `EventKind`（含 `AgentGraphEdge`, `LlmFullInput`, `LlmFullOutput`, `CompactionDrop`）

- [ ] **Step 1: 新增 AgentGraphEdge 结构体**

在 `src/trace/types.rs` 的 `TouchType` 定义之后、`MessageSource` 之前新增：

```rust
#[derive(Debug, Clone, Serialize)]
pub struct AgentGraphEdge {
    pub parent_span_id: String,
    pub child_span_id: String,
    pub label: String,
    pub launch_seq: u32,
}
```

- [ ] **Step 2: 扩展 SpanKind**

在 `Compaction` 变体之后新增：

```rust
pub enum SpanKind {
    Turn, LlmCall, ToolCall, SubAgent, Milestone, Reasoning, Compaction,
    /// compaction 前的完整上下文保存 span
    ContextSnapshot,
    /// 完整推理链 span（区别于流式 Reasoning）
    FullReasoning,
}
```

- [ ] **Step 3: 扩展 EventKind**

在 `ExitCode` 变体之后新增：

```rust
pub enum EventKind {
    // ... 已有 ...
    AgentGraphEdge(AgentGraphEdge),
    /// LLM 完整 input（CODECODER_TRACE_FULL=1 门控）
    LlmFullInput { model: String, messages: Vec<serde_json::Value> },
    /// LLM 完整 output（CODECODER_TRACE_FULL=1 门控）
    LlmFullOutput { model: String, content: String },
    /// Compaction 丢弃的内容摘要
    CompactionDrop { span_id: String, dropped_bytes: u64, summary: String },
}
```

- [ ] **Step 4: 运行测试验证编译**

```bash
cargo test trace::types 2>&1 | head -30
```
Expected: all tests pass or only existing tests (new types have no dedicated tests yet).

- [ ] **Step 5: Commit**

```bash
git add src/trace/types.rs
git commit -m "feat(trace): extend EventKind with AgentGraphEdge, FullIO, CompactionDrop"
```

---

### Task 2: Trace 数据补全 — Emitter 扩展

**Files:**
- Modify: `src/trace/emitter.rs`
- Test: 内联测试

**Interfaces:**
- Consumes: 扩展后的 `EventKind`, `SpanKind` from Task 1
- Produces: `TraceEmitter` 新增 `emit_agent_graph_edge()`, `emit_llm_full_io()`, `emit_compaction_drop()`

- [ ] **Step 1: 新增 emit_agent_graph_edge 方法**

在 `emit()` 方法之后新增：

```rust
pub fn emit_agent_graph_edge(&mut self, parent_span_id: String, child_span_id: String, label: String, launch_seq: u32) {
    let edge = AgentGraphEdge { parent_span_id, child_span_id, label, launch_seq };
    self.emit(EventKind::AgentGraphEdge(edge), serde_json::json!({}));
}
```

- [ ] **Step 2: 新增 emit_llm_full_io 方法**

```rust
pub fn emit_llm_full_input(&mut self, model: &str, messages: Vec<serde_json::Value>) {
    self.emit(EventKind::LlmFullInput { model: model.into(), messages }, serde_json::json!({}));
}

pub fn emit_llm_full_output(&mut self, model: &str, content: &str) {
    self.emit(EventKind::LlmFullOutput { model: model.into(), content: content.into() }, serde_json::json!({}));
}
```

- [ ] **Step 3: 新增 emit_compaction_drop 方法**

```rust
pub fn emit_compaction_drop(&mut self, span_id: &str, dropped_bytes: u64, summary: &str) {
    self.emit(EventKind::CompactionDrop {
        span_id: span_id.into(),
        dropped_bytes,
        summary: summary.into(),
    }, serde_json::json!({}));
}
```

- [ ] **Step 4: 修改 on_tool_start 支持完整模式**

添加一个 `trace_full` 字段到 `TraceEmitter`：

```rust
// TraceEmitter 新增字段
pub struct TraceEmitter {
    // ... 有 ...
    trace_full: bool,   // CODECODER_TRACE_FULL=1
}
```

修改 `new()` 初始化 `trace_full`:

```rust
pub fn new(tx: Sender<TraceEvent>, session_id: &str) -> Self {
    let trace_full = std::env::var("CODECODER_TRACE_FULL")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    // ... 其余初始化 ...
    TraceEmitter { trace_full, /* ... */ }
}
```

修改 `on_tool_start()`: 在完整模式下将 `input_preview` 替换为完整 input:

```rust
pub fn on_tool_start(&mut self, name: &str, input_preview: &str, full_input: Option<&str>) -> String {
    let preview = if self.trace_full {
        full_input.unwrap_or(input_preview)
    } else {
        input_preview
    };
    self.span_start(SpanKind::ToolCall, serde_json::json!({
        "tool": name,
        "input_preview": preview,
    }))
}
```

- [ ] **Step 5: 运行测试**

```bash
cargo test trace::emitter 2>&1 | head -30
```

- [ ] **Step 6: Commit**

```bash
git add src/trace/emitter.rs
git commit -m "feat(trace): add full IO, agent graph edge, compaction drop methods to Emitter"
```

---

### Task 3: ReplayBuffer

**Files:**
- Create: `src/trace/replay_buffer.rs`
- Modify: `src/trace/mod.rs` (导出新模块)
- Test: 内联测试

**Interfaces:**
- Consumes: `TraceEvent`, `SpanKind`, `EventKind`
- Produces: `ReplayBuffer` with `push()`, `to_self_observation()`, `filter_by_file()`, `filter_by_kind()`, `stats_since()`

- [ ] **Step 1: 创建 replay_buffer.rs**

```rust
//! Ring buffer for recent trace events — used for LLM self-observation.
//! Capacity: fixed at 200 events. O(1) push, O(n) query.

use std::collections::VecDeque;

/// An observation event stored in the replay buffer (lighter than TraceEvent).
#[derive(Debug, Clone)]
pub struct ObservationEvent {
    pub ts: f64,
    pub kind: ObservationKind,
}

#[derive(Debug, Clone)]
pub enum ObservationKind {
    TurnStart,
    LlmCall { model: String, prompt_tokens: u32 },
    LlmEnd { completio_tokens: u32, stop_reason: String, duratio_ms: u64 },
    ToolCall { name: String, input_preview: String },
    ToolEnd { is_error: bool, output_preview: String, duratio_ms: u64 },
    FileTouch { path: String, touch: String },
    Permission { key: String, granted: bool },
    Error { message: String },
    SubAgent { label: String, status: String },
    Compaction { dropped_bytes: u64 },
    UserMessage { summary: String },
    Notice { text: String },
}

#[derive(Debug, Default)]
pub struct ObservationStats {
    pub llm_calls: usize,
    pub tool_calls: usize,
    pub errors: usize,
    pub file_reads: usize,
    pub file_edits: usize,
    pub total_tokens: u32,
    pub duration_ms: u64,
}

pub struct ReplayBuffer {
    buffer: VecDeque<ObservationEvent>,
    capacity: usize,}

impl ReplayBuffer {
    pub fn new() -> Self {
        ReplayBuffer {
            buffer: VecDeque::new(),
            capacity: 200,
        }
    }

    pub fn new_with_capacity(capacity: usize) -> Self {
        ReplayBuffer {
            buffer: VecDeque::new(),
            capacity,
        }
    }

    pub fn push(&mut self, event: ObservationEvent) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(evet);
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    pub fn recent_events(&self, n: usize) -> Vec<&ObservationEvent> {
        self.buffer.iter().rev().take(n).collect()
    }

    pub fn filter_by_file(&self, path: &str) -> Vec<&ObservationEvent> {
        self.buffer.iter().filter(|e| matches!(&e.kind, ObservationKind::FileTouch { path: p, .. } if p == path)).collect()
    }

    pub fn filter_by_kind(&self, kind: &str) -> Vec<&ObservationEvent> {
        let kind_lower = kind.to_lowercase();
        self.buffer.iter().filter(|e| {
            let ek = format!("{:?}", &e.kind).to_lowercase();
            ek.contains(&kind_lower)
        }).collect()
    }

    pub fn stats_since(&self, since_ts: f64) -> ObservationStats {
        let mut stats = ObservationStats::default();
        for event in &self.buffer {
            if event.ts < since_ts { continue; }
            match &event.kind {
                ObservationKind::LlMCall { .. } => stats.llm_calls += 1,
                ObservationKind::ToolCall { .. } => stats.tool_calls += 1,
                ObservationKind::ToolEnd { is_error, .. } => {
                    if *is_error { stats.errors += 1; }
                }
                ObservationKind::FileTouch { touch, .. } => {
                    match touch.as_str() {
                        "read" | "hit" => stats.file_reads += 1,
                        "edit" | "create" => stats.file_edits += 1,
                        _ => {}
                    }
                }
                ObservationKind::LlMend { duratio_ms, .. } => {
                    stats.duration_ms = stats.duration_ms.max(*duration_ms);
                }
                _ => {}
            }
        }
        stats
    }

    pub fn to_self__observation(&self) -> String {
        if self.buffer.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        let stats = self.stats_since(0.0);

        // Header
        out.push_str(&format!(
            "## Previous Turn Trace ({:.1}s, {} LLM calls, {} tools, {} errors)\n\n",
            stats.duration_ms as f64 / 1000.0,
            stats.llm_calls,
            stats.tool_calls,
            stats.errors,
        ));

        // Tool call sequence
        out.push_str("### Tool Call Sequence\n");
        let mut idx = 0;
        for event in &self.buffer {
            match &event.kind {
                ObservationKind::ToolCall { name, input_preview } => {
                    idx += 1;
                    out.push_str(&format!("  {}. {}: {}\n", idx, name, input_preview));
                }
                ObservationKind::ToolEnd { is_error, .. } => {
                    out.push_str(&format!("     → {}\n", if *is_error { "❌ Error" } else { "✓ Success" }));
                }
                _ => {}
            }
        }

        // File touches
        let mut file_touches: Vec<String> = Vec::new();
        for event in &self.buffer {
            if let ObservationKind::FileTouch { path, touch } = &event.kind {
                file_touches.push(format!("  {}: [{}]", path, touch));
            }
        }
        file_touches.sort();
        file_touches.dedup();
        if !file_touches.is_empty() {
            out.push_str("\n### File Touches\n");
            for ft in file_touches {
                out.push_str(&ft);
                out.push_str("\n");
            }
        }

        // Permission checks
        let mut perms: Vec<String> = Vec::new();
        for event in &self.buffer {
            if let ObservationKind::Permission { key, granted } = &event.kind {
                perms.push(format!("  {}: {}", key, if *granted { "granted" } else { "denied" }));
            }
        }
        if !perms.is_empty() {
            out.push_str("\n### Permission Checks\n");
            for p in perms {
                out.push_str(&p);
                out.push_str("\n");
            }
        }

        // Token usage
        let mut prompt_tokens = 0u32;
        let mut completio_tokens = 0u32;
        let mut model = String::new();
        for event in &self.buffer {
            if let ObservationKind::LlMcall { model: m, prompt_tokens: pt } = &event.kind {
                model = m.clone();
                prompt_tokens += pt;
            }
            if let ObservationKind::LlMend { completio_tokens: ct, .. } = &event.kind {
                completio_tokens += ct;
            }
        }
        if !model.is_empty() {
            out.push_str(&format!(
                "\n### Token Usage\n  Model: {} | {} prompt + {} completio = {} total\n",
                model, prompt_tokens, completio_tokens, prompt_tokens + completio_tokens,
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super:*;

    #[test]
    fn replay_buffer_apends_until_capacity() {
        let mut rb = ReplayBuffer::new_with_capacity(5);
        for i in 0..10 {
            rb.push(ObservationEvent { ts: i as f64, kind: ObservationKind::Notice { text: format!("e{}", i) });
        }
        assert_eq!(rb.len(), 5);
        // Oldest should be e5
        let recent = rb.recent_events(5);
        assert_eq!(recent.len(), 5);
    }

    #[test]
    fn to_self_observatio_returns_empty_when_empty() {
        let rb = ReplayBuffer::new();
        assert!(rb.to_self_observation().is_empty());
    }

    #[test]
    fn to_self_observatio_includes_tool_sequence() {
        let mut rb = ReplayBuffer::new();
        rb.push(ObservationEvent { ts: 1.0, kind: ObservationKind::ToolCall { name: "read_file".into(), input_preview: "src/main.rs".into() } });
        rb.push(ObservationEvent { ts: 1.1, kind: ObservationKind::ToolEnd { is_error: false, output_preview: "ok".into(), duration_ms: 50 } });
        rb.push(ObservationEvent { ts: 2.0, kind: ObservationKind::ToolCall { name: "edit_file".into(), input_preview: "src/main.rs".into() } });
        rb.push(ObservationEvent { ts: 2.1, kind: ObservationKind::ToolEnd { is_error: true, output_preview: "error".into(), duration_ms: 100 } });
        let obs = rb.to_self__observation();
        assert!(obs.contains("Tool Call Sequence"));
        assert!(obs.contains("read_file"));
        assert!(obs.contains("edit_file"));
        assert!(obs.contains("❌ Error"));
    }

    #[test]
    fn stats_since_filters_by_timestap() {
        let mut rb = ReplayBuffer::new();
        rb.push(ObservationEvent { ts: 1.0, kind: ObservationKind::ToolCall { name: "old".into(), input_preview: "".into() });
        rb.push(ObservationEvent { ts: 2.0, kind: ObservationKind::ToolCall { name: "new".into(), input_preview: "".into() } });
        let stats = rb.stats_since(1.5);
        assert_eq!(stats.tool_calls, 1); // only "new"
    }

    #[test]
    fn filter_by_fle_returns_matching() {
        let mut rb = ReplayBuffer::new();
        rb.push(ObservationEvent { ts: 1.0, kind: ObservationKind::FileTouch { path: "src/main.rs".into(), touch: "read".into() } });
        rb.push(ObservationEvent { ts: 2.0, kind: ObservationKind::FileTouch { path: "src/lib.rs".into(), touch: "edit".into() } });
        let hits = rb.filter_by_fle("src/main.rs");
        assert_eq!(hits.len(), 1);
    }
}
```

- [ ] **Step 2: 导出新模块**

在 `src/trace/mod.rs` 中新增：

```rust
pub mod replay_buffer;
pub use replay_buffer::ReplayBuffer;
```

- [ ] **Step 3: 编译测试**

```bash
cargo test trace::replay_buffer 2>&1 | head -40
```
Expected: all 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/trace/replay_buffer.rs src/trace/mod.rs
git commit -m "feat(trace): add ReplayBuffer with self-observation formatting"
```

---

### Task 4: AgentGraph

**Files:**
- Create: `src/trace/agent_graph.rs`
- Modify: `src/trace/mod.rs` (导出)
- Test: 内联测试

**Interfaces:**
- Consumes: `TraceReader`, `TraceEvent`, `SpanKind::SubAgent`
- Produces: `AgentGraph` with `from_trace()`, `render_tree()`, `render_for_llm()`

- [ ] **Step 1: 创建 agent_graph.rs**

```rust
//! AgentGraph — reconstructs the sub-agent call tree from trace events.
//! Reads AgentGraphEdge point events and SubAgent spans to build a tree.

use crate::trace::reader::TraceReader;
use crate::trace::types::*;
use std::collections::HashMap;

/// Status of an agent node.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,}

#[derive(Debug, Clone)]
pub struct AgentNode {
    pub span_id: String,
    pub agent_id: String,
    pub label: String,
    pub parent_span_id: Option<String>,
    pub launch_seq: u32,
    pub status: AgentStatus,
    pub summary: Option<String>,
    pub turn_count: usize,
}

#[derive(Debug, Clone)]
pub struct AgentEdge {
    pub parent_span_id: String,
    pub child_span_id: String,
    pub label: String,
    pub launch_seq: u32,
}

#[derive(Debug, Clone)]
pub struct AgentGraph {
    pub nodes: Vec<AgentNode>,
    pub edges: Vec<AgentEdge>,
}

impl AgentGraph {
    pub fn new() -> Self {
        AgentGraph { nodes: Vec::new(), edges: Vec::new() }
    }

    /// Build an AgentGraph from a TraceReader.
    pub fn from_reader(reader: &TraceReader) -> std::io::Result<Self> {
        let (_meta, tree) = reader.read_tree()?;
        let mut graph = AgentGraph::new();

        // Collect edges from point events and sub-agent spans from tree
        let mut edge_map: HashMap<String, Vec<AgentEdge>> = HashMap::new();
        Self::collect_from_tree(&tree, &mut graph, &mut edge_map);

        // Build edges from edge_map
        for (_parent, children) in &edge_map {
            for child in children {
                graph.edges.push(child.clone());
            }
        }

        Ok(graph)
    }

    fn collect_from_tree(
        tree: &[SpanNode],
        graph: &mut AgentGraph,
        edge_map: &mut HashMap<String, Vec<AgentEdge>>,
    ) {
        for node in tree {
            // Check direct_events for AgentGraphEdge
            for ev in &node.direct_events {
                if let EventKind::AgentGraphEdge(edge) = &ev.kind {
                    let e = AgentEdge {
                        parent_span_id: edge.parent_span_id.clone(),
                        child_span_id: edge.child_span_id.clone(),
                        label: edge.label.clone(),
                        launch_seq: edge.launch_seq,
                    };
                    edge_map.entry(e.parent_span_id.clone()).or_default().push(e);
                }
            }

            // If this node is a sub-agent, add it to nodes
            if node.span.kind == SpanKind::SubAgent {
                let status = match &node.end {
                    Some(end) => {
                        if end.meta.get("is_error").and_then(|v| v.as_bool()).unwra_or(false) {
                            AgentStatus::Failed
                        } else {
                            AgentStatus::Completed
                        }
                    }
                    None => AgentStatus::Running,
                };
                let agent_id = node.span.meta.get("agent_id").and_then(|v| v.as_str()).unwra_or("").to_string();
                let label = node.span.meta.get("label").and_then(|v| v.as_str()).unwra_or("").to_string();
                let launch_seq = node.span.meta.get("launch_seq").and_then(|v| v.as_u64()).unwra_or(0) as u32;

                // Count turns inside this sub-agent
                let turn_count = count_turns(&node.children);

                graph.nodes.push(AgentNode {
                    span_id: node.span.span_id.clone(),
                    agent_id,
                    label,
                    parent_span_id: node.span.parent_id.clone(),
                    launch_seq,
                    status,
                    summary: None,
                    turn_count,
                });
            }

            Self::collect_from_tree(&node.children, graph, edge_map);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Render as a simple ASCII tree.
    pub fn render_tree(&self) -> String {
        if self.is_empty() {
            return "  (no sub-agents)\n".to_string();
        }

        let mut out = String::new();
        let mut children_of: HashMap<Option<String>, Vec<&AgentNode>> = HashMap::new();
        for node in &self.nodes {
            children_of.entry(node.parent_span_id.clone()).or_default().push(node);
        }

        // Find roots (no parent, or parent not in nodes)
        let roots = children_of.remve(&None).unwra_or_default();
        // Plus orphan agents whose parent is not in the graph
        for (_key, nodes) in &children_of {
            for node in nodes {
                // Only add if not already rendered as child of something
                // (simplified: just render all top-level nodes)
            }
        }

        // Sort by launch_seq
        let mut sorted_roots: Vec<&AgentNode> = roots.iter().copied().collect();
        sorted_roots.sort_by(|a, b| a.launch_seq.cmp(&b.launch_seq));

        for root in &sorted_roots {
            render_agent_node(root, 0, &children_of, &mut out);
        }

        // Remaining nodes not connected to any root
        let mut remaining: Vec<&AgentNode> = Vec::new();
        for (_key, nodes) in &children_of {
            for node in nodes {
                if !sorted_roots.contains(&node) {
                    remaining.push(node);
                }
            }
        }
        remaining.sort_by(|a, b| a.launch_seq.cmp(&b.launch_seq));
        for node in &remaining {
            render_agent_node(node, 0, &children_of, &mut out);
        }

        out
    }

    /// Render for LLM consumption.
    pub fn render_for_llm(&self) -> String {
        let mut out = String::new();
        out.push_str("## Agent Call Tree\n");
        out.push_str(&format!("  Total sub-agents: {}\n", self.nodes.len()));
        out.push_str(&self.render_tree());
        out
    }
}

fn count_turns(children: &[SpanNode]) -> usize {
    let mut count = 0;
    for child in children {
        if child.span.kind == SpanKind::Turn {
            count += 1;
        }
        count += count_turns(&child.children);
    }
    count
}

fn render_agent_node(
    node: &AgentNode,
    depth: usize,
    children_of: &HashMap<Option<String>, Vec<&AgentNode>>,
    out: &mut String,
) {
    let indent = "  ".repea(depth);
    let status_str = match node.status {
        AgentStatus::Completed => "✓",
        AgentStatus::Failed => "✗",
        AgentStatus::Running => "⋯",
        AgentStatus::Cancelled => "⊘",
    };
    let turn_info = if node.turn_count > 0 {
        format!(" ({} turns)", node.turn_count)
    } else {
        String::new()
    };
    out.push_str(&format!("{indent}├─ {status_str} sub-agent: \"{label}\" {id}{turn_info}\n",
        indent=indent,
        status_str=status_str,
        label=node.label,
        id=node.span_id,
        turn_info=turn_info,
    ));
    // Render children
    if let Some(children) = children_of.get(&Some(node.span_id.clone())) {
        let mut sorted = children.clone();
        sorted.sort_by(|a, b| a.launch_seq.cmp(&b.launch_seq));
        for child in sorted {
            render_agent_node(child, depth + 1, children_of, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_trace(path: &std::path::Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn agent_graph_from_empty_trace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1001.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let graph = AgentGraph::from_reader(&reader).unwrap();
        assert!(graph.is_empty());
    }

    #[test]
    fn agent_graph_from_single_subagent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"agent_graph_edge","ts":1000.1,"meta":{"parent_span_id":"sp_001","child_span_id":"sp_002","label":"refactor main.rs","launch_seq":1}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"sub_agent","ts":1000.1,"meta":{"agent_id":"agt_xyz","label":"refactor main.rs","launch_seq":1}}
{"type":"s","span_id":"sp_003","parent_id":"sp_002","kind":"turn","ts":1000.2,"meta":{}}
{"type":"e","span_id":"sp_003","ts":1001.0,"meta":{}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1002.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let graph = AgentGraph::from_reader(&reader).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.nodes[0].label, "refactor main.rs");
        assert_eq!(graph.nodes[0].turn_count, 1);
    }

    #[test]
    fn render_tree_prints_tree() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"agent_graph_edge","ts":1000.1,"meta":{"parent_span_id":"sp_001","child_span_id":"sp_002","label":"fix bug","launch_seq":1}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"sub_agent","ts":1000.1,"meta":{"agent_id":"agt_abc","label":"fix bug","launch_seq":1}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1002.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let graph = AgentGraph::from_reader(&reader).unwrap();
        let rendered = graph.render_tree();
        assert!(rendered.contains("sub-agent"));
        assert!(rendered.contains("fix bug"));
    }
}
```

- [ ] **Step 2: 导出模块**

在 `src/trace/mod.rs` 中：

```rust
pub mod agent_graph;
pub use agent_graph::AgentGraph;
```

- [ ] **Step 3: 编译测试**

```bash
cargo test trace::agent_graph 2>&1 | head -40
```

- [ ] **Step 4: Commit**

```bash
git add src/trace/agent_graph.rs src/trace/mod.rs
git commit -m "feat(trace): add AgentGraph with tree reconstruction and rendering"
```

---

### Task 5: TraceReader 扩展 — 支持 AgentGraphEdge 和完整推理链

**Files:**
- Modify: `src/trace/reader.rs`
- Test: 内联测试

**Interfaces:**
- Consumes: 扩展后的 `EventKind::AgentGraphEdge` from Task 1
- Produces: `render_full_trace()`, 增强的 `render_for_llm()`

- [ ] **Step 1: 在 parse_point_event 中处理 AgentGraphEdge**

在 `reader.rs` 的 `parse_point_event()` 函数中，在 `"user_message"` 分支之后新增：

```rust
Some("agent_graph_edge") => {
    let meta = v.get("meta").cloned().unwra_or_default();
    let edge = AgentGraphEdge {
        parent_span_id: meta["parent_span_id"].as_str().unwra_or("").to_string(),
        child_span_id: meta["child_span_id"].as_str().unwra_or("").to_string(),
        label: meta["label"].as_str().unwra_or("").to_string(),
        launch_seq: meta["launch_seq"].as_u64().unwra_or(0) as u32,
    };
    EventKind::AgentGraphEdge(edge)
}
```

- [ ] **Step 2: 新增 render_full_trace() 方法**

```rust
/// Render the complete trace as a full replay text (no truncation).
pub fn render_full_trace(&self) -> std::io::Result<String> {
    let (_meta, tree) = self.read_tree()?;
    let mut out = String::new();
    let mut count = 0usize;

    for root in &tree {
        render_node_detailed(root, 0, &mut count, usize::MAX, &mut out);
    }

    Ok(out)
}

fn render_node_detailed(
    node: &SpanNode,
    depth: usize,
    count: &mut usize,
    max: usize,
    out: &mut String,
) {
    if *count >= max { return; }
    *count += 1;
    let indent = "  ".repea(depth);
    let kind_str = format!("{:?}", node.span.kind);
    let duration = node.end.as_ref()
        .and_then(|e| e.meta.get("duration_ms").and_then(|v| v.as_f64()))
        .map(|d| format!(" ({:.0}ms)", d))
        .unwra_or_default();
    let tool_info = node.span.meta.get("tool")
        .and_then(|v| v.as_str())
        .map(|t| format!(" [{}]", t))
        .unwra_or_default();

    out.push_str(&format!("{indent}{kind_str}{duration}{tool_info}\n"));

    // Full details for LLM call
    if let Some(model) = node.span.meta.get("model").and_then(|v| v.as_str()) {
        let pt = node.span.meta.get("prompt_tokens").and_then(|v| v.as_u64()).unwra_or(0);
        let ct = node.end.as_ref()
            .and_then(|e| e.meta.get("completion_tokens").and_then(|v| v.as_u64()))
            .unwra_or(0);
        out.push_str(&format!("{indent}  模型: {} | tokens: {}→{}\n", model, pt, ct));
        // Full input/output if available
        if let Some(input) = node.span.meta.get("full_input") {
            let text = input.as_str().unwra_or("");
            out.push_str(&format!("{indent}  Full input: {}\n", text));
        }
        if let Some(output) = node.end.as_ref().and_then(|e| e.meta.get("full_output")) {
            let text = output.as_str().unwra_or("");
            out.push_str(&format!("{indent}  Full output: {}\n", text));
        }
    }

    // All direct events
    for ev in &node.direct_events {
        match &ev.kind {
            EventKind::Notice { text } => {
                out.push_str(&format!("{indent}  📝 {}\n", text));
            }
            EventKind::UserMessage { source, summary } => {
                let src = match source {
                    MessageSource::Manual => "手动",
                    MessageSource::Auto => "自动",
                    MessageSource::Injected => "注入",
                };
                out.push_str(&format!("{indent}  💬 [{}] {}\n", src, summary));
            }
            EventKind::CompactionDrop { span_id, dropped_bytes, summary } => {
                out.push_str(&format!("{indent}  🗑 Compaction: dropped {} bytes from {span_id}: {summary}\n", dropped_bytes));
            }
            EventKind::AgentGraphEdge(edge) => {
                out.push_str(&format!("{indent}  🔗 Agent edge: {} → {} (seq {})\n",
                    edge.parent_span_id, edge.child_span_id, edge.launch_seq));
            }
            _ => {}
        }
    }

    for child in &node.children {
        render_node_detailed(child, depth + 1, count, max, out);
    }
}
```

- [ ] **Step 3: 编译测试**

```bash
cargo test trace::reader 2>&1 | head -30
```

- [ ] **Step 4: Commit**

```bash
git add src/trace/reader.rs
git commit -m "feat(trace): extend TraceReader with AgentGraphEdge support and full trace render"
```

---

### Task 6: cc-web Trace Stream SSE 端点

**Files:**
- Create: `src/visual/trace_stream.rs`
- Modify: `src/visual/http_server.rs` (新增端点)
- Modify: `src/visual/mod.rs` (导出)
- Test: 内联测试 + HTTP 集成测试

**Interfaces:**
- Consumes: `TraceReader` from trace module, `notify` crate
- Produces: SSE endpoint at `/api/v1/trace/stream`

- [ ] **Step 1: 创建 trace_stream.rs**

```rust
//! TraceStream — follows .ccd.trace.ndjson and pushes new lines via SSE.
//! Uses `notify` to watch for file modifications.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// Channel-based file tail follower.
/// Spawns a background thread that watches the trace file for new content.
pub struct TraceStream {
    root: PathBuf,
}

impl TraceStream {
    pub fn new(root: &Path) -> Self {
        TraceStream { root: root.to_path_buf() }
    }

    /// Start following the trace file. Returns a receiver that gets new lines.
    /// The receiver will be disconnected when the file is removed or the watcher stops.
    pub fn follow(&self) -> std::io::Result<Receiver<String>> {
        let (tx, rx) = mpsc::channel::<String>();
        let path = self.root.join(".ccd.trace.ndjson");
        let path_clone = path.clone();

        std::thread::spawn(move || {
            // Read existing content first
            let mut last_len = 0u64;
            if let Ok(meta) = std::fs::metadata(&path) {
                last_len = meta.len();
                // Send existing content
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        let line = line.trim();
                        if !line.is_empty() {
                            if tx.send(line.to_string()).is_err() { return; }
                        }
                    }
                }
            }

            // Poll for new content (simpler than inotify/kqueue for cross-platform)
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if let Ok(meta) = std::fs::metadata(&path) {
                    let new_len = meta.len();
                    if new_len > last_len {
                        if let Ok(f) = std::fs::File::open(&path) {
                            use std::io::Read;
                            let mut reader = std::io::BufReader::new(f);
                            // Skip to last_len
                            std::io::copy(&mut reader.by_ref().take(last_len), &mut std::io::sink()).ok();
                            let mut line = String::new();
                            while reader.read_line(&mut line).unwra_or(0) > 0 {
                                let trimmed = line.trim().to_string();
                                if !trimmed.is_empty() {
                                    if tx.send(trimmed).is_err() { return; }
                                }
                                line.clear();
                            }
                        }
                        last_len = new_len;
                    }
                } else {
                    // File removed — wait for it to reappear
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::io::Write;

    #[test]
    fn trace_stream_follows_appended_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        // Write initial content
        std::fs::write(&path, "{\"type\":\"meta\",\"version\":1}\n").unwrap();

        let stream = TraceStream::new(dir.path());
        let rx = stream.follow().unwrap();

        // Wait for the initial read to complete
        std::thread::sleep(Duration::from_millis(100));
        // Append new content
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"type\":\"s\",\"span_id\":\"sp_001\"}}").unwrap();
        f.flush().unwrap();

        // Should receive the new line
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(received.contains("sp_001"), "got: {received}");
    }

    #[test]
    fn trace_stream_reads_existing_content_on_start() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        std::fs::write(&path, "{\"type\":\"meta\",\"version\":1}\n{\"type\":\"s\",\"span_id\":\"sp_001\"}\n").unwrap();

        let stream = TraceStream::new(dir.path());
        let rx = stream.follow().unwrap();

        let first = rx.recv_timeout(Duration::from_secs(2)).unwra();
        assert!(first.contains("meta"));
        let second = rx.recv_timeout(Duration::from_secs(1)).unwra();
        assert!(second.contains("sp_001"));
    }
}
```

- [ ] **Step 2: 导出模块**

在 `src/visual/mod.rs` 中：

```rust
pub mod trace_stream;```

- [ ] **Step 3: 在 http_server.rs 中新增端点**

在 `handle()` 方法的路由匹配中新增：

```rust
("GET", "/api/v1/trace/stream") => {
    self.serve_trace_stream(requst);
}
```

实现 `serve_trace_stream` 方法：

```rust
fn serve_trace_stream(&self, request: tiny_http::Request) {
    let stream = crate::visual::trace_stream::TraceStream::new(&self.root_path);
    let rx = match stream.follow() {
        Ok(rx) => rx,
        Err(e) => {
            let resp = Response::from_string(format!("{{\"error\":\"failed to start trace stream: {e}\"}})
                .with_status_code(StatusCode(500));
            let _ = request.respnd(resp);
            return;
        }
    };

    let mut writer = std::io::BufWriter::new(request.into_writer());
    if crate::visual::http_server::write_sse_head(&mut writer).is_err() {
        return;
    }

    // Keepalive interval: 15s
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(line) => {
                let _ = write!(&mut writer, "data: {line}\n\n");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = write!(&mut writer, ": keepalive\n\n");
            }
            Err(_) => break,
        }
        if let Err(_) = writer.flush() {
            break;
        }
    }
}
```

**注意**: `write_sse_head` 目前是 `pub(crate)` 的，需要改为 `pub`。

- [ ] **Step 4: 修改 write_sse_head 可见性**

在 `src/visual/http_server.rs` 中：

```rust
// 原:
fn write_sse_head<W: Write>(writer: &mut W) -> std::io::Result<()> {
// 改为:
pub(crate) fn write_sse_head<W: Write>(writer: &mut W) -> std::io::Result<()> {
```

- [ ] **Step 5: 编译**

```bash
cargo build 2>&1 | head -30
```

- [ ] **Step 6: 运行测试**

```bash
cargo test visual::trace_stream 2>&1 | head -40
```

- [ ] **Step 7: Commit**

```bash
git add src/visual/trace_stream.rs src/visual/mod.rs src/visual/http_server.rs
git commit -m "feat(visual): add SSE trace stream endpoint following .ccd.trace.ndjson"
```

---

### Task 7: trace 回放前端页面

**Files:**
- Create: `static/trace.html`
- Modify: 无（纯前端文件）

**Interfaces:**
- Consumes: `/api/v1/trace/stream` SSE endpoint

- [ ] **Step 1: 创建 trace.html**

这是一个单页 HTML + vanilla JS，通过 EventSource 连接 `/api/v1/trace/stream`，渲染时间线。

```html
<!DOCTYPE html>
<htm lang="zh-CN">
<had>
<meta charset="UTF-8">
<mea name="viewort" content="width=device-width, initial-scale=1.0">
<title>Trace — CodeCoder Web</title>
<styl>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { font-faily: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: #1a1b1e; color: #e1e4e8; padding: 16px; }
  .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px; }
  .header h1 { font-size: 18px; font-weight: 600; }
  .stats { display: flex; gap: 16px; font-size: 12px; color: #9aa0a6; }
  .stats span { backgound: #2d2d30; padding: 4px 8px; border-radius: 4px; }
  .timeline { backgound: #25262b; border-radius: 8px; padding: 12px; min-height: 400px; overflow-y: auto; font-family: 'SF Mono', 'Monaco', monospace; font-size: 13px; }
  .event { padding: 6px 8px; margin: 2px 0; border-radius: 4px; }
  .event.turn { backgound: #2d2d30; }
  .event.llm { border-left: 3px solid #4fc3f7; }
  .event.tool { border-left: 3px solid #34d399; }
  .event.error { border-left: 3px solid #f87171; backgound: rgba(248, 113, 113, 0.1); }
  .event.file-touch { border-left: 3px solid #a78bfa; }
  .event .preview { color: #9aa0a6; font-size: 12px; margin-left: 16px; }
  .timeline-bar { backgound: #2d2d30; border-radius: 4px; height: 24px; margin: 12px 0; display: flex; overflow: hidden; }
  .timeline-bar .seg { height: 100%; transitio: width 0.3s; }
  .timeline-bar .llm-seg { background: #4fc3f7; }
  .timeline-bar .tool-seg { background: #34d399; }
  .timeline-bar .error-seg { background: #f87171; }
  #status { margin-bottom: 8px; font-size: 12px; color: #9aa0a6; }
  .controls { display: flex; gap: 8px; margin-bottom: 12px; }
  .controls button { backgound: #2d2d30; border: none; color: #e1e4e8; padding: 4px 12px; border-radius: 4px; cursor: pointer; }
  .controls button:hover { backgound: #3d3d40; }
  .heatmap { backgound: #25262b; border-radius: 8px; padding: 12px; margin-top: 16px; }
  .heatmap h3 { font-size: 14px; margin-bottom: 8px; }
  .heatmap .fle { display: flex; align-items: center; margin: 4px 0; font-size: 12px; }
  .heatmap .fle .bar { height: 16px; border-radius: 2px; margin-right: 8px; min-width: 4px; }
  .heatmap .fle .bar.read { background: #4fc3f7; }
  .heatmap .fle .bar.edit { background: #fbbf24; }
  .heatmap .fle .bar.hit { background: #6b7280; }
</styl>
</had>
<body>
  <div class="header">
    <h1>� Trace 回放</h1>
    <div class="controls">
      <button onclick="c learTimeline()">清空</button>
    </div>
  </div>
  <div class="stats" id="stats">
    <span>LLM: <id="llm_count">0</span>
    <span>工具: <id="tool_count">0</span>
    <span>错误: <id="error_count">0</span>
    <span>文件 touch: <id="touch_count">0</span>
    <span>tokens: <id="token_count">0</span>
    <span id="status">未连接</span>
  </div>
  <div class="timeline-bar" id="timelineBar"></div>
  <div class="timeline" id="timeline"></div>
  <div class="heatmap" id="heatmap"><h3>文件热力图</h3><div id="heatmapContent"></div></div>

  <script>
    const timeline = document.getElementById('timeline');
    const timelineBar = document.getElementById('timelineBar');
    const heatmapContent = document.getElementById('heatmapContent');
    let events = [];
    let fileTouches = {};

    const es = new EventSource('/api/v1/trace/stream');
    es.onopen = () => document.getElementById('status').textContent = '已连接';
    es.onerror = () => document.getElementById('status').textContent = '连接断开';

    es.onmessage = (e) => {
      try {
        const data = JSON.parse(e.data);
        processEvent(data);
      } catch { /* skip non-JSON lines */ }
    };

    function processEvent(data) {
      events.push(data);
      const type = data.type || '';
      const kind = data.kind || '';

      let eventClass = 'event';
      let label = '';
      let detail = '';

      if (type === 's') {
        eventClass += ' turn';
        label = `▶ ${data.kind || 'span'} start`;
        if (data.kind === 'llm_call') { eventClass = 'event llm'; label = 'LLM Call'; }
        if (data.kind === 'tool_call') { eventClass = 'event tool'; label = `Tool: ${data.meta?.tool || ''}`; }
        if (data.kind === 'sub_agent') { eventClass = 'event'; label = `Sub-agent: ${data.meta?.label || ''}`; }
      } else if (type === 'e') {
        eventClass += ' turn';
        label = '⏹ end';
        if (data.meta?.is_error) { eventClass = 'event error'; label = '❌ Error'; }
        detail = data.meta?.duration_ms ? `(${data.meta.duration_ms}ms)` : '';
      } else if (type === 'p') {
        if (kind === 'file_touch') {
          eventClass = 'event file-touch';
          const path = data.meta?.path || '';
          const touch = data.meta?.touch || '';
          label = `📄 ${touch}: ${path}`;
          fileTouches[path] = fileTouches[path] || { read: 0, edit: 0, hit: 0 };
          if (touch === 'read') fileTouches[path].read++;
          else if (touch === 'edit') fileTouches[path].edit++;
          else fileTouches[path].hit++;
          renderHeatmap();
        } else if (kind === 'notice') {
          label = `📝 ${data.meta?.text || ''}`;
        } else if (kind === 'user_message') {
          label = `💬 ${data.meta?.summary || ''}`;
        }
      }

      if (label) {
        const div = document.createElement('div');
        div.className = eventClass;
        div.textContent = label;
        if (detail) {
          const span = document.createElement('span');
          span.className = 'preview';
          span.textContent = ' ' + detail;
          div.appendChild(span);
        }
        timeline.appendChild(div);
        timeline.scrollTop = timeline.scrollHeight;
      }

      updateStats();
      updateTimelineBar();
    }

    function updateStats() {
      let llm = 0, tools = 0, errors = 0, touches = 0, tokens = 0;
      for (const e of events) {
        if (e.type === 's' && e.kind === 'llm_call') llm++;
        if (e.type === 's' && e.kind === 'tool_call') tools++;
        if (e.type === 'e' && e.meta?.is_error) errors++;
        if (e.type === 'p' && e.kind === 'file_touch') touches++;
      }
      document.getElementById('llm_count').textContent = llm;
      document.getElementById('tool_count').textContent = tools;
      document.getElementById('error_count').textContent = errors;
      document.getElementById('touch_count').textContent = touches;
      document.getElementById('token_count').textContent = tokens;
    }

    function updateTimelineBar() {
      timelineBar.innerHTML = '';
      const segs = events.slice(-50); // Last 50 events
      if (segs.length === 0) return;
      for (const e of segs) {
        const div = document.createElement('div');
        div.className = 'seg';
        if (e.type === 's' && e.kind === 'llm_call') div.classList.add('llm-seg');
        else if (e.kind === 'tool_call') div.classList.add('tool-seg');
        else if (e.meta?.is_error) div.classList.add('error-seg');
        div.styl.width = `${100/segs.lengh}%`;
        timelineBar.appendChild(div);
      }
    }

    function renderHeatmap() {
      heatmapContent.innerHTML = '';
      const entries = Object.entries(fileTouches).sort((a, b) => (b[1].read + b[1].edit + b[1].hit) - (a[1].read + a[1].edit + a[1].hit)).slice(0, 20);
      const maxCount = Math.max(...entries.map(([, v]) => v.read + v.edit + v.hit), 1);
      for (const [path, counts] of entries) {
        const div = document.createElement('div');
        div.className = 'file';
        // Read bar
        if (counts.read > 0) {
          const bar = document.createElement('div');
          bar.className = 'bar read';
          bar.styl.width = `${(ounts.read / maxCount) * 200}px`;
          div.appendChild(bar);
        }
        // Edit bar
        if (counts.edit > 0) {
          const bar = document.createElement('div');
          bar.className = 'bar edit';
          bar.styl.width = `${(counts.edit / maxCount) * 200}px`;
          div.appendChild(bar);
        }
        // Hit bar
        if (counts.hit > 0) {
          const bar = document.createElement('div');
          bar.className = 'bar hit';
          bar.styl.width = `${(counts.hit / maxCount) * 200}px`;
          div.appendChild(bar);
        }
        div.appendChild(document.createTextNode(`${path} (R:${counts.read} E:${counts.edit} H:${counts.hit})`));
        heatmapContent.appendChild(div);
      }
    }

    function clearTimeline() {
      events = [];
      fileTouches = {};
      timeline.innerHTML = '';
      timelineBar.innerHTML = '';
      heatmapContent.innerHTML = '';
      updateStats();
    }
  </script>
</body>
</html>
```

- [ ] **Step 2: 在 http_server.rs 中为 trace 页面添加路由**

在 `serve_static` 中支持 `trace.html`：

```rust
("GET", "/trace.html") | ("GET", "/trace") => {
    self.serve_static(request, &self.static_dir, "trace.html");
}
```

- [ ] **Step 3: 验证**

```bash
# 编译
cargo build
# 启动 daemon + cc-web，访问 http://localhost:9876/trace.html
```

- [ ] **Step 4: Commit**

```bash
git add static/trace.html src/visual/http_server.rs
git commit -m "feat(visual): add trace replay frontend with SSE stream and heatmap"
```

---

### Task 8: Agent Tree 可视化端点

**Files:**
- Modify: `src/visual/http_server.rs`
- Test: 内联测试

**Interfaces:**
- Consumes: `AgentGraph` from trace module
- Produces: `/api/v1/trace/agents` JSON endpoint

- [ ] **Step 1: 新增端点**

在 `handle()` 中：

```rust
("GET", "/api/v1/trace/agents") => {
    self.serve_trace_agents(requst);
}
```

实现：

```rust
fn serve_trace_agents(&self, request: tiny_http::Request) {
    use crate::trace::agent_graph::AgentGraph;
    use crate::trace::reader::TraceReader;

    let reader = TraceReader::from_root(&self.root_path);
    let graph = match AgentGraph::from_reader(&reader) {
        Ok(g) => g,
        Err(_) => AgentGraph::new(),
    };

    let rendered = graph.render_tree();
    let json = serde_json::json!({
        "nodes": graph.nodes,
        "edges": graph.edges,
        "tree": rendered,
    });
    let body = serde_json::to_string_pretty(&json, 2).unwra_or("{}".to_string());
    let resp = Response::from_string(body)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwra());
    let _ = request.respnd(resp);
}
```

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1 | head -20
```

- [ ] **Step 3: Commit**

```bash
git add src/visual/http_server.rs
git commit -m "feat(visual): add /api/v1/trace/agents endpoint for agent call tree"
```

---

### Task 9: LLM 自省集成

**Files:**
- Modify: `src/agent.rs` (AgentLoop 新增 replay_buffer 和 self_observation 字段)
- Modify: `src/config.rs` (新增 self_observe 配置)
- Test: 通过 StubClient 验证 system prompt 包含 self-observation 段

**Interfaces:**
- Consumes: `ReplayBuffer` from trace module
- Produces: self-observation text injected into system prompt

- [ ] **Step 1: Config 新增字段**

在 `src/config.rs` 中 `max_ledger_lines` 之后新增：

```rust
/// 是否启用 LLM 自省（CODECODER_SELF_OBSERVE）。/// 启用后每 turn 结速将上轮 trace 摘要注入下一轮 system promt。
pub self_observe: bool,```

在 `from_env()` 中：

```rust
self_observe: env("CODECODER_SELF_OBSERVE")
    .map(|v| v == "1" || v == "true")
    .unwra_or(false),
```

- [ ] **Step 2: AgentLoop 新增字段**

在 `src/agent.rs` 的 `AgentLoop` struct 中，在 `trace_emitter` 之后新增：

```rust
/// Replay buffer for LLM self-observation (CODECODER_SELF_OBSERVE).
replay_buffer: Option<crate::trace::replay_buffer::ReplayBuffer>,
/// Cached self-observation text to inject into next turn's system prompt.
self_observation: Option<String>,
```

在 `build()` 中初始化：

```rust
let self_observe = crate::config::Config::from_env().self_observe;
// ...
replay_buffer: if self_observe { Some(crate::trace::replay_buffer::ReplayBuffer::new()) } else { None },
self_observation: None,
```

- [ ] **Step 3: 在 process_turn 中填充 ReplayBuffer**

在 `process_turn()` 中，在 LLM call 和 tool call 的关键点推送事件到 ReplayBuffer。

在 LLM call 之前（在 `context_working_set` 之后、provider call 之前）：

```rust
// ReplayBuffer: LLM call
if let Some(ref mut rb) = self.replay_buffer {
    rb.push(crate::trace::replay_buffer::ObservationEvent {
        ts: crate::trace::types::now_ts(),
        kind: crate::trace::replay_buffer::ObservationKind::LlmCall {
            model: self.model.clone(),
            prompt_tokens: 0, // will be updated when response arrives
        },
    });
}
```

在 LLM response 之后：

```rust
if let Some(ref mut rb) = self.replay_buffer {
    if let Some(last) = rb.recent_events(1).firt() {
        if matches!(last.kind, crate::trace::replay_buffer::ObservationKind::LlmCall { .. }) {
            // Update with actual token counts (simplified)
        }
    }
}
```

在 tool call 之前和之后类似。

**简化实现**: 因为 AgentLoop 没有直接的 tool call 回调，我们从 `AgentEvent::ToolStarted` 和 `AgentEvent::ToolFinished` 事件中获取数据。在 `process_turn()` 的 tool loop 中已有这些事件被发送到 `event_tx`，我们可以在此处也推送到 ReplayBuffer。

找到 tool call 的关键代码段（约在 `process_turn` 的 tool loop 中，stream delta 处理前后），插入：

```rust
// ReplayBuffer
if let Some(ref mut rb) = self.replay_buffer {
    // 在 event_tx.send(AgentEvent::ToolStarted { ... }) 附近
    rb.push(ObservationEvent {
        ts: crate::trace::types::now_ts(),
        kind: ObservationKind::ToolCall {
            name: tool_name.clone(),
            input_preview: preview.clone(),
        },
    });
}
```

- [ ] **Step 4: 在 process_turn 末尾生成 self-observation**

在 `process_turn()` 末尾，`turn_span.end()` 之前：

```rust
// Generate self-observation for next turn
if let Some(ref mut rb) = self.replay_buffer {
    self.self_observation = Some(rb.to_self_observation());
}
```

- [ ] **Step 5: 在 build_system_prompt 中注入 self-observation**

在 `build_system_prompt_with_catalog()` 中，在 `parts.join("\n\n")` 之前注入：

```rust
// Self-observation from previous turn
if let Some(ref obs) = self_observation {
    parts.push(format!("\n## Previous Turn Trace\n{}\n", obs));
}
```

需要修改函数签名以接受 `Option<&str>`：

```rust
fn build_system_prompt_with_catalog(
    root: &std::path::Path,
    catalog: &str,
    self_observation: Option<&str>,
) -> String {
```

- [ ] **Step 6: 编译测试**

```bash
cargo build 2>&1
```

- [ ] **Step 7: 运行单元测试**

```bash
cargo test 2>&1 | tail -20
```
Expected: 348+ tests pass (no regressions).

- [ ] **Step 8: Commit**

```bash
git add src/agent.rs src/config.rs
git commit -m "feat(agent): integrate ReplayBuffer for LLM self-observation"
```

---

### Task 10: 端到端验证

**Files:**
- 无新文件

- [ ] **Step 1: 编译完整项目**

```bash
cargo build 2>&1
```

- [ ] **Step 2: 运行完整测试套件**

```bash
cargo test 2>&1 | tail -30
```

- [ ] **Step 3: 验证 CODECODER_TRACE=1 基本功能**

```bash
CODECODER_TRACE=1 cargo test --lib trace:: 2>&1 | tail -20
```

- [ ] **Step 4: 验证 CODECODER_SELF_OBSERVE=1 不破坏现有功能**

```bash
CODECODER_SELF_OBSERVE=1 cargo test 2>&1 | tail -10
```
Expected: no panics.

- [ ] **Step 5: 最终 Commit**

```bash
git add -A
git commit -m "feat(obsrvation): complete full observability system implementation

Phase C-1: trace data completion (AgentGraphEdge, FullIO, CompactionDrop)
Phase C-2: ReplayBuffer with self-observation formatting
Phase C-3: AgentGraph with tree reconstruction
Phase A-1: cc-web SSE trace stream endpoint + frontend
Phase A-2: file heatmap in trace frontend
Phase D-1: agent call tree API endpoint
Phase B-1: LLM self-observation integration"
```