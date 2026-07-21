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

/// Strip assistant `ToolCall`s whose matching `ToolResult` is absent from the
/// thread, leaving every remaining `ToolCall` paired. After a `navigate_to`
/// onto a mid-tool-call assistant, the active thread can contain an assistant
/// whose tool_calls were abandoned (their results off the active path) — which
/// the OpenAI-compatible provider rejects with a 400 ("tool_calls must be
/// followed by tool messages"). This sanitizes the in-memory copy sent to the
/// provider without altering recorded history. See ADR 0015.
pub fn sanitize_unpaired_tool_calls(messages: &mut Vec<Message>) {
    use std::collections::HashSet;
    let answered: HashSet<String> = messages
        .iter()
        .flat_map(|m| {
            m.items.iter().filter_map(|it| match it {
                MessageItem::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
        })
        .collect();
    for m in messages.iter_mut() {
        if m.role != Role::Assistant {
            continue;
        }
        m.items.retain(|it| match it {
            MessageItem::ToolCall { id, .. } => answered.contains(id),
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_unpaired_tool_calls_after_navigate() {
        // Simulates a navigate onto an assistant whose tool_call result was
        // abandoned (off the active thread): the provider would otherwise see an
        // assistant tool_call with no following tool message.
        let assistant = Message::new(
            1,
            Role::Assistant,
            vec![
                MessageItem::Text { text: "let me check".into() },
                MessageItem::ToolCall { id: "c1".into(), name: "read_file".into(), args: serde_json::json!({}) },
            ],
        );
        // No ToolResult for c1 anywhere in this thread.
        let mut thread = vec![
            Message::text(0, Role::User, "hi"),
            assistant,
            Message::text(2, Role::User, "now what?"),
        ];
        sanitize_unpaired_tool_calls(&mut thread);
        let asst = &thread[1];
        assert!(asst.items.iter().any(|it| matches!(it, MessageItem::Text { .. })));
        assert!(!asst.items.iter().any(|it| matches!(it, MessageItem::ToolCall { .. })));
    }

    #[test]
    fn sanitize_keeps_paired_tool_calls() {
        let assistant = Message::new(
            1,
            Role::Assistant,
            vec![MessageItem::ToolCall { id: "c1".into(), name: "read_file".into(), args: serde_json::json!({}) }],
        );
        let result = Message::new(
            2,
            Role::Tool,
            vec![MessageItem::ToolResult { call_id: "c1".into(), output: "ok".into(), is_error: false }],
        );
        let mut thread = vec![Message::text(0, Role::User, "hi"), assistant, result];
        sanitize_unpaired_tool_calls(&mut thread);
        assert!(thread[1].items.iter().any(|it| matches!(it, MessageItem::ToolCall { .. })));
    }
}
