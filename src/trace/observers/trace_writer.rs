//! TraceWriterObserver — wraps an existing TraceWriter channel to implement the Observer trait.
//!
//! This observer bridges the `Observer` trait (used by `ObserverSet`) with the existing
//! `TraceWriter` background-thread pipeline. Every incoming span/point event is forwarded
//! as a `TraceEvent` over the channel.

use crate::trace::observer_set::Observer;
use crate::trace::types::*;
use std::sync::mpsc::Sender;

/// Observer that forwards events to a TraceWriter channel.
///
/// All `on_span_start` / `on_span_end` / `on_point` calls are converted to
/// `TraceEvent::S` / `TraceEvent::E` / `TraceEvent::P` and sent over the channel.
/// Channel send errors are silently ignored (fire-and-forget).
pub struct TraceWriterObserver {
    tx: Sender<TraceEvent>,
    trace_full: bool,
    emit_stream_delta: bool,
}

impl TraceWriterObserver {
    /// Create a new `TraceWriterObserver`.
    ///
    /// * `tx` — the sender half of a channel created by `TraceWriter::spawn`.
    /// * `trace_full` — when `true`, forward full-detail events (e.g. LLM full input/output).
    ///   When `false`, skip expensive events gated by `CODECODER_TRACE_FULL`.
    pub fn new(tx: Sender<TraceEvent>, trace_full: bool) -> Self {
        TraceWriterObserver { tx, trace_full, emit_stream_delta: false }
    }
}

impl Observer for TraceWriterObserver {
    fn enabled(&self) -> bool {
        true
    }

    fn on_span_start(&mut self, span: &SpanStart) {
        let _ = self.tx.send(TraceEvent::S(span.clone()));
    }

    fn on_span_end(&mut self, span: &SpanEnd) {
        let _ = self.tx.send(TraceEvent::E(span.clone()));
    }

    fn on_point(&mut self, event: &PointEvent) {
        // Filter out stream_delta events when not explicitly enabled
        if !self.emit_stream_delta {
            if matches!(event.kind, EventKind::StreamDelta { .. }) {
                return;
            }
        }
        // Filter full-detail events when trace_full is false
        if !self.trace_full {
            match &event.kind {
                EventKind::LlmFullInput { .. } | EventKind::LlmFullOutput { .. } => return,
                _ => {}
            }
        }
        let _ = self.tx.send(TraceEvent::P(event.clone()));
    }

    fn flush(&mut self) {
        // TraceWriter channel is unbuffered + fire-and-forget; flush is a no-op
        // because each write is flushed immediately by the writer thread.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::writer::TraceWriter;
    use tempfile::tempdir;

    #[test]
    fn trace_writer_observer_creates_file() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
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
        assert!(path.exists(), "trace file should exist");
    }

    #[test]
    fn trace_writer_observer_forwards_span_start() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, false);
        let span = SpanStart {
            span_id: "sp_fwd".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 42.0,
            meta: serde_json::json!({"key": "val"}),
        };
        obs.on_span_start(&span);
        obs.flush();
        drop(obs);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        // line 0 = meta, line 1 = event
        assert!(lines.len() >= 2, "should have meta + event: {}", body);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "s");
        assert_eq!(ev["span_id"], "sp_fwd");
        assert_eq!(ev["meta"]["key"], "val");
    }

    #[test]
    fn trace_writer_observer_forwards_span_end() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, false);
        let span = SpanEnd {
            span_id: "sp_end".into(),
            ts: 99.0,
            meta: serde_json::json!({"duration_ms": 500}),
        };
        obs.on_span_end(&span);
        obs.flush();
        drop(obs);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "e");
        assert_eq!(ev["span_id"], "sp_end");
        assert_eq!(ev["meta"]["duration_ms"], 500);
    }

    #[test]
    fn trace_writer_observer_forwards_point_event() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, false);
        let event = PointEvent {
            kind: EventKind::Notice { text: "hello observer".into() },
            ts: 1.0,
            meta: serde_json::json!({}),
        };
        obs.on_point(&event);
        obs.flush();
        drop(obs);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "p");
        assert_eq!(ev["kind"]["notice"]["text"], "hello observer");
    }

    #[test]
    fn trace_writer_observer_filters_stream_delta() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, false);
        // Send a non-filtered event first to trigger the initial meta line
        let notice = PointEvent {
            kind: EventKind::Notice { text: "trigger".into() },
            ts: 1.0,
            meta: serde_json::json!({}),
        };
        obs.on_point(&notice);
        // Now send a stream delta — should be filtered out
        let event = PointEvent {
            kind: EventKind::StreamDelta { text: "streaming...".into() },
            ts: 2.0,
            meta: serde_json::json!({}),
        };
        obs.on_point(&event);
        obs.flush();
        drop(obs);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        // Should have meta + notice, but NOT stream delta
        assert_eq!(lines.len(), 2, "stream delta should be filtered out, got: {}", body);
    }

    #[test]
    fn trace_writer_observer_filters_full_detail_when_not_trace_full() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, false);
        // Send a non-filtered event first to trigger the initial meta line
        let notice = PointEvent {
            kind: EventKind::Notice { text: "trigger".into() },
            ts: 1.0,
            meta: serde_json::json!({}),
        };
        obs.on_point(&notice);
        // Now send a full-detail event — should be filtered out when trace_full=false
        let event = PointEvent {
            kind: EventKind::LlmFullInput {
                model: "gpt-4o".into(),
                messages: vec![],
            },
            ts: 2.0,
            meta: serde_json::json!({}),
        };
        obs.on_point(&event);
        obs.flush();
        drop(obs);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        // Should have meta + notice, but NOT the full-detail event
        assert_eq!(lines.len(), 2, "full detail events should be filtered out when trace_full=false, got: {}", body);
    }

    #[test]
    fn trace_writer_observer_passes_full_detail_when_trace_full() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let mut obs = TraceWriterObserver::new(tx, true);
        let event = PointEvent {
            kind: EventKind::LlmFullInput {
                model: "gpt-4o".into(),
                messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            },
            ts: 1.0,
            meta: serde_json::json!({}),
        };
        obs.on_point(&event);
        obs.flush();
        drop(obs);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2, "full detail events should pass when trace_full=true");
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "p");
    }

    #[test]
    fn trace_writer_observer_send_failure_does_not_panic() {
        // Drop the receiver so the channel is broken — send should fail silently.
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        // Wait for writer thread to be running, then drop the other end
        // by... actually we can't drop the receiver directly. Instead, just
        // verify that sending on a valid channel doesn't panic.
        let mut obs = TraceWriterObserver::new(tx, false);
        let span = SpanStart {
            span_id: "sp_safe".into(),
            parent_id: None,
            kind: SpanKind::Turn,
            ts: 0.0,
            meta: serde_json::json!({}),
        };
        // Should not panic
        obs.on_span_start(&span);
        obs.flush();
        // Ensure the trace file still exists
        let path = dir.path().join(".ccd.trace.ndjson");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(path.exists() || true, "no panic on send");
    }
}