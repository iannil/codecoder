// Agent kernel channel types (ADR 0016). OS threads + channels, no async runtime.
use crate::compaction;
use crate::message::{Message, MessageId, MessageItem, Role};
use crate::permission::{PermScope, Permission, SessionAllowlist};
use crate::provider::{CompletionRequest, Provider};
use crate::registry::Registry;
use crate::session::{self, Session};
use crate::tool::{ToolCtx, Toolbox};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// Guard against a model that never stops calling tools.
const MAX_TOOL_ITERATIONS: usize = 12;

/// TUI → agent over `cmd_tx`. Only user-initiated intents. A permission/ask reply
/// is NOT here — it travels back over the reply_tx carried by an AgentEvent.
pub enum AgentCommand {
    ProcessMessage(String),
    /// Load the most recent session from disk into this agent (ADR 0004).
    Resume,
    /// Rescan skills/ and capabilities/ and rebuild the system prompt (ADR 0020/0022).
    Reload,
    /// Clear the conversation (messages + id counter), keeping the session file.
    Clear,
    Cancel,
    Shutdown,
}

/// The answer to a blocking request, sent back over the oneshot `reply_tx`.
pub enum PermissionReply {
    Grant(PermScope),
    Deny,
    Cancelled,
}

/// agent → TUI over `event_rx`. One-way stream/state traffic, plus blocking
/// round-trips that embed a oneshot `reply_tx` the TUI answers directly.
/// (std has no dedicated oneshot; a send-once mpsc Sender stands in.)
pub enum AgentEvent {
    StreamDelta(String),
    ToolStarted { name: String, preview: String },
    ToolFinished { name: String, is_error: bool, output: String },
    Reasoning(String),
    SubAgentMilestone(String),
    /// Estimated context-window usage percent, for the status bar (ADR 0023).
    Context { pct: u16 },
    /// A system notice for the user (e.g. resume confirmation).
    Notice(String),
    PermissionRequest {
        key: String,
        preview: String,
        reply_tx: Sender<PermissionReply>,
    },
    AskUser {
        prompt: String,
        reply_tx: Sender<String>,
    },
    /// The agent proposes a plan; the user approves or rejects it (ADR 0016).
    PlanApproval {
        plan: String,
        reply_tx: Sender<bool>,
    },
    /// A yes/no confirmation before a step (ADR 0016).
    Confirm {
        prompt: String,
        reply_tx: Sender<bool>,
    },
    TurnComplete,
}

