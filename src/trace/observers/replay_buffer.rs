//! ReplayBufferObserver — translates trace EventKind events into lightweight
//! ObservationKind events and stores them in a ring buffer for LLM self-observation.
//!
//! This observer is the bridge between the trace system's rich event model and the
//! replay buffer's compact observation model. It can be filtered with a subscribe
//! mask (string prefixes on the Debug representation of EventKind) to reduce noise.

use crate::trace::observer_set::Observer;
use crate::trace::replay_buffer::{ObservationEvent, ObservationKind, ReplayBuffer};
use crate::trace::types::*;

/// An Observer that translates `EventKind` into `ObservationKind` and pushes them
/// into a ring buffer. Useful for LLM self-observation: the buffer can be queried
/// for recent activity, stats, and formatted summaries.
///
/// # Subscribe Mask
///
/// When `subscribe_mask` is `Some`, only events whose `Debug` representation starts
/// with one of the given prefixes are accepted. When `None` (the default), all events
/// that have a valid `ObservationKind` translation are accepted.
pub struct ReplayBufferObserver {
    buffer: ReplayBuffer,
    subscribe_mask: Option<Vec<&'static str>>,
}

impl ReplayBufferObserver {
    /// Create a new observer with the given ring buffer capacity.
    pub fn new(capacity: usize) -> Self {
        ReplayBufferObserver {
            buffer: ReplayBuffer::new_with_capacity(capacity),
            subscribe_mask: None,
        }
    }

    /// Number of events currently in the buffer.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns `true` if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Set a subscribe mask: only events whose Debug representation starts with
    /// one of the given prefixes will be accepted. Pass `None` to clear the mask.
    pub fn set_subscribe_mask(&mut self, kinds: Option<Vec<&'static str>>) {
        self.subscribe_mask = kinds;
    }

    /// Access the underlying buffer (immutable).
    pub fn buffer(&self) -> &ReplayBuffer {
        &self.buffer
    }

    /// Access the underlying buffer (mutable).
    pub fn buffer_mut(&mut self) -> &mut ReplayBuffer {
        &mut self.buffer
    }

