use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

/// Global registry of live sub-agents, keyed by agent_id, holding a channel
/// through which the parent can send them messages.
pub struct AgentRegistry {
    agents: HashMap<String, std::sync::mpsc::Sender<String>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self { agents: HashMap::new() }
    }
    pub fn register(&mut self, id: String, tx: std::sync::mpsc::Sender<String>) {
        self.agents.insert(id, tx);
    }
    pub fn unregister(&mut self, id: &str) {
        self.agents.remove(id);
    }
    pub fn send(&mut self, id: &str, message: &str) -> Result<(), String> {
        match self.agents.get(id) {
            Some(tx) => tx
                .send(message.to_string())
                .map_err(|_| format!("agent {id} is no longer reachable")),
            None => Err(format!("no live agent with id {id}")),
        }
    }
    pub fn contains(&self, id: &str) -> bool {
        self.agents.contains_key(id)
    }
    pub fn ids(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }
}

pub static AGENT_REGISTRY: LazyLock<Mutex<AgentRegistry>> =
    LazyLock::new(|| Mutex::new(AgentRegistry::new()));

pub struct SendMessage;

impl Tool for SendMessage {
    fn name(&self) -> &str {
        "send_message"
    }
    fn description(&self) -> &str {
        "Send a message to a live sub-agent (identified by its agent_id) or to the parent agent. For sub-agent communication and coordination."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Target: a sub-agent's id, or the special value 'main' for the parent." },
                "message": { "type": "string", "description": "Message content (required)." },
                "summary": { "type": "string", "description": "Short summary for display (optional)." }
            },
            "required": ["to", "message"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let to = args
            .get("to")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let message = args
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if to.is_empty() || message.is_empty() {
            return Ok(ToolOutput::err("send_message requires `to` and `message`"));
        }
        // 'main' = parent agent. This is handled by the parent's own message loop,
        // but for now we route sub-agent -> parent via the registry's parent channel.
        if to == "main" {
            // Parent agent's channel is registered under "main" by the spawning logic.
            match AGENT_REGISTRY.lock().unwrap().send("main", &message) {
                Ok(()) => Ok(ToolOutput::ok("message sent to parent")),
                Err(e) => Ok(ToolOutput::err(e)),
            }
        } else {
            match AGENT_REGISTRY.lock().unwrap().send(&to, &message) {
                Ok(()) => Ok(ToolOutput::ok(format!("message sent to {to}"))),
                Err(e) => Ok(ToolOutput::err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn registry_register_send_unregister() {
        let mut reg = AgentRegistry::new();
        let (tx, rx) = mpsc::channel();
        reg.register("sub_1".into(), tx);
        assert!(reg.contains("sub_1"));
        assert!(reg.send("sub_1", "hello").is_ok());
        assert_eq!(rx.try_recv().unwrap(), "hello");
        reg.unregister("sub_1");
        assert!(!reg.contains("sub_1"));
        assert!(reg.send("sub_1", "x").is_err());
    }

    #[test]
    fn registry_ids_returns_all_keys() {
        let mut reg = AgentRegistry::new();
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        reg.register("a".into(), tx1);
        reg.register("b".into(), tx2);
        let mut ids = reg.ids();
        ids.sort();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn send_message_tool_requires_to_and_message() {
        let out = SendMessage
            .run(json!({}), &mut ToolCtx::new(Path::new(".")))
            .unwrap();
        assert!(out.is_error);
        let out = SendMessage
            .run(json!({"to": "x"}), &mut ToolCtx::new(Path::new(".")))
            .unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn send_message_to_unknown_agent_errors() {
        let out = SendMessage
            .run(
                json!({"to": "nonexistent", "message": "hi"}),
                &mut ToolCtx::new(Path::new(".")),
            )
            .unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn send_message_permission_none() {
        assert!(matches!(
            SendMessage.permission(&json!({}), Path::new(".")),
            Permission::None
        ));
    }
}