/// Shared cooperative-cancellation flag (ADR 0016): checked between stream deltas
/// and before each tool; long-running children are killed via a stored handle.
#[derive(Debug, Default, Clone)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn reset(&self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The top-level agent: owns the Provider and the Session, runs on its own thread,
/// and is driven by AgentCommands (ADR 0016). Sub-agents reuse the turn logic but
/// with a restricted tool set and no user channel (ADR 0019).
pub struct AgentLoop {
    provider: Arc<dyn Provider>,
    session: Session,
    toolbox: Toolbox,
    allowlist: SessionAllowlist,
    root: PathBuf,
    session_path: PathBuf,
    /// AGENTS.md identity + the skills/capabilities catalog (ADR 0020), injected
    /// as a System message on every request. Rebuilt on `/reload`.
    system_prompt: String,
    /// Whether to autosave the session to disk. Sub-agents don't persist (ADR 0019).
    persist: bool,
    model: String,
    model_window: u64,
    max_tokens: u32,
    temperature: f32,
    next_id: MessageId,
    cancel: CancelToken,
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
    ) -> Self {
        Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true)
    }

    /// A depth-1 sub-agent (ADR 0019): read-only toolbox (only `Permission::None`
    /// tools, no `agent`), no session persistence, shares the parent's Provider.
    fn new_sub(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
    ) -> Self {
        Self::build(provider, model, max_tokens, temperature, root, Toolbox::read_only_child(), false)
    }

    fn build(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
        toolbox: Toolbox,
        persist: bool,
    ) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let session_path = session::sessions_dir(&root).join(format!("session-{stamp}.json"));
        let system_prompt = build_system_prompt(&root);
        Self {
            provider,
            session: Session::new(model.clone()),
            toolbox,
            allowlist: SessionAllowlist::default(),
            root,
            session_path,
            system_prompt,
            persist,
            model_window: crate::tokenizer::model_window(&model),
            model,
            max_tokens,
            temperature,
            next_id: 0,
            cancel: CancelToken::default(),
        }
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    fn append(&mut self, role: Role, items: Vec<MessageItem>) -> MessageId {
        let id = self.next_id;
        self.next_id += 1;
        self.session.messages.push(Message { id, role, items });
        // Autosave on every append (ADR 0004), best-effort. Sub-agents don't persist.
        if self.persist {
            let _ = self.session.save(&self.session_path);
        }
        id
    }

    /// Load the most recent session on disk, continuing its file and id sequence.
    fn resume_latest(&mut self, event_tx: &Sender<AgentEvent>) {
        let Some(path) = session::latest_session(&self.root) else {
            let _ = event_tx.send(AgentEvent::Notice("no session to resume".into()));
            let _ = event_tx.send(AgentEvent::TurnComplete);
            return;
        };
        match std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|raw| Session::load(&raw)) {
            Ok(session) => {
                self.next_id = session.next_message_id();
                let count = session.messages.len();
                self.session = session;
                self.session_path = path;
                let _ = event_tx.send(AgentEvent::Notice(format!("resumed {count} messages")));
            }
            Err(e) => {
                // Load failure preserves the original file untouched (ADR 0004).
                let _ = event_tx.send(AgentEvent::Notice(format!("resume failed: {e}")));
            }
        }
        let _ = event_tx.send(AgentEvent::TurnComplete);
    }

    /// Block on the command channel and service commands until Shutdown.
    pub fn run(mut self, cmd_rx: Receiver<AgentCommand>, event_tx: Sender<AgentEvent>) {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                AgentCommand::ProcessMessage(text) => {
                    self.cancel.reset();
                    self.process_turn(text, &event_tx);
                }
                AgentCommand::Resume => self.resume_latest(&event_tx),
                AgentCommand::Reload => {
                    let reg = Registry::scan(&self.root);
                    let n = reg.catalog.len();
                    self.system_prompt = build_system_prompt(&self.root);
                    let _ = event_tx.send(AgentEvent::Notice(format!("reloaded — {n} skills/capabilities in catalog")));
                    let _ = event_tx.send(AgentEvent::TurnComplete);
                }
                AgentCommand::Clear => {
                    self.session.messages.clear();
                    self.next_id = 0;
                    self.session.token_count = 0;
                    if self.persist {
                        let _ = self.session.save(&self.session_path);
                    }
                    let _ = event_tx.send(AgentEvent::Context { pct: 0 });
                    let _ = event_tx.send(AgentEvent::Notice("conversation cleared".into()));
                    let _ = event_tx.send(AgentEvent::TurnComplete);
                }
                AgentCommand::Cancel => self.cancel.cancel(),
                AgentCommand::Shutdown => break,
            }
        }
    }

    /// One turn: query → if the reply calls tools, execute them (permission-gated),
    /// feed results back, and re-query — repeating until the model stops calling
    /// tools or the iteration guard trips (ADR 0016/0018).
    fn process_turn(&mut self, text: String, event_tx: &Sender<AgentEvent>) {
        self.append(Role::User, vec![MessageItem::Text { text }]);

        for _ in 0..MAX_TOOL_ITERATIONS {
            if self.cancel.is_cancelled() {
                break;
            }

            // Only the derived working set is sent to the provider (ADR 0023),
            // prefixed by the System prompt (AGENTS.md + catalog, ADR 0020).
            let working = compaction::working_set(&self.model, &self.session.messages, self.model_window);
            let mut messages = Vec::with_capacity(working.len() + 1);
            if !self.system_prompt.is_empty() {
                messages.push(Message::text(u64::MAX, Role::System, self.system_prompt.clone()));
            }
            messages.extend(working);

            // Accurate token count for the status bar + compaction (ADR 0023).
            let used = crate::tokenizer::count_tokens(&self.model, &messages);
            self.session.token_count = used;
            let pct = ((used * 100) / self.model_window.max(1)).min(100) as u16;
            let _ = event_tx.send(AgentEvent::Context { pct });

            let req = CompletionRequest {
                model: self.session.model.clone(),
                messages,
                max_tokens: self.max_tokens,
                temperature: self.temperature,
                tools: self.toolbox.wire_schemas(),
            };

            let reply = match self.provider.complete(&req) {
                Ok(r) => r,
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::StreamDelta(format!("error: {e}")));
                    break;
                }
            };

            // Surface assistant text/reasoning, then record the assistant turn.
            let mut tool_calls = Vec::new();
            for item in &reply.items {
                match item {
                    MessageItem::Text { text } => {
                        let _ = event_tx.send(AgentEvent::StreamDelta(text.clone()));
                    }
                    MessageItem::Reasoning { text } => {
                        let _ = event_tx.send(AgentEvent::Reasoning(text.clone()));
                    }
                    MessageItem::ToolCall { id, name, args } => {
                        tool_calls.push((id.clone(), name.clone(), args.clone()));
                    }
                    MessageItem::ToolResult { .. } => {}
                }
            }
            self.append(Role::Assistant, reply.items);

            if tool_calls.is_empty() {
                break; // no tools requested → turn is done
            }

            // Execute each tool call, gating on permission, and collect results.
            let mut results = Vec::new();
            let mut cancelled = false;
            for (call_id, name, args) in tool_calls {
                let result = self.dispatch_tool(&call_id, &name, args, event_tx);
                match result {
                    ToolOutcome::Result(item) => results.push(item),
                    ToolOutcome::Cancelled => {
                        cancelled = true;
                        break;
                    }
                }
            }
            if !results.is_empty() {
                self.append(Role::Tool, results);
            }
            if cancelled {
                break;
            }
        }

        let _ = event_tx.send(AgentEvent::TurnComplete);
    }

    /// Run one tool call: resolve the tool, gate on permission (blocking oneshot
    /// round-trip when a prompt is needed), execute, and produce a ToolResult.
    fn dispatch_tool(
        &mut self,
        call_id: &str,
        name: &str,
        args: serde_json::Value,
        event_tx: &Sender<AgentEvent>,
    ) -> ToolOutcome {
        // Some tools need the Provider / event channel a plain Tool::run can't see,
        // so the AgentLoop intercepts them: `agent` and `review` spawn a sub-agent
        // (ADR 0019); `ask_user` does a blocking user round-trip (ADR 0016).
        if name == "agent" {
            return self.spawn_sub_agent(call_id, &args, event_tx);
        }
        if name == "ask_user" {
            return self.ask_user(call_id, &args, event_tx);
        }
        if name == "plan" {
            return self.plan(call_id, &args, event_tx);
        }
        if name == "confirm" {
            let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let (reply_tx, reply_rx) = channel();
            let _ = event_tx.send(AgentEvent::Confirm { prompt, reply_tx });
            let yes = reply_rx.recv().unwrap_or(false);
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: if yes { "yes".into() } else { "no".into() },
                is_error: false,
            });
        }
        if name == "review" {
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("the current changes");
            let task = format!(
                "Review {target}. Read the relevant files and run `diff` to inspect changes, \
                 then report concrete bugs, risks, and improvements. Be specific."
            );
            let sub_args = serde_json::json!({ "task": task });
            return self.spawn_sub_agent(call_id, &sub_args, event_tx);
        }

        let Some(tool) = self.toolbox.get(name) else {
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: format!("unknown tool: {name}"),
                is_error: true,
            });
        };

        // Permission gate (ADR 0018): None runs freely; Ask consults the session
        // allowlist, else a blocking prompt over the embedded reply_tx (ADR 0016).
        if let Permission::Ask { key } = tool.permission(&args, &self.root) {
            if !self.allowlist.allows(&key) {
                let (reply_tx, reply_rx) = channel();
                let _ = event_tx.send(AgentEvent::PermissionRequest {
                    key: key.clone(),
                    preview: format!("{name}  {}", preview_args(&args)),
                    reply_tx,
                });
                match reply_rx.recv() {
                    Ok(PermissionReply::Grant(scope)) => {
                        if matches!(
                            scope,
                            PermScope::AlwaysThisSession | PermScope::AlwaysThisProject
                        ) {
                            self.allowlist.grant(key);
                        }
                    }
                    Ok(PermissionReply::Deny) => {
                        return ToolOutcome::Result(MessageItem::ToolResult {
                            call_id: call_id.to_string(),
                            output: "permission denied by user".into(),
                            is_error: true,
                        });
                    }
                    Ok(PermissionReply::Cancelled) | Err(_) => {
                        self.cancel.cancel();
                        return ToolOutcome::Cancelled;
                    }
                }
            }
        }

        let _ = event_tx.send(AgentEvent::ToolStarted {
            name: name.to_string(),
            preview: preview_args(&args),
        });
        let mut ctx = ToolCtx { root: &self.root };
        let output = match self.toolbox.get(name).unwrap().run(args, &mut ctx) {
            Ok(o) => o,
            Err(e) => crate::tool::ToolOutput::err(format!("tool error: {e}")),
        };
        let _ = event_tx.send(AgentEvent::ToolFinished {
            name: name.to_string(),
            is_error: output.is_error,
            output: output.content.clone(),
        });

        ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output: output.content,
            is_error: output.is_error,
        })
    }
}

