//! Observer trait and ObserverSet — multi-observer dispatch for trace events.
//!
//! An `Observer` is a consumer that receives a subset of trace events
//! (span starts, span ends, point events). The `ObserverSet` holds a list
//! of registered observers and dispatches every event to all enabled observers.
//!
//! By default observers run in **soft_fail** mode: if one observer panics or
//! errors, the next observer still receives the event. This prevents a single
//! misbehaving consumer from breaking the entire observability pipe.

use crate::trace::types::*;

/// Observer trait — each implementation handles one consumer's subset of events.
///
/// All methods are no-ops by default; override only what you need.
pub trait Observer: Send {
    /// Whether this observer is currently active. Returning `false` skips
    /// dispatch to this observer without removing it.
    fn enabled(&self) -> bool {
        true
    }

    /// Called when a span starts.
    fn on_span_start(&mut self, _span: &SpanStart) {}

    /// Called when a span ends.
    fn on_span_end(&mut self, _span: &SpanEnd) {}

    /// Called for an instantaneous point event.
    fn on_point(&mut self, _event: &PointEvent) {}

    /// Flush any buffered output. Called at shutdown or at explicit checkpoints.
    fn flush(&mut self) {}
}

/// ObserverSet — holds all registered observers, dispatches events to each.
///
/// # Soft-fail mode
///
/// By default (`soft_fail: true`), a panic in one observer does not prevent
/// subsequent observers from receiving the event. Panics are caught with
/// `std::panic::catch_unwind` and the error is swallowed (logged only via
/// `eprintln!`). When `soft_fail` is `false`, the first panic propagates.
pub struct ObserverSet {
    observers: Vec<Box<dyn Observer + Send>>,
    soft_fail: bool,
}

impl ObserverSet {
    /// Create a new empty ObserverSet with soft_fail enabled.
    pub fn new() -> Self {
        ObserverSet {
            observers: Vec::new(),
            soft_fail: true,
        }
    }

    /// Create a new ObserverSet with a specific soft_fail setting.
    pub fn with_soft_fail(soft_fail: bool) -> Self {
        ObserverSet {
            observers: Vec::new(),
            soft_fail,
        }
    }

    /// Register a new observer.
    pub fn register(&mut self, observer: Box<dyn Observer + Send>) {
        self.observers.push(observer);
    }

    /// Remove all observers.
    pub fn clear(&mut self) {
        self.observers.clear();
    }

    /// Number of registered observers.
    pub fn len(&self) -> usize {
        self.observers.len()
    }

    /// Returns `true` if no observers are registered.
    pub fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }

    /// Dispatch `on_span_start` to all enabled observers.
    pub fn on_span_start(&mut self, span: &SpanStart) {
        for obs in &mut self.observers {
            if !obs.enabled() {
                continue;
            }
            if self.soft_fail {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    obs.on_span_start(span);
                }));
                if let Err(e) = result {
                    let msg = panic_message(&e);
                    eprintln!("[trace] observer panicked in on_span_start: {msg}");
                }
            } else {
                obs.on_span_start(span);
            }
        }
    }

    /// Dispatch `on_span_end` to all enabled observers.
    pub fn on_span_end(&mut self, span: &SpanEnd) {
        for obs in &mut self.observers {
            if !obs.enabled() {
                continue;
            }
            if self.soft_fail {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    obs.on_span_end(span);
                }));
                if let Err(e) = result {
                    eprintln!("[trace] observer panicked in on_span_end: {}", panic_message(&e));
                }
            } else {
                obs.on_span_end(span);
            }
        }
    }

    /// Dispatch `on_point` to all enabled observers.
    pub fn on_point(&mut self, event: &PointEvent) {
        for obs in &mut self.observers {
            if !obs.enabled() {
                continue;
            }
            if self.soft_fail {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    obs.on_point(event);
                }));
                if let Err(e) = result {
                    let msg = panic_message(&e);
                    eprintln!("[trace] observer panicked in on_point: {msg}");
                }
            } else {
                obs.on_point(event);
            }
        }
    }

    /// Flush all observers.
    pub fn flush(&mut self) {
        for obs in &mut self.observers {
            if !obs.enabled() {
                continue;
            }
            if self.soft_fail {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    obs.flush();
                }));
                if let Err(e) = result {
                    let msg = panic_message(&e);
                    eprintln!("[trace] observer panicked in flush: {msg}");
                }
            } else {
                obs.flush();
            }
        }
    }

    // ---------------------------------------------------------------
    // Convenience methods matching existing TraceEmitter API
    // ---------------------------------------------------------------

    /// Emit a turn-start span. The span_id uses a timestamp-based suffix for uniqueness.
    pub fn on_turn_start(&mut self) -> Option<String> {
        let span_id = span_id("turn", crate::trace::types::now_ts() as u64);
        let span = SpanStart {
            span_id: span_id.clone(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: now_ts(),
            meta: serde_json::json!({}),
        };
        self.on_span_start(&span);
        Some(span_id)
    }

    /// Emit an LLM call start span.
    pub fn on_llm_call_start(
        &mut self,
        model: &str,
        prompt_tokens: u32,
        prompt_preview: &str,
    ) -> Option<String> {
        let span_id = format!("llm_{}", now_ts());
        let span = SpanStart {
            span_id: span_id.clone(),
            parent_id: None,
            kind: SpanKind::LlmCall,
            ts: now_ts(),
            meta: serde_json::json!({
                "model": model,
                "prompt_tokens": prompt_tokens,
                "prompt_preview": prompt_preview,
            }),
        };
        self.on_span_start(&span);
        Some(span_id)
    }

    /// Emit an LLM call end span.
    pub fn on_llm_call_end(
        &mut self,
        span_id: &str,
        completion_tokens: u32,
        stop_reason: &str,
        duration_ms: u64,
    ) {
        let span = SpanEnd {
            span_id: span_id.into(),
            ts: now_ts(),
            meta: serde_json::json!({
                "completion_tokens": completion_tokens,
                "stop_reason": stop_reason,
                "duration_ms": duration_ms,
            }),
        };
        self.on_span_end(&span);
    }

    /// Emit a tool call start span.
    pub fn on_tool_start(
        &mut self,
        name: &str,
        input_preview: &str,
        full_input: Option<&str>,
    ) -> Option<String> {
        let span_id = format!("tool_{}", now_ts());
        let preview = full_input.unwrap_or(input_preview);
        let span = SpanStart {
            span_id: span_id.clone(),
            parent_id: None,
            kind: SpanKind::ToolCall,
            ts: now_ts(),
            meta: serde_json::json!({
                "tool": name,
                "input_preview": preview,
            }),
        };
        self.on_span_start(&span);
        Some(span_id)
    }

    /// Emit a tool call end span.
    pub fn on_tool_end(
        &mut self,
        span_id: &str,
        is_error: bool,
        output_preview: &str,
        target_files: &[String],
    ) {
        let span = SpanEnd {
            span_id: span_id.into(),
            ts: now_ts(),
            meta: serde_json::json!({
                "is_error": is_error,
                "output_preview": output_preview,
                "target_files": target_files,
            }),
        };
        self.on_span_end(&span);
    }
}

