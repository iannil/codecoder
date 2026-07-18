use std::sync::{Arc, Mutex};
use codecoder::{Completion, CompletionRequest, Message, MessageItem, Provider, Role};

#[derive(Clone)]
pub struct RecordedRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<serde_json::Value>,
}

pub type Recorder = Arc<Mutex<Vec<RecordedRequest>>>;

/// Deterministic provider: replays a scripted sequence of assistant messages,
/// one per complete() call, and records every request for assertion.
pub struct ScriptedProvider {
    turns: Vec<Message>,
    idx: Mutex<usize>,
    recorder: Recorder,
}

impl ScriptedProvider {
    pub fn new(turns: Vec<Message>) -> (Arc<ScriptedProvider>, Recorder) {
        let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
        let p = Arc::new(ScriptedProvider {
            turns,
            idx: Mutex::new(0),
            recorder: recorder.clone(),
        });
        (p, recorder)
    }
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &str { "scripted" }
    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
        self.recorder.lock().unwrap().push(RecordedRequest {
            model: req.model.clone(),
            messages: req.messages.clone(),
            tools: req.tools.clone(),
        });
        let mut i = self.idx.lock().unwrap();
        // After the script is exhausted, always return a bare text turn so the
        // agent's tool loop terminates deterministically.
        let msg = self.turns.get(*i).cloned().unwrap_or(Message {
            id: 0, role: Role::Assistant,
            items: vec![MessageItem::Text { text: "[done]".into() }],
        });
        *i += 1;
        // Stop reason is inferred from the message shape (From<Message>): a tool
        // call implies ToolCalls, else Stop — never Length, so scripted turns are
        // always safe to execute.
        Ok(msg.into())
    }
}

pub fn assistant_text(text: &str) -> Message {
    Message { id: 0, role: Role::Assistant,
        items: vec![MessageItem::Text { text: text.into() }] }
}

pub fn assistant_tool_call(id: &str, name: &str, args: serde_json::Value) -> Message {
    Message { id: 0, role: Role::Assistant,
        items: vec![MessageItem::ToolCall { id: id.into(), name: name.into(), args }] }
}

pub fn assistant_tool_calls(calls: Vec<(&str, &str, serde_json::Value)>) -> Message {
    Message { id: 0, role: Role::Assistant, items: calls.into_iter()
        .map(|(id, name, args)| MessageItem::ToolCall { id: id.into(), name: name.into(), args })
        .collect() }
}
