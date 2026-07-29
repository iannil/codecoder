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
    LlmEnd { completion_tokens: u32, stop_reason: String, duration_ms: u64 },
    ToolCall { name: String, input_preview: String },
    ToolEnd { is_error: bool, output_preview: String, duration_ms: u64 },
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
    capacity: usize,
}

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
        self.buffer.push_back(event);
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
                ObservationKind::LlmCall { .. } => stats.llm_calls += 1,
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
                ObservationKind::LlmEnd { duration_ms, .. } => {
                    stats.duration_ms = stats.duration_ms.max(*duration_ms);
                }
                _ => {}
            }
        }
        stats
    }

    pub fn to_self_observation(&self) -> String {
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
                    out.push_str(&format!("     \u{2192} {}\n", if *is_error { "Error" } else { "Success" }));
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
        let mut completion_tokens = 0u32;
        let mut model = String::new();
        for event in &self.buffer {
            if let ObservationKind::LlmCall { model: m, prompt_tokens: pt } = &event.kind {
                model = m.clone();
                prompt_tokens += pt;
            }
            if let ObservationKind::LlmEnd { completion_tokens: ct, .. } = &event.kind {
                completion_tokens += ct;
            }
        }
        if !model.is_empty() {
            out.push_str(&format!(
                "\n### Token Usage\n  Model: {} | {} prompt + {} completion = {} total\n",
                model, prompt_tokens, completion_tokens, prompt_tokens + completion_tokens,
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_buffer_apends_until_capacity() {
        let mut rb = ReplayBuffer::new_with_capacity(5);
        for i in 0..10 {
            rb.push(ObservationEvent { ts: i as f64, kind: ObservationKind::Notice { text: format!("e{}", i) } });
        }
        assert_eq!(rb.len(), 5);
        // Oldest should be e5
        let recent = rb.recent_events(5);
        assert_eq!(recent.len(), 5);
    }

    #[test]
    fn to_self_observation_returns_empty_when_empty() {
        let rb = ReplayBuffer::new();
        assert!(rb.to_self_observation().is_empty());
    }

    #[test]
    fn to_self_observation_includes_tool_sequence() {
        let mut rb = ReplayBuffer::new();
        rb.push(ObservationEvent { ts: 1.0, kind: ObservationKind::ToolCall { name: "read_file".into(), input_preview: "src/main.rs".into() } });
        rb.push(ObservationEvent { ts: 1.1, kind: ObservationKind::ToolEnd { is_error: false, output_preview: "ok".into(), duration_ms: 50 } });
        rb.push(ObservationEvent { ts: 2.0, kind: ObservationKind::ToolCall { name: "edit_file".into(), input_preview: "src/main.rs".into() } });
        rb.push(ObservationEvent { ts: 2.1, kind: ObservationKind::ToolEnd { is_error: true, output_preview: "error".into(), duration_ms: 100 } });
        let obs = rb.to_self_observation();
        assert!(obs.contains("Tool Call Sequence"));
        assert!(obs.contains("read_file"));
        assert!(obs.contains("edit_file"));
        assert!(obs.contains("Error"));
    }

    #[test]
    fn stats_since_filters_by_timestamp() {
        let mut rb = ReplayBuffer::new();
        rb.push(ObservationEvent { ts: 1.0, kind: ObservationKind::ToolCall { name: "old".into(), input_preview: "".into() } });
        rb.push(ObservationEvent { ts: 2.0, kind: ObservationKind::ToolCall { name: "new".into(), input_preview: "".into() } });
        let stats = rb.stats_since(1.5);
        assert_eq!(stats.tool_calls, 1); // only "new"
    }

    #[test]
    fn filter_by_file_returns_matching() {
        let mut rb = ReplayBuffer::new();
        rb.push(ObservationEvent { ts: 1.0, kind: ObservationKind::FileTouch { path: "src/main.rs".into(), touch: "read".into() } });
        rb.push(ObservationEvent { ts: 2.0, kind: ObservationKind::FileTouch { path: "src/lib.rs".into(), touch: "edit".into() } });
        let hits = rb.filter_by_file("src/main.rs");
        assert_eq!(hits.len(), 1);
    }
}