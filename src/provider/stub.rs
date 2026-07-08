// StubClient: deterministic fake used when CODECODER_API_KEY is unset (ADR 0017).
use super::{CompletionRequest, Provider};
use crate::message::{Message, MessageItem, Role};

pub struct StubClient;

impl Provider for StubClient {
    fn name(&self) -> &str {
        "stub"
    }

    fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Message> {
        Ok(Message {
            id: 0,
            role: Role::Assistant,
            items: vec![MessageItem::Text {
                text: "[stub] no API key set — StubClient deterministic response".into(),
            }],
        })
    }
}
