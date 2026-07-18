// StubClient: deterministic fake used when CODECODER_API_KEY is unset (ADR 0017).
use super::{Completion, CompletionRequest, Provider};
use crate::message::{Message, MessageItem, Role};
use std::sync::Mutex;

pub struct StubClient;

impl Provider for StubClient {
    fn name(&self) -> &str {
        "stub"
    }

    fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
        Ok(Message {
            id: 0,
            role: Role::Assistant,
            items: vec![MessageItem::Text {
                text: "[stub] no API key set — StubClient deterministic response".into(),
            }],
        }
        .into())
    }
}

/// Reads a JSON script of assistant turns from disk, replaying one per call.
/// Used only by the pty smoke layer (L2) via CODECODER_SCRIPT.
pub struct ScriptFileProvider {
    turns: Vec<Message>,
    idx: Mutex<usize>,
}

impl ScriptFileProvider {
    pub fn from_path(path: &str) -> Self {
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        let turns: Vec<Message> = serde_json::from_str(&raw).unwrap_or_default();
        Self {
            turns,
            idx: Mutex::new(0),
        }
    }
}

impl Provider for ScriptFileProvider {
    fn name(&self) -> &str {
        "script-file"
    }
    fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
        let mut i = self.idx.lock().unwrap();
        let msg = self.turns.get(*i).cloned().unwrap_or(Message {
            id: 0,
            role: Role::Assistant,
            items: vec![MessageItem::Text {
                text: "[script exhausted]".into(),
            }],
        });
        *i += 1;
        Ok(msg.into())
    }
}
