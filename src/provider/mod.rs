// Provider trait (ADR 0017): translates the neutral message model to/from a wire
// protocol. OpenAI chat-completions is canonical; StubClient is the keyless fake.
use crate::message::Message;

pub mod openai;
pub mod stub;

pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub temperature: f32,
    /// OpenAI-facing tool schemas (ADR 0018). Empty when no tools are offered.
    pub tools: Vec<serde_json::Value>,
}

/// Why the provider stopped generating (OpenAI `finish_reason`). Carried out of
/// band so the loop can distinguish a natural stop from a `max_tokens` truncation
/// — a truncated turn may hold a half-serialized tool call whose args must NOT be
/// executed (roadmap #1 / ADR 0027).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Stop,
    Length,
    ToolCalls,
    Other,
}

/// A provider's assembled reply: the neutral Message plus why it stopped.
pub struct Completion {
    pub message: Message,
    pub stop_reason: StopReason,
}

impl From<Message> for Completion {
    /// Infer the stop reason from the message shape — used by fakes/tests and any
    /// provider that doesn't report `finish_reason`. A tool call present implies
    /// `ToolCalls`, otherwise `Stop`. Never infers `Length` (truncation can only be
    /// known from the wire), so inferred completions are always safe to execute.
    fn from(message: Message) -> Self {
        let stop_reason = if message
            .items
            .iter()
            .any(|it| matches!(it, crate::message::MessageItem::ToolCall { .. }))
        {
            StopReason::ToolCalls
        } else {
            StopReason::Stop
        };
        Completion { message, stop_reason }
    }
}

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    /// Blocking completion. (Streaming deltas → AgentEvent is the real design,
    /// ADR 0016; the scaffold returns the assembled Message.)
    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion>;
}
