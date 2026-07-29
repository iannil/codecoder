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
        assert_eq!(id.len(), 20, "got: {id} (len={})", id.len());
        assert!(id.starts_with("sp_session_abcd"));
    }
}