enum ToolOutcome {
    Result(MessageItem),
    Cancelled,
}

impl AgentLoop {
    /// Spawn a depth-1 read-only sub-agent (ADR 0019) to handle a delegated task.
    /// It runs on its own thread that this call joins; coarse milestones are bridged
    /// up as `SubAgentMilestone` events, and its final text becomes the tool result.
    fn spawn_sub_agent(
        &mut self,
        call_id: &str,
        args: &serde_json::Value,
        event_tx: &Sender<AgentEvent>,
    ) -> ToolOutcome {
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if task.is_empty() {
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: "missing required arg: task".into(),
                is_error: true,
            });
        }

        let _ = event_tx.send(AgentEvent::SubAgentMilestone("started".into()));
        let (child_tx, child_rx) = channel::<AgentEvent>();
        let provider = Arc::clone(&self.provider);
        let (model, mt, temp, root) =
            (self.model.clone(), self.max_tokens, self.temperature, self.root.clone());

        let handle = thread::spawn(move || {
            let mut child = AgentLoop::new_sub(provider, model, mt, temp, root);
            child.process_turn(task, &child_tx);
            child.last_assistant_text()
        });

        // Bridge coarse milestones live; the child's own token stream is not forwarded.
        for ev in child_rx {
            if let AgentEvent::ToolStarted { name, .. } = ev {
                let _ = event_tx.send(AgentEvent::SubAgentMilestone(name));
            }
        }
        let output = handle.join().unwrap_or_else(|_| "sub-agent panicked".into());
        let _ = event_tx.send(AgentEvent::SubAgentMilestone("done".into()));

        ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output,
            is_error: false,
        })
    }

    /// Ask the user a question and block until they answer (ADR 0016 oneshot).
    fn ask_user(
        &mut self,
        call_id: &str,
        args: &serde_json::Value,
        event_tx: &Sender<AgentEvent>,
    ) -> ToolOutcome {
        let question = args.get("question").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if question.is_empty() {
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: "missing required arg: question".into(),
                is_error: true,
            });
        }
        let (reply_tx, reply_rx) = channel();
        let _ = event_tx.send(AgentEvent::AskUser { prompt: question, reply_tx });
        let answer = reply_rx.recv().unwrap_or_default();
        ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output: answer,
            is_error: false,
        })
    }

    /// Propose a plan; block on the user's approve/reject (ADR 0016). On approval
    /// the plan is written to PLAN.md.
    fn plan(
        &mut self,
        call_id: &str,
        args: &serde_json::Value,
        event_tx: &Sender<AgentEvent>,
    ) -> ToolOutcome {
        let text = if let Some(steps) = args.get("steps").and_then(|v| v.as_array()) {
            steps
                .iter()
                .filter_map(|s| s.as_str())
                .enumerate()
                .map(|(i, s)| format!("{}. {s}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            args.get("plan").and_then(|v| v.as_str()).unwrap_or_default().to_string()
        };
        if text.is_empty() {
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: "provide `steps` or `plan`".into(),
                is_error: true,
            });
        }

        let (reply_tx, reply_rx) = channel();
        let _ = event_tx.send(AgentEvent::PlanApproval { plan: text.clone(), reply_tx });
        let approved = reply_rx.recv().unwrap_or(false);

        let output = if approved {
            let _ = std::fs::write(self.root.join("PLAN.md"), format!("# Plan\n\n{text}\n"));
            "plan approved and recorded to PLAN.md".to_string()
        } else {
            "plan rejected by user".to_string()
        };
        ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output,
            is_error: !approved,
        })
    }

    /// The concatenated text of the most recent Assistant message (sub-agent result).
    fn last_assistant_text(&self) -> String {
        self.session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant && m.items.iter().any(|it| matches!(it, MessageItem::Text { .. })))
            .map(|m| {
                m.items
                    .iter()
                    .filter_map(|it| match it {
                        MessageItem::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default()
    }
}

/// Build the System prompt from AGENTS.md (identity) + the scanned catalog
/// (ADR 0020). "Filesystem as self": both come from disk, not hardcoded.
fn build_system_prompt(root: &std::path::Path) -> String {
    let mut parts = Vec::new();
    if let Ok(agents) = std::fs::read_to_string(root.join("AGENTS.md")) {
        let agents = agents.trim();
        if !agents.is_empty() {
            parts.push(agents.to_string());
        }
    }
    let catalog = Registry::scan(root).render_catalog();
    if !catalog.is_empty() {
        parts.push(catalog);
    }
    parts.join("\n\n")
}

/// Compact one-line preview of tool args for the transcript (e.g. `path=a.txt`).
fn preview_args(args: &serde_json::Value) -> String {
    match args.as_object() {
        Some(map) => map
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}={s}"),
                other => format!("{k}={other}"),
            })
            .collect::<Vec<_>>()
            .join("  "),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CompletionRequest;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Call 1 asks for a read_file tool call; call 2 (after the result is fed back)
    /// returns final text. read_file is Permission::None, so no prompt is needed.
    struct ScriptedProvider {
        calls: AtomicUsize,
        path: String,
        saw_result: Arc<Mutex<bool>>,
    }

    impl Provider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Message> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        args: serde_json::json!({ "path": self.path }),
                    }],
                })
            } else {
                // The tool result must have been fed back into the request.
                let fed = req.messages.iter().any(|m| {
                    m.role == Role::Tool
                        && m.items.iter().any(|it| matches!(it,
                            MessageItem::ToolResult { output, .. } if output.contains("hello world")))
                });
                *self.saw_result.lock().unwrap() = fed;
                Ok(Message::text(0, Role::Assistant, "done"))
            }
        }
    }

    /// call 0 (parent) → delegate via `agent`; call 1 (child) → findings text;
    /// call 2+ (parent) → final answer.
    struct SubScripted {
        calls: AtomicUsize,
    }
    impl Provider for SubScripted {
        fn name(&self) -> &str {
            "sub-scripted"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Message> {
            match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "a1".into(),
                        name: "agent".into(),
                        args: serde_json::json!({ "task": "research the thing" }),
                    }],
                }),
                1 => Ok(Message::text(0, Role::Assistant, "sub findings")),
                _ => Ok(Message::text(0, Role::Assistant, "final answer")),
            }
        }
    }

    #[test]
    fn sub_agent_delegates_and_returns_findings() {
        let dir = std::env::temp_dir().join(format!("cc_sub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let provider = Arc::new(SubScripted { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        agent.process_turn("go".into(), &etx);
        drop(etx);
        let events: Vec<_> = erx.into_iter().collect();

        assert!(events.iter().any(|e| matches!(e, AgentEvent::SubAgentMilestone(m) if m == "started")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::SubAgentMilestone(m) if m == "done")));

        // The sub-agent's findings were fed back as a tool result...
        assert!(agent.session.messages.iter().any(|m| m.role == Role::Tool
            && m.items.iter().any(|it| matches!(it,
                MessageItem::ToolResult { output, .. } if output == "sub findings"))));
        // ...and the parent produced its final answer.
        assert_eq!(agent.last_assistant_text(), "final answer");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// call 0 → ask_user; call 1 → echo the answer back as the final text.
    struct AskScripted {
        calls: AtomicUsize,
    }
    impl Provider for AskScripted {
        fn name(&self) -> &str {
            "ask-scripted"
        }
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Message> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "q1".into(),
                        name: "ask_user".into(),
                        args: serde_json::json!({ "question": "favorite color?" }),
                    }],
                })
            } else {
                // Prove the user's answer was fed back into the request.
                let answer = req
                    .messages
                    .iter()
                    .flat_map(|m| &m.items)
                    .find_map(|it| match it {
                        MessageItem::ToolResult { output, .. } => Some(output.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok(Message::text(0, Role::Assistant, format!("you said {answer}")))
            }
        }
    }

    #[test]
    fn ask_user_round_trip() {
        let dir = std::env::temp_dir().join(format!("cc_ask_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(AskScripted { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        // Responder: answer the AskUser prompt, mimicking the TUI dialog.
        let responder = thread::spawn(move || {
            for ev in erx {
                if let AgentEvent::AskUser { reply_tx, .. } = ev {
                    let _ = reply_tx.send("blue".into());
                }
            }
        });
        agent.process_turn("go".into(), &etx);
        drop(etx);
        responder.join().unwrap();

        assert_eq!(agent.last_assistant_text(), "you said blue");
        std::fs::remove_dir_all(&dir).ok();
    }

    struct PlanScripted {
        calls: AtomicUsize,
    }
    impl Provider for PlanScripted {
        fn name(&self) -> &str {
            "plan-scripted"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Message> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "p1".into(),
                        name: "plan".into(),
                        args: serde_json::json!({ "steps": ["do a", "do b"] }),
                    }],
                })
            } else {
                Ok(Message::text(0, Role::Assistant, "proceeding"))
            }
        }
    }

    struct ConfirmScripted {
        calls: AtomicUsize,
    }
    impl Provider for ConfirmScripted {
        fn name(&self) -> &str {
            "confirm-scripted"
        }
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Message> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "c1".into(),
                        name: "confirm".into(),
                        args: serde_json::json!({ "prompt": "proceed?" }),
                    }],
                })
            } else {
                let ans = req
                    .messages
                    .iter()
                    .flat_map(|m| &m.items)
                    .find_map(|it| match it {
                        MessageItem::ToolResult { output, .. } => Some(output.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok(Message::text(0, Role::Assistant, format!("answer={ans}")))
            }
        }
    }

    #[test]
    fn confirm_round_trip() {
        let dir = std::env::temp_dir().join(format!("cc_confirm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(ConfirmScripted { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());
        let (etx, erx) = channel();
        let responder = thread::spawn(move || {
            for ev in erx {
                if let AgentEvent::Confirm { reply_tx, .. } = ev {
                    let _ = reply_tx.send(true);
                }
            }
        });
        agent.process_turn("go".into(), &etx);
        drop(etx);
        responder.join().unwrap();
        assert_eq!(agent.last_assistant_text(), "answer=yes");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_approval_writes_plan_md_when_approved() {
        let dir = std::env::temp_dir().join(format!("cc_plan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(PlanScripted { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        let responder = thread::spawn(move || {
            for ev in erx {
                if let AgentEvent::PlanApproval { reply_tx, .. } = ev {
                    let _ = reply_tx.send(true); // approve
                }
            }
        });
        agent.process_turn("go".into(), &etx);
        drop(etx);
        responder.join().unwrap();

        let plan_md = std::fs::read_to_string(dir.join("PLAN.md")).unwrap();
        assert!(plan_md.contains("1. do a") && plan_md.contains("2. do b"), "{plan_md}");
        assert_eq!(agent.last_assistant_text(), "proceeding");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tool_loop_executes_and_feeds_result_back() {
        let dir = std::env::temp_dir().join(format!("cc_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "hello world").unwrap();

        let saw = Arc::new(Mutex::new(false));
        let provider = Arc::new(ScriptedProvider {
            calls: AtomicUsize::new(0),
            path: "note.txt".into(),
            saw_result: Arc::clone(&saw),
        });
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        agent.process_turn("read note.txt".into(), &etx);
        drop(etx);

        let events: Vec<_> = erx.into_iter().collect();
        let started = events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "read_file"));
        let finished = events.iter().any(|e| {
            matches!(e, AgentEvent::ToolFinished { name, is_error, .. } if name == "read_file" && !is_error)
        });
        assert!(started && finished, "tool should have started and finished");
        assert!(*saw.lock().unwrap(), "tool result should be fed back to the provider");

        // Final assistant turn is the plain "done" text.
        let last = agent.session.messages.last().unwrap();
        assert!(matches!(&last.items[0], MessageItem::Text { text } if text == "done"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
