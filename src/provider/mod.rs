// Provider trait (ADR 0017): translates the neutral message model to/from a wire
// protocol. OpenAI chat-completions is canonical; StubClient is the keyless fake.
use crate::message::Message;
use std::sync::Arc;

pub mod openai;
pub mod retry;
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

/// Token usage for a single LLM completion call.
#[derive(Debug, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// A provider's assembled reply: the neutral Message plus why it stopped.
#[derive(Debug)]
pub struct Completion {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Option<Usage>,
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
        Completion { message, stop_reason, usage: None }
    }
}

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    /// Blocking completion. (Streaming deltas → AgentEvent is the real design,
    /// ADR 0016; the scaffold returns the assembled Message.)
    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion>;
}

/// A provider that tries a primary provider first, and on failure falls back to a
/// secondary provider. Useful for failover across API endpoints or model providers.
pub struct FallbackProvider {
    primary: Arc<dyn Provider>,
    fallback: Arc<dyn Provider>,
}

impl FallbackProvider {
    pub fn new(primary: Arc<dyn Provider>, fallback: Arc<dyn Provider>) -> Self {
        Self { primary, fallback }
    }
}

impl Provider for FallbackProvider {
    fn name(&self) -> &str {
        "fallback"
    }

    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
        match self.primary.complete(req) {
            Ok(c) => Ok(c),
            Err(e) => {
                eprintln!("ccd: primary provider failed: {e}, trying fallback");
                self.fallback.complete(req)
            }
        }
    }
}

/// A fake provider that always fails. Used in fallback tests.
struct AlwaysFailProvider;

impl Provider for AlwaysFailProvider {
    fn name(&self) -> &str {
        "always-fail"
    }
    fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
        Err(anyhow::anyhow!("AlwaysFailProvider always fails"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{MessageItem, Role};
    use crate::provider::stub::StubClient;

    fn dummy_req() -> CompletionRequest {
        CompletionRequest {
            model: "test".into(),
            messages: vec![],
            max_tokens: 100,
            temperature: 0.0,
            tools: vec![],
        }
    }

    /// When the primary succeeds, the fallback is never called.
    #[test]
    fn fallback_primary_success() {
        let primary = Arc::new(StubClient);
        let fallback = Arc::new(AlwaysFailProvider);
        let fb = FallbackProvider::new(primary, fallback);
        let result = fb.complete(&dummy_req());
        assert!(result.is_ok());
        let c = result.unwrap();
        assert_eq!(c.message.role, Role::Assistant);
        assert!(c.message.items.iter().any(|it| matches!(it, MessageItem::Text { text } if text.contains("stub"))));
    }

    /// When the primary fails, the fallback is tried and its result returned.
    #[test]
    fn fallback_primary_fails_fallback_succeeds() {
        let primary = Arc::new(AlwaysFailProvider);
        let fallback = Arc::new(StubClient);
        let fb = FallbackProvider::new(primary, fallback);
        let result = fb.complete(&dummy_req());
        assert!(result.is_ok());
        let c = result.unwrap();
        assert_eq!(c.message.role, Role::Assistant);
        assert!(c.message.items.iter().any(|it| matches!(it, MessageItem::Text { text } if text.contains("stub"))));
    }

    /// When both primary and fallback fail, the error from the fallback is returned.
    #[test]
    fn fallback_both_fail() {
        let primary = Arc::new(AlwaysFailProvider);
        let fallback = Arc::new(AlwaysFailProvider);
        let fb = FallbackProvider::new(primary, fallback);
        let result = fb.complete(&dummy_req());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("AlwaysFailProvider"), "error should mention the fallback failure: {err}");
    }

    /// FallbackProvider name is "fallback".
    #[test]
    fn fallback_name() {
        let fb = FallbackProvider::new(Arc::new(StubClient), Arc::new(StubClient));
        assert_eq!(fb.name(), "fallback");
    }
}
