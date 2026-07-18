// OpenAiClient: the canonical chat-completions Provider (ADR 0017).
use super::{Completion, CompletionRequest, Provider, StopReason};
use crate::config::Config;
use crate::message::{Message, MessageItem, Role};
use serde_json::{Value, json};

pub struct OpenAiClient {
    api_key: String,
    api_base: String,
}

impl OpenAiClient {
    pub fn new(cfg: &Config) -> Self {
        Self {
            api_key: cfg.api_key.clone().unwrap_or_default(),
            api_base: cfg.api_base.clone(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.api_base.trim_end_matches('/'))
    }
}

impl Provider for OpenAiClient {
    fn name(&self) -> &str {
        "openai"
    }

    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
        let mut body = json!({
            "model": req.model,
            "messages": to_wire_messages(&req.messages),
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req.tools);
        }

        let resp = match ureq::post(&self.endpoint())
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_json(body)
        {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                anyhow::bail!("OpenAI API returned {code}: {detail}");
            }
            Err(e) => return Err(anyhow::Error::new(e).context("OpenAI request failed")),
        };

        let json: Value = resp.into_json()?;
        from_wire_response(&json)
    }
}

/// Neutral message model -> OpenAI chat-completions `messages` array (ADR 0017).
/// `Reasoning` items are skipped — chain-of-thought is never replayed (ADR 0004).
fn to_wire_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User | Role::System => {
                let role = if msg.role == Role::User { "user" } else { "system" };
                out.push(json!({ "role": role, "content": collect_text(msg) }));
            }
            Role::Assistant => {
                let text = collect_text(msg);
                let tool_calls: Vec<Value> = msg
                    .items
                    .iter()
                    .filter_map(|it| match it {
                        MessageItem::ToolCall { id, name, args } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": args.to_string() },
                        })),
                        _ => None,
                    })
                    .collect();
                let mut m = json!({ "role": "assistant" });
                if !text.is_empty() {
                    m["content"] = json!(text);
                }
                if !tool_calls.is_empty() {
                    m["tool_calls"] = json!(tool_calls);
                }
                // Skip an assistant turn that is neither text nor tool calls.
                if m.get("content").is_some() || m.get("tool_calls").is_some() {
                    out.push(m);
                }
            }
            Role::Tool => {
                // Each ToolResult becomes its own `role:"tool"` turn keyed by tool_call_id.
                for it in &msg.items {
                    if let MessageItem::ToolResult { call_id, output, .. } = it {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": output,
                        }));
                    }
                }
            }
        }
    }
    out
}

fn collect_text(msg: &Message) -> String {
    msg.items
        .iter()
        .filter_map(|it| match it {
            MessageItem::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// OpenAI response -> neutral assistant Message + StopReason. `id` is left 0; the
/// AgentLoop assigns the session-local MessageId on append.
fn from_wire_response(json: &Value) -> anyhow::Result<Completion> {
    let choice = json
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| anyhow::anyhow!("malformed response: missing choices[0]"))?;
    let message = choice
        .get("message")
        .ok_or_else(|| anyhow::anyhow!("malformed response: missing choices[0].message"))?;

    let stop_reason = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("length") => StopReason::Length,
        Some("tool_calls") => StopReason::ToolCalls,
        Some("stop") => StopReason::Stop,
        _ => StopReason::Other,
    };

    let mut items = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        if !content.is_empty() {
            items.push(MessageItem::Text { text: content.to_string() });
        }
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let id = call.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let func = call.get("function");
            let name = func
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args = func
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            items.push(MessageItem::ToolCall { id, name, args });
        }
    }

    let message = Message { id: 0, role: Role::Assistant, items };
    Ok(Completion { message, stop_reason })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_is_not_replayed() {
        let msgs = vec![Message {
            id: 0,
            role: Role::Assistant,
            items: vec![
                MessageItem::Reasoning { text: "secret cot".into() },
                MessageItem::Text { text: "hello".into() },
            ],
        }];
        let wire = to_wire_messages(&msgs);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["content"], "hello");
        assert!(!wire[0].to_string().contains("secret cot"));
    }

    #[test]
    fn tool_call_and_result_map_to_wire() {
        let msgs = vec![
            Message {
                id: 0,
                role: Role::Assistant,
                items: vec![MessageItem::ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    args: json!({ "path": "a.txt" }),
                }],
            },
            Message {
                id: 1,
                role: Role::Tool,
                items: vec![MessageItem::ToolResult {
                    call_id: "call_1".into(),
                    output: "contents".into(),
                    is_error: false,
                }],
            },
        ];
        let wire = to_wire_messages(&msgs);
        assert_eq!(wire[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(wire[0]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(wire[1]["role"], "tool");
        assert_eq!(wire[1]["tool_call_id"], "call_1");
        assert_eq!(wire[1]["content"], "contents");
    }

    #[test]
    fn finish_reason_maps_to_stop_reason() {
        let length = json!({
            "choices": [{ "finish_reason": "length", "message": { "content": "partial" } }]
        });
        assert_eq!(from_wire_response(&length).unwrap().stop_reason, StopReason::Length);

        let tools = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": { "tool_calls": [{
                    "id": "c", "type": "function",
                    "function": { "name": "x", "arguments": "{}" }
                }] }
            }]
        });
        assert_eq!(from_wire_response(&tools).unwrap().stop_reason, StopReason::ToolCalls);

        let stop = json!({
            "choices": [{ "finish_reason": "stop", "message": { "content": "done" } }]
        });
        assert_eq!(from_wire_response(&stop).unwrap().stop_reason, StopReason::Stop);

        let other = json!({
            "choices": [{ "finish_reason": "content_filter", "message": { "content": "" } }]
        });
        assert_eq!(from_wire_response(&other).unwrap().stop_reason, StopReason::Other);
    }

    #[test]
    fn response_with_content_and_tool_calls_parses() {
        let resp = json!({
            "choices": [{
                "message": {
                    "content": "sure",
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": { "name": "run_command", "arguments": "{\"cmd\":\"ls\"}" }
                    }]
                }
            }]
        });
        let msg = from_wire_response(&resp).unwrap().message;
        assert!(matches!(&msg.items[0], MessageItem::Text { text } if text == "sure"));
        match &msg.items[1] {
            MessageItem::ToolCall { id, name, args } => {
                assert_eq!(id, "call_9");
                assert_eq!(name, "run_command");
                assert_eq!(args["cmd"], "ls");
            }
            _ => panic!("expected ToolCall"),
        }
    }
}
