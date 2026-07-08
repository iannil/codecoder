// Compaction (ADR 0023): shapes the derived Context Working Set when token_count
// nears the model window (~75%). Never destroys the persisted Session.
use crate::message::Message;

/// Fraction of the model window at which compaction kicks in.
pub const COMPACTION_THRESHOLD: f32 = 0.75;

pub fn should_compact(token_count: u64, model_window: u64) -> bool {
    model_window > 0 && token_count as f32 >= model_window as f32 * COMPACTION_THRESHOLD
}

/// Derive the working set sent to the provider from the full-fidelity messages.
/// Tier 1: drop old ToolResult bodies + Reasoning. Tier 2: summarize oldest span.
/// The first user goal is anchored (never evicted). Scaffold: identity view.
pub fn working_set(messages: &[Message], _model_window: u64) -> Vec<Message> {
    // TODO(ADR 0023): tiered hybrid compaction. For now, the full history is the view.
    messages.to_vec()
}
