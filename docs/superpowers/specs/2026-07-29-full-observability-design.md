# CodeCoder 全面观测系统 — 设计文档

**日期:** 2026-07-29
**状态:** Draft
**优先级:** C（全记录）→ A（可视化）→ D（多 agent 关联）→ B（自省回路）

## 概述

以"不碰内核设计原则"为底线，在 codecoder 已有的 Trace 系统（2026-07-29 实现）基础上，构建一套完整、精准、细粒度、高性能的观测方案。目标是：

- **任何人**可以实时观察 agent 的执行过程（Web 仪表盘）
- **任何 agent**（包括自己）可以事后或实时读取自己的执行 trace，用于自我修正
- **任何执行过程**完整可复盘（保留完整推理链、完整 IO、compaction 丢弃记录）
- **多 agent 执行**关联可追溯（parent sub-agent 调用树）

## 核心架构

```
AgentLoop (process_turn)
  │
  ├── TraceEmitter ──→ TraceWriter ──→ .ccd.trace.ndjson (唯一事实来源)
  │                           │
  │                   [SSE tail-follower]  ← cc-web (Trace Stream)
  │
  ├── ReplayBuffer (内存环形缓冲区, 200 事件)
  │     └── to_self_observation() → LLM 下一轮 system prompt
  │
  └── AgentGraph (内存维护, 跟踪 sub-agent 树)
        └── render_for_llm() → LLM 可见的 agent 调用树
```

### 设计原则

1. **`.ccd.trace.ndjson` 是唯一事实来源** — ReplayBuffer 和 AgentGraph 是内存加速结构，可从 trace 重建
2. **零侵入内核** — 所有修改在 `src/trace/` 和 `src/visual/` 下，`src/agent.rs` 只增加钩子
3. **渐进交付** — 每个 phase 独立可验证，phase 之间不阻塞
4. **门控安全** — 完整 IO 记录和自省功能默认关闭，由环境变量启用

## Phase C-1: Trace 数据补全

### 目标

让 trace 记录完整的执行信息，支持事后完整复盘。

### 新增类型 (`src/trace/types.rs`)

```rust
// 新增：AgentGraph 边事件
pub struct AgentGraphEdge {
    pub parent_span_id: String,
    pub child_span_id: String,
    pub label: String,         // sub-agent 的任务描述
    pub launch_seq: u32,
}

// 扩展 SpanKind
pub enum SpanKind {
    Turn, LlmCall, ToolCall, SubAgent, Milestone, Reasoning, Compaction,
    ContextSnapshot,     // compaction 前的完整上下文保存
    FullReasoning,       // 完整推理链（区别于流式 Reasoning）
}

// 扩展 EventKind
pub enum EventKind {
    // ... 已有 ...
    AgentGraphEdge(AgentGraphEdge),
    LlmFullInput { model: String, messages: Vec<serde_json::Value> },    // CODECODER_TRACE_FULL=1 门控
    LlmFullOutput { model: String, content: String },
    CompactionDrop { span_id: String, dropped_bytes: u64, summary: String },
}
```

### 变更说明

**`src/trace/emitter.rs`:**
- `on_llm_call_start()`: CODECODER_TRACE_FULL=1 时记录完整 messages
- `on_tool_start()`: 完整模式时记录完整 input（不再截断）
- `emit_reasoning()`: 新增，记录完整 reasoning text
- `emit_compaction_drop()`: 新增，记录 compaction 丢弃的上下文

**`src/trace/writer.rs`**
- 新增 CODECODER_TRACE_FULL 环境变量控制完整模式
- 完整模式 rotation 阈值提升到 50 MB
- meta header 增加 `full: true` 标记

**`src/trace/reader.rs`**:
- 支持读取 AgentGraphEdge 点事件，重建 agent 树
- `render_for_llm()` 输出完整推理链（可选）
- 新增 `render_full_trace()` 输出完整的逐事件回放文本

## Phase C-2: ReplayBuffer

### 目标

在内存中维护最近 N 个事件的环形缓冲区，agent 可在 turn 结束时读取格式化摘要用于自省。

### 新增 `src/trace/replay_buffer.rs`

```rust
pub struct ReplayBuffer {
    buffer: VecDeque<ObservationEvent>,
    capacity: usize,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self;
    pub fn push(&mut self, event: ObservationEvent);
    pub fn to_self_observation(&self) -> String;     // → LLM prompt
    pub fn filter_by_file(&self, path: &str) -> Vec<&ObservationEvent>;
    pub fn filter_by_kind(&self, kind: SpanKind) -> Vec<&ObservationEvent>;
    pub fn recent_events(&self, n: usize) -> Vec<&ObservationEvent>;
    pub fn stats_since(&self, since_ts: f64) -> ObservationStats;
}

pub struct ObservationStats {
    pub llm_calls: usize,
    pub tool_calls: usize,
    pub errors: usize,
    pub file_reads: usize,
    pub file_edits: usize,
    pub total_tokens: u32,
    pub duration_ms: u64,
}
```

