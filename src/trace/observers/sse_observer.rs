//! SseObserver — maintains a file touch heatmap for SSE visualization.
//!
//! This observer tracks reads, edits, and hits per file path, building a heatmap
//! that can be served via SSE in Phase 5. For now it is a pure data-collection
//! observer with no network component.
//!
//! The `heatmap()` accessor returns a reference to the internal `HashMap`,
//! which can be serialized to JSON for the SSE endpoint.

use std::collections::HashMap;

use serde::Serialize;

use crate::trace::observer_set::Observer;
use crate::trace::types::*;

/// Per-file touch statistics for the SSE heatmap.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FileTouchStats {
    /// Number of read touches.
    pub reads: u64,
    /// Number of edit/create touches.
    pub edits: u64,
    /// Number of hit touches (navigation/cursor hits).
    pub hits: u64,
    /// Timestamp of the most recent touch.
    pub last_touch_ts: f64,
    /// Debug representation of the most recent touch type.
    pub last_touch: String,
}

/// Observer that maintains a file touch heatmap.
///
/// Collects `FileTouch` point events into a `HashMap<String, FileTouchStats>`,
/// counting reads, edits, and hits per file path. The `heatmap()` method
/// provides access to the accumulated data.
///
/// # SSE integration (Phase 5)
///
/// The `router_tx` field is reserved for the future SSE event router channel.
/// It is not used in this phase.
pub struct SseObserver {
    touches: HashMap<String, FileTouchStats>,
    /// Reserved for future SSE event router channel (Phase 5).
    #[allow(dead_code)]
    router_tx: Option<std::sync::mpsc::Sender<()>>,
}

impl SseObserver {
    /// Create a new `SseObserver` with an empty heatmap.
    pub fn new() -> Self {
        SseObserver {
            touches: HashMap::new(),
            router_tx: None,
        }
    }

    /// Return a reference to the file touch heatmap.
    pub fn heatmap(&self) -> &HashMap<String, FileTouchStats> {
        &self.touches
    }
}

impl Default for SseObserver {
    fn default() -> Self {
        SseObserver::new()
    }
}

impl Observer for SseObserver {
    fn on_point(&mut self, event: &PointEvent) {
        if let EventKind::FileTouch { path, touch, .. } = &event.kind {
            let stats = self.touches.entry(path.clone()).or_default();
            match touch {
                TouchType::Read => {
                    stats.reads += 1;
                }
                TouchType::Edit | TouchType::Create => {
                    stats.edits += 1;
                }
                TouchType::Hit => {
                    stats.hits += 1;
                }
                TouchType::Delete => {
                    // Delete touches are noted but do not increment counters.
                }
            }
            stats.last_touch_ts = event.ts;
            stats.last_touch = format!("{:?}", touch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::types::*;

    #[test]
    fn sse_observer_tracks_touches() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "src/main.rs".into(),
                touch: TouchType::Read,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        obs.on_point(&PointEvent {
            ts: 2.0,
            kind: EventKind::FileTouch {
                path: "src/main.rs".into(),
                touch: TouchType::Edit,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        let heatmap = obs.heatmap();
        assert_eq!(heatmap.len(), 1);
        assert_eq!(heatmap["src/main.rs"].reads, 1);
        assert_eq!(heatmap["src/main.rs"].edits, 1);
    }

    #[test]
    fn sse_observer_tracks_hits() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "src/lib.rs".into(),
                touch: TouchType::Hit,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        let heatmap = obs.heatmap();
        assert_eq!(heatmap["src/lib.rs"].hits, 1);
    }

    #[test]
    fn sse_observer_tracks_multiple_files() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "a.rs".into(),
                touch: TouchType::Read,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        obs.on_point(&PointEvent {
            ts: 2.0,
            kind: EventKind::FileTouch {
                path: "b.rs".into(),
                touch: TouchType::Edit,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        let heatmap = obs.heatmap();
        assert_eq!(heatmap.len(), 2);
        assert_eq!(heatmap["a.rs"].reads, 1);
        assert_eq!(heatmap["b.rs"].edits, 1);
    }

    #[test]
    fn sse_observer_accumulates_counts() {
        let mut obs = SseObserver::new();
        for _ in 0..5 {
            obs.on_point(&PointEvent {
                ts: 1.0,
                kind: EventKind::FileTouch {
                    path: "src/main.rs".into(),
                    touch: TouchType::Read,
                    lines: None,
                },
                meta: serde_json::json!({}),
            });
        }
        assert_eq!(obs.heatmap()["src/main.rs"].reads, 5);
    }

    #[test]
    fn sse_observer_delete_does_not_increment() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "gone.rs".into(),
                touch: TouchType::Delete,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        let stats = &obs.heatmap()["gone.rs"];
        assert_eq!(stats.reads, 0);
        assert_eq!(stats.edits, 0);
        assert_eq!(stats.hits, 0);
    }

    #[test]
    fn sse_observer_last_touch_ts() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 42.0,
            kind: EventKind::FileTouch {
                path: "f.rs".into(),
                touch: TouchType::Read,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        assert!((obs.heatmap()["f.rs"].last_touch_ts - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sse_observer_ignores_non_file_events() {
        let mut obs = SseObserver::new();
        // Non-file-touch events should not affect the heatmap
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::Notice {
                text: "hello".into(),
            },
            meta: serde_json::json!({}),
        });
        assert!(obs.heatmap().is_empty());
    }

    #[test]
    fn sse_observer_default_is_empty() {
        let obs = SseObserver::default();
        assert!(obs.heatmap().is_empty());
    }

    #[test]
    fn sse_observer_new_is_empty() {
        let obs = SseObserver::new();
        assert!(obs.heatmap().is_empty());
    }

    #[test]
    fn file_touch_stats_serialize() {
        let stats = FileTouchStats {
            reads: 3,
            edits: 2,
            hits: 1,
            last_touch_ts: 123.0,
            last_touch: "Edit".into(),
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"reads\":3"));
        assert!(json.contains("\"edits\":2"));
        assert!(json.contains("\"hits\":1"));
        assert!(json.contains("\"last_touch_ts\":123.0"));
        assert!(json.contains("\"last_touch\":\"Edit\""));
    }

    #[test]
    fn file_touch_stats_clone() {
        let stats = FileTouchStats {
            reads: 1,
            edits: 2,
            hits: 3,
            last_touch_ts: 0.0,
            last_touch: "Read".into(),
        };
        let cloned = stats.clone();
        assert_eq!(cloned.reads, 1);
        assert_eq!(cloned.edits, 2);
        assert_eq!(cloned.hits, 3);
    }

    #[test]
    fn sse_observer_updates_last_touch() {
        let mut obs = SseObserver::new();
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "f.rs".into(),
                touch: TouchType::Read,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        assert_eq!(obs.heatmap()["f.rs"].last_touch, "Read");
        obs.on_point(&PointEvent {
            ts: 2.0,
            kind: EventKind::FileTouch {
                path: "f.rs".into(),
                touch: TouchType::Edit,
                lines: None,
            },
            meta: serde_json::json!({}),
        });
        assert_eq!(obs.heatmap()["f.rs"].last_touch, "Edit");
    }
}