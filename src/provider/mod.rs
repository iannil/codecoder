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

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    /// Blocking completion. (Streaming deltas → AgentEvent is the real design,
    /// ADR 0016; the scaffold returns the assembled Message.)
    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Message>;
}