    /// Clear all events from the buffer.
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

impl Observer for ReplayBufferObserver {
    fn on_point(&mut self, event: &PointEvent) {
        // Apply subscribe mask if set
        if let Some(ref mask) = self.subscribe_mask {
            let kind_str = format!("{:?}", event.kind);
            if !mask.iter().any(|m| kind_str.starts_with(m)) {
                return;
            }
        }

        // Translate EventKind to ObservationKind
        if let Some(obs_kind) = translate_to_observation(&event.kind) {
            self.buffer.push(ObservationEvent {
                ts: event.ts,
                kind: obs_kind,
            });
        }
    }
}

/// Translate a trace `EventKind` into a lightweight `ObservationKind`.
///
/// Returns `None` for events that are not relevant for self-observation
/// (e.g. streaming deltas, structural graph edges, full LLM I/O).
fn translate_to_observation(kind: &EventKind) -> Option<ObservationKind> {
    match kind {
        // --- Direct mappings ---
        EventKind::Notice { text } => Some(ObservationKind::Notice { text: text.clone() }),

        EventKind::FileTouch { path, touch, .. } => {
            let touch_str = match touch {
                TouchType::Read | TouchType::Hit => "read",
                TouchType::Edit | TouchType::Create => "edit",
                TouchType::Delete => "delete",
            };
            Some(ObservationKind::FileTouch {
                path: path.clone(),
                touch: touch_str.into(),
            })
        }

        EventKind::PermissionCheck { key, decision } => {
            Some(ObservationKind::Permission {
                key: key.clone(),
                granted: matches!(
                    decision,
                    PermissionDecision::Granted | PermissionDecision::AutoGranted
                ),
            })
        }

        EventKind::PermissionFull { key, decision, .. } => {
            Some(ObservationKind::Permission {
                key: key.clone(),
                granted: matches!(
                    decision,
                    PermissionDecision::Granted | PermissionDecision::AutoGranted
                ),
            })
        }

        // --- Retry -> Error ---
        EventKind::RetryEvent { kind, attempt, .. } => Some(ObservationKind::Error {
            message: format!("retry {kind} attempt #{attempt}"),
        }),

        // --- Sub-agent lifecycle and results ---
        EventKind::SubAgentLifecycle {
            agent_id, status, ..
        } => Some(ObservationKind::SubAgent {
            label: agent_id.clone(),
            status: format!("{:?}", status),
        }),

        EventKind::SubAgentResult { agent_id, summary } => Some(ObservationKind::SubAgent {
            label: agent_id.clone(),
            status: summary.clone(),
        }),

        // --- Compaction ---
        EventKind::CompactionDrop { dropped_bytes, .. } => Some(ObservationKind::Compaction {
            dropped_bytes: *dropped_bytes,
        }),

        EventKind::ContextSnapshot {
            before_tokens,
            after_tokens,
            ..
        } => {
            let dropped = before_tokens.saturating_sub(*after_tokens);
            Some(ObservationKind::Compaction { dropped_bytes: dropped })
        }

        // --- User messages / input ---
        EventKind::UserMessage { source: _, summary } => {
            Some(ObservationKind::UserMessage { summary: summary.clone() })
        }

        EventKind::UserInput { source: _, preview, .. } => {
            Some(ObservationKind::UserMessage { summary: preview.clone() })
        }

        // --- Tool calls ---
        EventKind::ToolCallBegin { name, args } => {
            let preview = serde_json::to_string(args).unwrap_or_default();
            Some(ObservationKind::ToolCall {
                name: name.clone(),
                input_preview: preview,
            })
        }

        EventKind::ToolCallEnd {
            name: _,
            is_error,
            duration_ms,
            output_preview,
            ..
        } => Some(ObservationKind::ToolEnd {
            is_error: *is_error,
            output_preview: output_preview.clone(),
            duration_ms: *duration_ms,
        }),

        // --- Milestone transitions ---
        EventKind::MilestoneStatus {
            id,
            title,
            old_status,
            new_status,
        } => Some(ObservationKind::Notice {
            text: format!("milestone #{id} ({title}): {old_status} \u{2192} {new_status}"),
        }),

        // --- Workgraph / exit / process identity -> Notice ---
        EventKind::WorkgraphStatus {
            total,
            pending,
            done,
        } => Some(ObservationKind::Notice {
            text: format!(
                "workgraph: {total} total, {pending} pending, {done} done"
            ),
        }),

        EventKind::ExitCode { code } => Some(ObservationKind::Notice {
            text: format!("exit code: {code}"),
        }),

        EventKind::ProcessIdentity { pid, agent_type, .. } => Some(ObservationKind::Notice {
            text: format!("process {pid} ({agent_type})"),
        }),

        // --- Events skipped for self-observation ---
        EventKind::StreamDelta { .. }
        | EventKind::AgentGraphEdge(_)
        | EventKind::LlmFullInput { .. }
        | EventKind::LlmFullOutput { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::trace::types::*;

    #[test]
    fn replay_buffer_observer_collects_events() {
        let mut obs = ReplayBufferObserver::new(100);
        let event = PointEvent {
            ts: 1.0,
            kind: EventKind::Notice {
                text: "hello".into(),
            },
            meta: serde_json::json!({}),
        };
        obs.on_point(&event);
        assert_eq!(obs.len(), 1);
    }

    #[test]
    fn notice_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::Notice {
                text: "hello".into(),
            },
            meta: serde_json::json!({}),
        });
        assert_eq!(obs.len(), 1);
        let events = obs.buffer().recent_events(1);
        assert!(matches!(events[0].kind, ObservationKind::Notice { .. }));
    }