impl Default for ObserverSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract a human-readable message from a panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

// ---------------------------------------------------------------
// TestObserver for testing
// ---------------------------------------------------------------

#[cfg(test)]
struct TestObserver {
    tx: std::sync::mpsc::Sender<String>,
}

#[cfg(test)]
impl Observer for TestObserver {
    fn on_span_start(&mut self, span: &SpanStart) {
        let _ = self.tx.send(format!("span_start:{}", span.span_id));
    }

    fn on_span_end(&mut self, span: &SpanEnd) {
        let _ = self.tx.send(format!("span_end:{}", span.span_id));
    }

    fn on_point(&mut self, event: &PointEvent) {
        let _ = self.tx.send(format!("point:{}", event.ts));
    }

    fn flush(&mut self) {
        let _ = self.tx.send("flush".into());
    }
}

#[cfg(test)]
struct PanickingObserver;

#[cfg(test)]
impl Observer for PanickingObserver {
    fn on_span_start(&mut self, _span: &SpanStart) {
        panic!("intentional panic in on_span_start");
    }

    fn on_span_end(&mut self, _span: &SpanEnd) {
        panic!("intentional panic in on_span_end");
    }

    fn on_point(&mut self, _event: &PointEvent) {
        panic!("intentional panic in on_point");
    }

    fn flush(&mut self) {
        panic!("intentional panic in flush");
    }
}

