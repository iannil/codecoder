// Unified message model (ADR 0015) — provider-neutral (ADR 0017).
use serde::{Deserialize, Serialize};

/// Per-session monotonic id; the UI/persistence identity of a whole Message.
/// NOT a UUID, and distinct from `ToolCall.id` (the provider correlation id).
pub type MessageId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// One content element of a Message. Provider-neutral: serializes identically
/// regardless of which Provider produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum MessageItem {
    Text { text: String },
    /// Chain-of-thought. Persisted, but NOT replayed to the provider (ADR 0004).
    Reasoning { text: String },
    /// `id` is the provider-neutral correlation id → OpenAI `tool_calls[].id`.
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// `call_id` links back to the originating ToolCall → OpenAI `tool_call_id`.
    ToolResult {
        call_id: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub role: Role,
    pub items: Vec<MessageItem>,
}

impl Message {
    pub fn new(id: MessageId, role: Role, items: Vec<MessageItem>) -> Self {
        Self { id, role, items }
    }

    pub fn text(id: MessageId, role: Role, text: impl Into<String>) -> Self {
        Self {
            id,
            role,
            items: vec![MessageItem::Text { text: text.into() }],
        }
    }
}
