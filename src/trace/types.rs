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
    /// compaction 前的完整上下文保存 span
    ContextSnapshot,
    /// 完整推理链 span（区别于流式 Reasoning）
    FullReasoning,
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

#[derive(Debug, Clone, Serialize)]
pub struct AgentGraphEdge {
    pub parent_span_id: String,
    pub child_span_id: String,
    pub label: String,
    pub launch_seq: u32,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Spawned,
    Running,
    Done,
    Failed,
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
    /// 子 agent 调用边（用于重建调用树）
    AgentGraphEdge(AgentGraphEdge),
    /// LLM 完整 input（CODECODER_TRACE_FULL=1 门控）
    LlmFullInput { model: String, messages: Vec<serde_json::Value> },
    /// LLM 完整 output（CODECODER_TRACE_FULL=1 门控）
    LlmFullOutput { model: String, content: String },
    /// Compaction 丢弃的内容摘要
    CompactionDrop { span_id: String, dropped_bytes: u64, summary: String },
    // === New variants (2026-07-29 unified observability) ===
    UserInput {
        source: MessageSource,
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
        kind: String,
        attempt: u32,
        max_retries: u32,
        error: String,
    },
    ProcessIdentity {
        pid: u32,
        agent_type: String,
        session_id: String,
    },
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