// ---------------------------------------------------------------
// Tests
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn observer_set_dispatches_span_end() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let span = SpanEnd {
            span_id: "sp_002".into(),
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_span_end(&span);
        assert_eq!(rx.recv().unwrap(), "span_end:sp_002");
    }

    #[test]
    fn observer_set_dispatches_point_event() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let event = PointEvent {
            kind: EventKind::Notice { text: "hello".into() },
            ts: 42.0,
            meta: serde_json::json!({}),
        };
        set.on_point(&event);
        let received = rx.recv().unwrap();
        assert!(received.starts_with("point:"));
    }

    #[test]
    fn observer_set_flush_calls_all_observers() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx: tx.clone() }));
        set.register(Box::new(TestObserver { tx: tx.clone() }));
        set.flush();
        let mut count = 0;
        while let Ok(msg) = rx.try_recv() {
            assert_eq!(msg, "flush");
            count += 1;
        }
        assert_eq!(count, 2);
    }

    #[test]
    fn observer_set_skips_disabled_observers() {
        struct DisabledObserver;
        impl Observer for DisabledObserver {
            fn enabled(&self) -> bool {
                false
            }
            fn on_span_start(&mut self, _span: &SpanStart) {
                panic!("should not be called");
            }
        }

        let mut set = ObserverSet::new();
        set.register(Box::new(DisabledObserver));
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let span = SpanStart {
            span_id: "sp_003".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        // This should not panic despite DisabledObserver's on_span_start panicking,
        // because it's disabled and never called.
        set.on_span_start(&span);
        assert_eq!(rx.recv().unwrap(), "span_start:sp_003");
    }

    #[test]
    fn soft_fail_swallows_observer_panic() {
        let mut set = ObserverSet::new();
        set.register(Box::new(PanickingObserver));
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let span = SpanStart {
            span_id: "sp_004".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        // PanickingObserver panics, but soft_fail=true catches it and
        // TestObserver still receives the event.
        set.on_span_start(&span);
        assert_eq!(rx.recv().unwrap(), "span_start:sp_004");
    }

    #[test]
    fn soft_fail_panicking_observer_span_end() {
        let mut set = ObserverSet::new();
        set.register(Box::new(PanickingObserver));
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let span = SpanEnd {
            span_id: "sp_005".into(),
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_span_end(&span);
        assert_eq!(rx.recv().unwrap(), "span_end:sp_005");
    }

    #[test]
    fn soft_fail_panicking_observer_point() {
        let mut set = ObserverSet::new();
        set.register(Box::new(PanickingObserver));
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let event = PointEvent {
            kind: EventKind::Notice { text: "test".into() },
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_point(&event);
        let received = rx.recv().unwrap();
        assert!(received.starts_with("point:"));
    }

    #[test]
    fn soft_fail_panicking_observer_flush() {
        let mut set = ObserverSet::new();
        set.register(Box::new(PanickingObserver));
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        set.flush();
        assert_eq!(rx.recv().unwrap(), "flush");
    }

    #[test]
    #[should_panic(expected = "intentional panic in on_span_start")]
    fn hard_fail_propagates_panic() {
        let mut set = ObserverSet::with_soft_fail(false);
        set.register(Box::new(PanickingObserver));
        let span = SpanStart {
            span_id: "sp_006".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_span_start(&span);
    }

    #[test]
    fn observer_set_len_and_is_empty() {
        let mut set = ObserverSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        let (tx, _) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        assert!(!set.is_empty());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn observer_set_clear_removes_all() {
        let mut set = ObserverSet::new();
        let (tx, _) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        assert_eq!(set.len(), 1);
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn observer_set_multiple_observers_all_receive() {
        let mut set = ObserverSet::new();
        let (tx1, rx1) = std::sync::mpsc::channel();
        let (tx2, rx2) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx: tx1 }));
        set.register(Box::new(TestObserver { tx: tx2 }));
        let span = SpanStart {
            span_id: "sp_multi".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_span_start(&span);
        assert_eq!(rx1.recv().unwrap(), "span_start:sp_multi");
        assert_eq!(rx2.recv().unwrap(), "span_start:sp_multi");
    }

    #[test]
    fn default_impl_is_noop() {
        struct NoopObserver;
        impl Observer for NoopObserver {}

        let mut set = ObserverSet::new();
        set.register(Box::new(NoopObserver));
        // Should not panic.
        let span = SpanStart {
            span_id: "sp_noop".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        set.on_span_start(&span);
        set.on_span_end(&SpanEnd {
            span_id: "sp_noop".into(),
            ts: 0.0,
            meta: serde_json::json!({}),
        });
        set.on_point(&PointEvent {
            kind: EventKind::Notice { text: "test".into() },
            ts: 0.0,
            meta: serde_json::json!({}),
        });
        set.flush();
    }

    #[test]
    fn convenience_on_turn_start() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let span_id = set.on_turn_start();
        assert!(span_id.is_some());
        assert_eq!(rx.recv().unwrap(), format!("span_start:{}", span_id.unwrap()));
    }

    #[test]
    fn convenience_on_llm_call() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let id = set.on_llm_call_start("gpt-4o", 100, "hello").unwrap();
        assert_eq!(rx.recv().unwrap(), format!("span_start:{}", id));
        set.on_llm_call_end(&id, 50, "stop", 1234);
        assert_eq!(rx.recv().unwrap(), format!("span_end:{}", id));
    }

    #[test]
    fn convenience_on_tool_call() {
        let mut set = ObserverSet::new();
        let (tx, rx) = std::sync::mpsc::channel();
        set.register(Box::new(TestObserver { tx }));
        let id = set.on_tool_start("read", "file.txt", None).unwrap();
        assert_eq!(rx.recv().unwrap(), format!("span_start:{}", id));
        set.on_tool_end(&id, false, "content", &["file.txt".into()]);
        assert_eq!(rx.recv().unwrap(), format!("span_end:{}", id));
    }

    #[test]
    fn observer_set_default_is_empty() {
        let set = ObserverSet::default();
        assert!(set.is_empty());
    }
}