### 自省格式化输出 (`to_self_observation`)

```
## 上轮执行回溯 (2.3s, 1 LLM call, 3 tools, 0 错误)

### 工具调用序列
  1. read_file: src/main.rs (73ms, 成功)
  2. grep: "TODO" src/lib.rs (12ms, 成功)
  3. edit_file: src/main.rs (149ms, 成功, 42 bytes)

### 文件 touch
  src/main.rs: [Read, Edit]
  src/lib.rs: [Hit]

### 权限检查
  read_file: auto_granted
  edit_file: auto_granted

### token 消耗
  模型: gpt-4o | 1500 prompt + 450 completion = 1950 tokens
```

## Phase C-3: AgentGraph

### 目标

记录 sub-agent 的 spawn 和 result 关联，支持跨 agent 的 trace 导航。

### 新增 `src/trace/agent_graph.rs`

```rust
pub struct AgentGraph {
    nodes: Vec<AgentNode>,
    edges: Vec<AgentEdge>,
}

pub struct AgentNode {
    pub span_id: String,
    pub agent_id: String,        // sub-agent 的 session ID
    pub label: String,
    pub parent_span_id: Option<String>,
    pub launch_seq: u32,
    pub status: AgentStatus,
    pub summary: Option<String>,
}

pub enum AgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AgentGraph {
    pub fn from_trace(reader: &TraceReader) -> io::Result<Self>;
    pub fn render_tree(&self) -> String;        // ASCII 树
    pub fn render_for_llm(&self) -> String;
    pub fn agent_by_id(&self, id: &str) -> Option<&AgentNode>;
}
```

### Trace 中的表示

```json
{"type":"p","kind":"agent_graph_edge","ts":...,
 "meta":{"parent_span_id":"sp_abc_0005","child_span_id":"sp_abc_0010",
         "label":"refactor main.rs to use async","launch_seq":1}}

{"type":"s","span_id":"sp_abc_0010","parent_id":"sp_abc_0005",
 "kind":"sub_agent","ts":...,
 "meta":{"agent_id":"agent_xyz","label":"refactor main.rs"}}
```

## Phase A-1: cc-web Trace 实时流

### 目标

`cc-web` 新增 trace 页面，通过 SSE 实时推送 `.ccd.trace.ndjson` 的追加内容。

### 新增端点

`/api/v1/trace/stream` — SSE 端点，监听 `.ccd.trace.ndjson` 的文件追加

**实现**: `src/visual/trace_stream.rs` — 用 `notify` crate 监听文件修改事件，读取新行并推送 SSE

### 前端页面 (`static/trace.html`)

**布局**:
```
┌──────────────────────────────────────────────────────────────┐
│  Trace 回放   [▶ 播放] [� 暂停] [� 跳过]  速度: [1x▾] │
├──────────────────────────────────────────────────────────────┤
│  Timeline:                                           │
│  ┌──────────────────────────────────────────────┐          │
│  │ ████░░████████░░██░░████████████░░████░░░░░░░░░░ │   │
│  │ ← LLM call (蓝)  Tool call (绿)  错误 (红)  →   │   │
│  └─────────────────────────────────────────────┘          │
│                                                       │
│  ┌ 2.3s  Turn #5 ───────────────────────────┐          │
│  │ ├── LlmCall: gpt-4o (1.1s, 1500→450 tokens)    │   │
│  │ ├── ToolCall: read_file: src/main.rs (73ms)     │   │
│  │ ├── ToolCall: edit_file: src/main.rs (149ms)    │   │
│  │ └── FileTouch: src/main.rs [Read, Edit]         │   │
│  └──────────────────────────────────────────────┘          │
│                                                       │
│  ┌ 1.2s  Turn #4 ───────────────────────────┐          │
│  │ ...                                           │   │
│  └──────────────────────────────────────────────┘          │
│                                                       │
├──────────────────────────────────────────────┤               │
│  文件热力图                                         │
│  ┌──────────────────────────────────────────────┐          │
│  │ src/main.rs: █████████ (Read 2, Edit 1)           │   │
│  │ src/lib.rs:  ████ (Read 1)                       │   │
│  │ tests/test.rs: ██ (Hit 1)                        │   │
│  └────────────────────────────────────────────┘          │
├──────────────────────────────────────────────┤               │
│  统计栏                                             │
│  总耗时: 42.3s | LLM 调用: 12 | 工具: 45 | 错误: 2   │
│  文件 touch: 18 | tokens: 45,000                     │
└─────────────────────────────────────────────────────┘
```

**实现**: 纯 HTML + vanilla JS（沿用 cc-web 的无构建前端原则），通过 EventSource 连接 `/api/v1/trace/stream`

## Phase A-2: 代码热力图

### 目标

可视化 agent 在代码库中的"注意力分布"。

### 新增端点