    #[test]
    fn file_touch_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "src/main.rs".into(),
                touch: TouchType::Read,
                lines: None,
                file_size: None,
                content_hash: None,
                language: None,
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::FileTouch { path, touch } => {
                assert_eq!(path, "src/main.rs");
                assert_eq!(touch, "read");
            }
            other => panic!("expected FileTouch, got {:?}", other),
        }
    }

    #[test]
    fn file_edit_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::FileTouch {
                path: "src/lib.rs".into(),
                touch: TouchType::Edit,
                lines: None,
                file_size: None,
                content_hash: None,
                language: None,
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::FileTouch { path, touch } => {
                assert_eq!(path, "src/lib.rs");
                assert_eq!(touch, "edit");
            }
            other => panic!("expected FileTouch, got {:?}", other),
        }
    }

    #[test]
    fn permission_granted_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::PermissionCheck {
                key: "read_file".into(),
                decision: PermissionDecision::Granted,
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Permission { key, granted } => {
                assert_eq!(key, "read_file");
                assert!(granted);
            }
            other => panic!("expected Permission, got {:?}", other),
        }
    }

    #[test]
    fn permission_denied_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::PermissionFull {
                key: "write_file".into(),
                decision: PermissionDecision::Denied,
                tool: "write".into(),
                headless: false,
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Permission { key, granted } => {
                assert_eq!(key, "write_file");
                assert!(!granted);
            }
            other => panic!("expected Permission, got {:?}", other),
        }
    }

    #[test]
    fn retry_translates_to_error() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::RetryEvent {
                kind: "llm_call".into(),
                attempt: 2,
                max_retries: 3,
                error: "timeout".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Error { message } => {
                assert!(message.contains("retry"));
                assert!(message.contains("llm_call"));
                assert!(message.contains("#2"));
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::ToolCallBegin {
                name: "read".into(),
                args: serde_json::json!({"path": "file.txt"}),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::ToolCall { name, .. } => {
                assert_eq!(name, "read");
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn tool_end_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::ToolCallEnd {
                name: "read".into(),
                is_error: false,
                output_size: 100,
                duration_ms: 50,
                output_preview: "file content".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::ToolEnd {
                is_error,
                duration_ms,
                ..
            } => {
                assert!(!is_error);
                assert_eq!(*duration_ms, 50);
            }
            other => panic!("expected ToolEnd, got {:?}", other),
        }
    }

    #[test]
    fn sub_agent_lifecycle_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::SubAgentLifecycle {
                agent_id: "sub_1".into(),
                status: SubAgentStatus::Running,
                parent_span_id: "sp_001".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::SubAgent { label, status } => {
                assert_eq!(label, "sub_1");
                assert_eq!(status, "Running");
            }
            other => panic!("expected SubAgent, got {:?}", other),
        }
    }

    #[test]
    fn compaction_drop_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::CompactionDrop {
                span_id: "sp_001".into(),
                dropped_bytes: 4096,
                summary: "dropped old turns".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Compaction { dropped_bytes } => {
                assert_eq!(*dropped_bytes, 4096);
            }
            other => panic!("expected Compaction, got {:?}", other),
        }
    }

    #[test]
    fn user_message_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::UserMessage {
                source: MessageSource::Manual,
                summary: "user said hello".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::UserMessage { summary } => {
                assert_eq!(summary, "user said hello");
            }
            other => panic!("expected UserMessage, got {:?}", other),
        }
    }

    #[test]
    fn user_input_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::UserInput {
                source: MessageSource::Manual,
                length: 20,
                preview: "hello world".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::UserMessage { summary } => {
                assert_eq!(summary, "hello world");
            }
            other => panic!("expected UserMessage, got {:?}", other),
        }
    }

    #[test]
    fn milestone_status_translation() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::MilestoneStatus {
                id: 3,
                title: "Implement API".into(),
                old_status: "pending".into(),
                new_status: "done".into(),
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Notice { text } => {
                assert!(text.contains("#3"));
                assert!(text.contains("Implement API"));
                assert!(text.contains("done"));
            }
            other => panic!("expected Notice, got {:?}", other),
        }
    }

    #[test]
    fn subscribe_mask_filters_events() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.set_subscribe_mask(Some(vec!["Notice"]));
        // This should pass through
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::Notice { text: "visible".into() },
            meta: serde_json::json!({}),
        });
        // This should be filtered out
        obs.on_point(&PointEvent {
            ts: 2.0,
            kind: EventKind::FileTouch {
                path: "x".into(),
                touch: TouchType::Read,
                lines: None,
                file_size: None,
                content_hash: None,
                language: None,
            },
            meta: serde_json::json!({}),
        });
        assert_eq!(obs.len(), 1);
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Notice { text } => assert_eq!(text, "visible"),
            other => panic!("expected Notice, got {:?}", other),
        }
    }

    #[test]
    fn context_snapshot_translates_to_compaction() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::ContextSnapshot {
                before_tokens: 10000,
                after_tokens: 5000,
                dropped_events: 10,
            },
            meta: serde_json::json!({}),
        });
        let events = obs.buffer().recent_events(1);
        match &events[0].kind {
            ObservationKind::Compaction { dropped_bytes } => {
                assert_eq!(*dropped_bytes, 5000);
            }
            other => panic!("expected Compaction, got {:?}", other),
        }
    }

    #[test]
    fn stream_delta_is_kpped() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::StreamDelta { text: "streaming...".into() },
            meta: serde_json::json!({}),
        });
        assert_eq!(obs.len(), 0);
    }

    #[test]
    fn multiple_events_accumulate() {
        let mut obs = ReplayBufferObserver::new(100);
        for i in 0..5 {
            obs.on_point(&PointEvent {
                ts: i as f64,
                kind: EventKind::Notice { text: format!("e{i}") },
                meta: serde_json::json!({}),
            });
        }
        assert_eq!(obs.len(), 5);
        let events = obs.buffer().recent_events(5);
        assert_eq!(events.len(), 5);
        // Most recent first (reversed)
        assert!(format!("{:?}", events[0].kind).contains("e4"));
    }

    #[test]
    fn clear_resets_buffer() {
        let mut obs = ReplayBufferObserver::new(10);
        obs.on_point(&PointEvent {
            ts: 1.0,
            kind: EventKind::Notice { text: "x".into() },
            meta: serde_json::json!({}),
        });
        assert_eq!(obs.len(), 1);
        obs.clear();
        assert_eq!(obs.len(), 0);
    }
}