`GET /api/v1/trace/heatmap` — 返回文件 touch 频率 JSON:
```json
{
  "files": [
    {"path": "src/main.rs", "read": 3, "edit": 2, "hit": 1},
    {"path": "src/lib.rs", "read": 1, "edit": 0, "hit": 4}
  ]
}
```

### 前端

用 D3 的 bubble chart 或 treemap 展示文件 touch 频率：
- 颜色代表 touch 类型：蓝色 = read，琥珀色 = edit，灰色 = hit
- 大小代表 touch 频率
- ���选可切换为 treemap 视图（更适合文件层级深的项目）

## Phase D-1: agent 调用树可视化

### 目标

在 cc-web 中展示 sub-agent 的调用树。

### 新增端点

`GET /api/v1/trace/agents` — 返回 AgentGraph JSON

### 前端：折叠树

```
┌── Main Agent ───────────────────────────────────────────┐
│  ├─┬ sub-agent: "Refactor main.rs" (sp_abc_0010)     │
│  │ ├── Turn #3 (1.2s)                                │
│  │ ├── Turn #4 (2.1s)                                │
│  │ └── Turn #5 (0.8s)  ✓ Completed                   │
│  │                                                    │
│  ├─┬ sub-agent: "Add tests" (sp_abc_0020)            │
│  │ ├── Turn #7 (3.4s)                                │
│  │ └── Turn #8 (1.1s)  ✗ Failed (test error)         │
│  └───────────────────────────────────────────────     │
└───────────────────────────────────────────────       ┘
```

点击任一 sub-agent 节点可展开其内部的 turn 序列，点击 turn 可跳转到 trace timeline 的对应位置。

## Phase B-1: LLM 自省集成

### 目标

agent 在执行过程中能读取自己的 trace，用于自我修正。

### 变更：`src/agent.rs`

在 `process_turn()` 末尾调用 `ReplayBuffer::to_self_observation()`：

```rust
// process_turn 末尾
if let Some(ref mut replay) = self.replay_buffer {
    let observation = replay.to_self_observation();
    if !observation.is_empty() {
        self.self_observation = Some(observation);
    }
}

// 构建 system prompt 时
if let Some(ref obs) = self.self_observation {
    system_parts.push(format!(
        "\n## Previous Turn Trace\n{}\n",
        obs
    ));
}
```

### 门控

由 `CODECODER_SELF_OBSERVE=1` 环境变量控制，默认关闭。可通过 `codecoder.json` 的 `self_observe: true` 配置。

## 文件变更总清单

| Phase | 文件 | 动作 | 说明 |
|-------|------|------|------|
| C-1 | `src/trace/types.rs` | 修改 | 增加 AgentGraphEdge、LlmFullInput/Output、CompactionDrop |
| C-1 | `src/trace/emitter.rs` | 修改 | 完整 IO 记录、compaction drop 记录 |
| C-1 | `src/trace/writer.rs` | 修改 | FULL 模式 rotation 阈值 |
| C-1 | `src/trace/reader.rs` | 修改 | 支持 AgentGraphEdge、完整推理链 |
| C-2 | **`src/trace/replay_buffer.rs`** | **新增** | 环形缓冲区 + 自省格式化 |
| C-3 | **`src/trace/agent_graph.rs`** | **新增** | agent 树重建 + LLM 渲染 |
| C-3 | `src/trace/mod.rs` | 修改 | 导出新模块 |
| A-1 | `src/visual/http_server.rs` | 修改 | 新增 `/api/v1/trace/stream` SSE 端点 |
| A-1 | **`src/visual/trace_stream.rs`** | **新增** | trace 文件尾部跟随器 |
| A-1 | **`static/trace.html`** | **新增** | trace 回放前端页面 |
| A-2 | `src/visual/http_server.rs` | 修改 | 新增 `/api/v1/trace/heatmap` |
| A-2 | `src/visual/http_server.rs` | 修改 | 热力图前端 JS |
| D-1 | `src/visual/http_server.rs` | 修改 | 新增 `/api/v1/trace/agents` |
| D-1 | `src/visual/http_server.rs` | 修改 | agent 树前端 JS |
| B-1 | `src/agent.rs` | 修改 | process_turn 注入 self-observation |
| B-1 | `src/config.rs` | 修改 | 增加 self_observe 配置 |

## 测试策略

| 层 | 内容 | 方式 |
|----|------|------|
| 单元 | ReplayBuffer 环形行为、to_self_observation 格式化 | `src/trace/replay_buffer.rs` 内联测试 |
| 单元 | AgentGraph 从 trace 重建树 | `src/trace/agent_graph.rs` 内联测试 |
| 单元 | TraceStream 文件尾部跟随 | mock 文件写入 + SSE 验证 |
| 集成 | 完整模式 trace 写入 + reader 重建 | 先写 trace，再用 Reader 读取验证 |
| 集成 | cc-web trace 端点返回有效 SSE | 启动测试 HTTP server，EventSource 连接 |

## 依赖增量

```
cargo.toml:
  notify = "6"         # 文件监听（已在 visual 依赖中）
  # 无其他新依赖
```