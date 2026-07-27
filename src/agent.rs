// Agent kernel channel types (ADR 0016). OS threads + channels, no async runtime.
use crate::compaction;
use crate::message::{Message, MessageId, MessageItem, Role};
use crate::permission::{PermScope, Permission, ProjectAllowlist, SessionAllowlist, scope_ceiling};
use crate::provider::{Completion, CompletionRequest, Provider, StopReason};
use crate::registry::Registry;
use crate::session::{self, Session};
use crate::trust::{self, TrustDecision};
use crate::tool::{ToolCtx, Toolbox};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// Guard against a model that never stops calling tools.
const MAX_TOOL_ITERATIONS: usize = 12;

/// 纯探索工具(迭代 4 no-op 兜底):只读/查、不推进交付物。turn 内连续多轮全是这些
/// → 注入一次 steering nudge。write_file/edit_file/run_command/commit/reason/milestone/
/// memory/plan 等都不在此集,算「动了」。
const EXPLORATION_TOOLS: &[&str] = &["read_file", "glob", "grep", "diff"];

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
    /// Navigate to a session entry (time-travel within the tree).
    Navigate(u64),
    Cancel,
    Shutdown,
}

/// The answer to a blocking request, sent back over the oneshot `reply_tx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionReply {
    Grant(PermScope),
    Deny,
    Cancelled,
}

/// The user's answer to a project-trust prompt (ADR 0028). `Always`/`Never`
/// persist to the global trust store; `Once` trusts for this session only.
pub enum TrustReply {
    Always,
    Once,
    Never,
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
    /// First-turn prompt to trust an undecided project's disk "self" (ADR 0028).
    /// The reply decides whether AGENTS.md/skills/capabilities and the
    /// codecoder.json allowlist load at all.
    TrustPrompt {
        root: PathBuf,
        reply_tx: Sender<TrustReply>,
    },
    /// Verify test suite loaded — pre-scan of all test cases.
    TestSuiteLoaded(crate::verify::TestSuiteLoaded),
    /// Progress update for one test case.
    TestProgress(crate::verify::TestProgress),
    /// All tests completed.
    TestSuiteComplete(crate::verify::TestSuiteComplete),
    /// L4 场景进度
    L4ScenarioProgress(crate::verify::event::L4ScenarioProgress),
    /// L4 探索进度
    L4ExploreProgress(crate::verify::event::L4ExploreProgress),
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

    /// On Unix, arrange for SIGINT (Ctrl+C / `kill -INT`) to cancel this token,
    /// so a runaway headless Background Agent stops gracefully (ADR 0026): the
    /// turn loop polls `is_cancelled`, and `run_command` kills its subprocess on
    /// cancel. signal-hook stacks handlers, so registering per-agent (the initial
    /// turn + each auto-advanced milestone) sets every registered token on SIGINT.
    /// Returns Err if installation fails; callers log and continue (cancel just
    /// won't be wired). No-op off-Unix.
    #[cfg(unix)]
    pub fn cancel_on_sigint(&self) -> anyhow::Result<()> {
        signal_hook::flag::register(signal_hook::consts::SIGINT, self.0.clone())?;
        Ok(())
    }
    #[cfg(not(unix))]
    #[allow(unused_variables)]
    pub fn cancel_on_sigint(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A shared queue of user messages submitted while a turn is running (ADR 0029).
/// Like `CancelToken`, the TUI writes to it directly — the agent thread is blocked
/// in `process_turn` and cannot service `cmd_rx` mid-turn. `process_turn` drains
/// it to inject steering, and to restart instead of stopping (follow-up).
#[derive(Clone, Default)]
pub struct SteerQueue(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl SteerQueue {
    /// Enqueue a user message to be picked up mid-turn.
    pub fn push(&self, msg: String) {
        if let Ok(mut q) = self.0.lock() {
            q.push(msg);
        }
    }
    /// Take everything queued so far (FIFO), leaving the queue empty.
    pub fn drain(&self) -> Vec<String> {
        match self.0.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }
}

/// In-memory tier-2 summary cache (ADR 0023). Keyed by the covered span's last
/// message id: stable within a turn (tools append non-User messages), so at most
/// one summary LLM call fires per turn. Not persisted — recomputed after /resume.
struct Tier2Summary {
    covered_last_id: MessageId,
    text: String,
    read_files: BTreeSet<String>,
    modified_files: BTreeSet<String>,
}

/// Whether the project's disk "self" (AGENTS.md, skills/prompts/capabilities, the
/// codecoder.json allowlist) has been loaded (ADR 0028). `Pending` means an
/// interactive top-level agent has not yet asked the user — it prompts on the
/// first turn and only then loads. Headless and sub-agents never stay Pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustState {
    Trusted,
    Untrusted,
    Pending,
}

/// The top-level agent: owns the Provider and the Session, runs on its own thread,
/// and is driven by AgentCommands (ADR 0016). Sub-agents reuse the turn logic but
/// with a restricted tool set and no user channel (ADR 0019).
pub struct AgentLoop {
    provider: Arc<dyn Provider>,
    session: Session,
    toolbox: Toolbox,
    allowlist: SessionAllowlist,
    /// Persisted project-scope grants (`codecoder.json`, ADR 0005), loaded at
    /// startup and consulted alongside the in-memory session allowlist.
    project_allowlist: ProjectAllowlist,
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
    /// 自适应截断根治(迭代 2):命中 StopReason::Length 时,单 turn 有效 max_tokens
    /// 翻倍上调的封顶值。build 内由 Config::from_env() 注入,故所有构造点(交互/BG/
    /// daemon/sub-agent/verify)统一遵守 CODECODER_MAX_TOKENS_CEILING。
    max_tokens_ceiling: u32,
    /// no-op 探索兜底阈值(迭代 4)。build 内由 Config::from_env() 注入;0 = 禁用。
    noop_nudge_threshold: usize,
    temperature: f32,
    next_id: MessageId,
    cancel: CancelToken,
    /// 单 turn 工具迭代上限(默认 MAX_TOOL_ITERATIONS;BG 单 milestone 经
    /// `set_tool_cap` 收紧,防固着耗尽预算)。
    tool_cap: usize,
    /// No user is present (Background Agent, ADR 0026). Changes the permission
    /// gate: an Ask-tool not in an allowlist is auto-denied instead of prompting,
    /// and ask_user/confirm/plan short-circuit — there is no one to answer.
    headless: bool,
    /// Whether the project's disk "self" has loaded (ADR 0028). Gated at build();
    /// an interactive undecided project resolves via a first-turn TrustPrompt.
    trust: TrustState,
    /// User messages submitted mid-turn (ADR 0029), drained inside `process_turn`.
    steer: SteerQueue,
    /// Derived tier-2 summary overlay (ADR 0023); never persisted.
    tier2: Option<Tier2Summary>,
    /// 最近一次 turn 的 provider 错误(若有)。BG runner 据此置 mission_state=Error(ADR 0033)。
    last_error: Option<String>,
    /// daemon 共享目录（ADR 0020）。`None` 时 build_system_prompt 自扫（TUI/sub-agent）。
    shared_registry: Option<Arc<std::sync::RwLock<Registry>>>,
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
    ) -> Self {
        Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true, false, None)
    }

    /// A Background Agent (ADR 0026): full builtin toolbox, persists its session,
    /// but runs headless (no user present) — the permission gate auto-denies any
    /// Ask-tool not pre-authorized in an allowlist.
    pub fn new_background(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
    ) -> Self {
        Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true, true, None)
    }

    /// daemon 托管的 session：共享 daemon 的 Registry（ADR 0020 daemon 级目录）。
    pub fn new_daemon(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
        registry: Arc<std::sync::RwLock<Registry>>,
    ) -> Self {
        Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true, false, Some(registry))
    }

    /// Drive exactly one turn to completion (headless). Thin public wrapper over
    /// the internal turn loop so a Background runner can invoke it without the
    /// full command-channel run loop.
    pub fn run_one_turn(&mut self, task: String, event_tx: &Sender<AgentEvent>) {
        self.cancel.reset();
        self.process_turn(task, event_tx);
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
        Self::build(provider, model, max_tokens, temperature, root, Toolbox::read_only_child(), false, false, None)
    }

    fn build(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
        toolbox: Toolbox,
        persist: bool,
        headless: bool,
        shared_registry: Option<Arc<std::sync::RwLock<Registry>>>,
    ) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let session_path = session::sessions_dir(&root).join(format!("session-{stamp}.json"));

        // Resolve trust (ADR 0028). An explicit recorded decision always wins.
        // When undecided: headless takes the env default (no one to prompt); a
        // sub-agent (no persistence, no user channel) defaults to Untrusted; an
        // interactive top-level agent stays Pending and prompts on its first turn.
        let trust = match trust::decide(&root) {
            Some(TrustDecision::Trusted) => TrustState::Trusted,
            Some(TrustDecision::Untrusted) => TrustState::Untrusted,
            None if headless => match trust::default_trust() {
                TrustDecision::Trusted => TrustState::Trusted,
                TrustDecision::Untrusted => TrustState::Untrusted,
            },
            // A sub-agent has no user channel to prompt (ADR 0019) → safe default.
            None if !persist => TrustState::Untrusted,
            // Nothing on disk to gate → no need to bother the user.
            None if !trust::has_config_resources(&root) => TrustState::Trusted,
            // Interactive top-level, undecided, with real config on disk → ask.
            None => TrustState::Pending,
        };

        // The disk "self" loads only when trusted; otherwise the agent runs on its
        // compiled-in base identity + native tools, with an empty allowlist.
        let trusted = trust == TrustState::Trusted;
        if crate::trust::should_warn_untrusted_allowlist(&root, trusted, headless) {
            use std::sync::Once;
            static WARN_ONCE: Once = Once::new();
            WARN_ONCE.call_once(|| {
                eprintln!(
                    "ccd: codecoder.json found but project is untrusted → allowlist not loaded; \
                     every pre-authorized Ask tool will be auto-denied. Trust the project to \
                     load it (CODECODER_DEFAULT_TRUST=always for undecided projects, or a \
                     ~/.codecoder/trust.json entry; an explicitly-untrusted recorded decision \
                     must be changed there)."
                );
            });
        }
        let system_prompt = if trusted {
            match &shared_registry {
                // Render the catalog under a brief read-lock, then DROP the lock
                // before the disk I/O inside `build_system_prompt_with_catalog`.
                Some(reg) => {
                    let catalog = reg.read().unwrap().render_catalog();
                    build_system_prompt_with_catalog(&root, &catalog)
                }
                None => build_system_prompt(&root),
            }
        } else { String::new() };
        let project_allowlist = if trusted { ProjectAllowlist::load(&root) } else { ProjectAllowlist::default() };

        Self {
            provider,
            session: Session::new(model.clone()),
            toolbox,
            allowlist: SessionAllowlist::default(),
            project_allowlist,
            root,
            session_path,
            system_prompt,
            persist,
            model_window: crate::tokenizer::model_window(&model),
            model,
            max_tokens,
            max_tokens_ceiling: crate::config::Config::from_env().max_tokens_ceiling,
            noop_nudge_threshold: crate::config::Config::from_env().noop_nudge_threshold,
            temperature,
            next_id: 0,
            cancel: CancelToken::default(),
            tool_cap: MAX_TOOL_ITERATIONS,
            headless,
            trust,
            steer: SteerQueue::default(),
            tier2: None,
            last_error: None,
            shared_registry,
        }
    }

    /// Load the project's disk "self" now that it is trusted (ADR 0028): AGENTS.md
    /// identity + skills/capabilities catalog + the codecoder.json allowlist.
    fn load_self(&mut self) {
        self.system_prompt = build_system_prompt(&self.root);
        self.project_allowlist = ProjectAllowlist::load(&self.root);
    }

    /// Rebuild `system_prompt` from the shared daemon Registry if this session
    /// has one and is trusted. Called at the top of every `process_turn` so a
    /// skill/capability written by another session (or a manual edit) shows up
    /// on this session's next turn. No-op for sub-agents / background agents
    /// (`shared_registry` is `None`) and for untrusted/pending projects.
    fn refresh_system_prompt_if_shared(&mut self) {
        if self.trust != TrustState::Trusted {
            return;
        }
        if let Some(reg) = &self.shared_registry {
            // Render the catalog under a brief read-lock, then DROP the lock
            // before any disk I/O (`AGENTS.md`, `WorkGraph::read`).
            let catalog = reg.read().unwrap().render_catalog();
            self.system_prompt = build_system_prompt_with_catalog(&self.root, &catalog);
        }
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// 覆盖默认工具迭代上限(ADR 0026:BG 单 milestone 用更紧预算,防固着)。
    pub fn set_tool_cap(&mut self, n: usize) {
        self.tool_cap = n.max(1);
    }

    /// 覆盖自适应截断的 max_tokens 封顶(测试/特殊场景)。默认由 build 从 env 注入。
    pub fn set_max_tokens_ceiling(&mut self, n: u32) {
        self.max_tokens_ceiling = n;
    }

    /// 覆盖 no-op 兜底阈值(测试/特殊场景)。
    pub fn set_noop_nudge_threshold(&mut self, n: usize) {
        self.noop_nudge_threshold = n;
    }

    /// 最近一次 turn 是否因 provider 错误失败(ADR 0033:BG 据此置 mission_state=Error)。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// A shared handle to the steering queue (ADR 0029). The TUI pushes mid-turn
    /// user input here; `process_turn` drains it.
    pub fn steer_handle(&self) -> SteerQueue {
        self.steer.clone()
    }

    fn append(&mut self, role: Role, items: Vec<MessageItem>) -> MessageId {
        let id = self.next_id;
        self.next_id += 1;
        self.session.append(Message { id, role, items });
        // Autosave on every append (ADR 0004), best-effort. Sub-agents don't persist.
        if self.persist {
            let _ = self.session.save(&self.session_path);
        }
        id
    }

    /// Apply a session-leaf meta mark from a tool's `ToolOutput.session_meta_mark`
    /// (Phase E side-channel — tools can't write the session directly). Writes the
    /// mark onto the current leaf's `SessionEntry.meta` and autosaves. No-op if
    /// there is no current leaf.
    fn apply_session_meta_mark(&mut self, mark: serde_json::Value) {
        if let Some(leaf) = self.session.leaf {
            self.session.update_meta(leaf, |m| *m = Some(mark));
            if self.persist {
                let _ = self.session.save(&self.session_path);
            }
        }
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
                let count = session.entries.len();
                self.session = session;
                self.session_path = path;
                self.tier2 = None;
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
                    // Auto-drive the workgraph: if there are ready milestones, advance
                    // them without waiting for another user command (plan #2).
                    if self.persist && self.trust == TrustState::Trusted {
                        self.drive_workgraph(&event_tx);
                    }
                }
                AgentCommand::Resume => self.resume_latest(&event_tx),
                AgentCommand::Reload => {
                    // Only a trusted project re-scans its disk "self" (ADR 0028);
                    // an untrusted/pending one keeps its empty identity.
                    if self.trust == TrustState::Trusted {
                        // Acquire the read-lock ONCE for both `n` (count) and the
                        // rendered catalog, then DROP it before any disk I/O
                        // (Critical #2 + Important #4 — avoids both a held-lock-
                        // across-I/O and a TOCTOU between count and prompt).
                        let (n, catalog) = match &self.shared_registry {
                            Some(reg) => {
                                let g = reg.read().unwrap();
                                (g.catalog.len(), g.render_catalog())
                            }
                            None => {
                                let r = Registry::scan(&self.root);
                                (r.catalog.len(), r.render_catalog())
                            }
                        };
                        self.system_prompt = build_system_prompt_with_catalog(&self.root, &catalog);
                        let _ = event_tx.send(AgentEvent::Notice(format!("reloaded — {n} skills/capabilities in catalog")));
                    } else {
                        let _ = event_tx.send(AgentEvent::Notice("project not trusted; nothing reloaded".into()));
                    }
                    let _ = event_tx.send(AgentEvent::TurnComplete);
                }
                AgentCommand::Clear => {
                    self.session.clear();
                    self.next_id = 0;
                    self.session.token_count = 0;
                    self.tier2 = None;
                    if self.persist {
                        let _ = self.session.save(&self.session_path);
                    }
                    let _ = event_tx.send(AgentEvent::Context { pct: 0 });
                    let _ = event_tx.send(AgentEvent::Notice("conversation cleared".into()));
                    let _ = event_tx.send(AgentEvent::TurnComplete);
                }
                AgentCommand::Navigate(id) => {
                    self.handle_navigate(id, &event_tx);
                    let _ = event_tx.send(AgentEvent::TurnComplete);
                }
                AgentCommand::Cancel => self.cancel.cancel(),
                AgentCommand::Shutdown => break,
            }
        }
    }

    /// Handle a Navigate command (Phase C: summarize abandoned branch, then jump).
    /// Extracted for testability (Task 3).
    fn handle_navigate(&mut self, id: u64, event_tx: &Sender<AgentEvent>) {
        // Phase C: when navigating leaves a branch behind, summarize it
        // before the jump (ADR 0023 tier-2 is reused for the summary).
        if self.session.leaf.is_some_and(|l| l != 0) && self.session.entries.len() > 1 {
            let abandoned = self.session.abandoned_branch(id);
            if !abandoned.is_empty() {
                let thread = self.session.nodes_by_id(&abandoned);
                let rendered = crate::compaction::render_span(&thread);
                if !rendered.trim().is_empty() {
                    let (summary, summarized_ok) = match self.summarize_span(&rendered, None) {
                        Ok(s) => (s, true),
                        Err(_) => (
                            "(summary unavailable: compaction failed — branch abandoned without a generated brief)"
                                .into(),
                            false,
                        ),
                    };
                    let _ = event_tx.send(AgentEvent::Notice(if summarized_ok {
                        format!("abandoned branch summarized: {summary}")
                    } else {
                        "failed to summarize abandoned branch; linked causal nodes marked ruled_out with an unavailable ruling".into()
                    }));
                    // Phase E: structured ruling on linked causal nodes. Even when
                    // summarization failed, write a placeholder ruling so a linked
                    // node is visibly marked rather than silently orphaned.
                    for entry_id in &abandoned {
                        let causal_node = self.session.entry_by_id(*entry_id)
                            .and_then(|e| e.meta.as_ref())
                            .and_then(|m| m.get("causal_node"))
                            .and_then(|v| v.as_u64());
                        if let Some(cn) = causal_node {
                            let s = summary.clone();
                            self.session.update_meta(*entry_id, |m| {
                                let obj = m.get_or_insert(serde_json::json!({}));
                                if let Some(o) = obj.as_object_mut() {
                                    o.insert("status".into(), "ruled_out".into());
                                    o.insert("ruling".into(), s.into());
                                }
                            });
                            if let Err(e) = crate::tool::reason::record_ruling(&self.root, cn, &summary) {
                                let _ = event_tx.send(AgentEvent::Notice(
                                    format!("causal ruling write failed for node #{cn}: {e}"),
                                ));
                            }
                        }
                    }
                }
            }
        }
        if self.session.navigate_to(id) {
            let _ = event_tx.send(AgentEvent::Notice(format!("navigated to entry #{id}")));
            // Autosave after changing leaf (daemon Navigate expects this).
            if self.persist {
                let _ = self.session.save(&self.session_path);
            }
        } else {
            let _ = event_tx.send(AgentEvent::Notice(format!("no entry #{id}")));
        }
    }

    /// One-shot LLM summary of a rendered span (ADR 0023 tier-2). Structured brief;
    /// when `previous` is set, the earlier summary is merged with the new span
    /// (iterative compaction). Returns Err on transport failure or empty output.
    fn summarize_span(&self, rendered: &str, previous: Option<&str>) -> anyhow::Result<String> {
        let system = "You are compacting an agent's conversation history into a concise, \
            structured brief. Use exactly these sections, plain prose under each, and omit a \
            section when it has no content:\n\
            ## 目标\n## 约束与偏好\n## 进展（已完成 / 进行中 / 受阻）\n## 关键决策\n## 下一步\n## 关键上下文\n\
            Preserve goals, decisions, key facts, file paths, tool outcomes, and open threads. \
            Do NOT list read/modified files — those are tracked separately. Omit chit-chat and \
            any preamble.";
        let mut messages = vec![Message::text(0, Role::System, system)];
        let mut uid = 1u64;
        if let Some(prev) = previous {
            messages.push(Message::text(
                uid,
                Role::User,
                format!("先前摘要（请与下列新增消息合并，更新为一份完整摘要）：\n{prev}"),
            ));
            uid += 1;
        }
        messages.push(Message::text(uid, Role::User, rendered.to_string()));
        let req = CompletionRequest {
            model: self.model.clone(),
            messages,
            max_tokens: 1024,
            temperature: 0.0,
            tools: vec![],
        };
        let reply = self.provider.complete(&req)?.message;
        let text: String = reply
            .items
            .iter()
            .filter_map(|it| match it {
                MessageItem::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            anyhow::bail!("empty summary");
        }
        Ok(text)
    }

    /// Derive the Context Working Set: tier-1 always; tier-2 (summarize the oldest
    /// span) only when tier-1 is still over the window threshold. Degrades to tier-1
    /// if the summary call fails. Caches the summary in-memory (one call per turn).
    fn context_working_set(&mut self, event_tx: &Sender<AgentEvent>) -> Vec<Message> {
        let thread = self.session.active_thread();
        let tier1 = compaction::working_set(&self.model, &thread, self.model_window, None, false);
        if !compaction::should_compact(
            crate::tokenizer::count_tokens(&self.model, &tier1),
            self.model_window,
        ) {
            return tier1;
        }
        let Some((start, end)) = compaction::summary_span(&thread) else {
            return tier1;
        };
        let anchor_id = thread[start - 1].id;
        let covered_last_id = thread[end - 1].id;

        let mut read = BTreeSet::new();
        let mut modified = BTreeSet::new();
        let prose: String;

        match self.tier2.as_ref() {
            // Span unchanged → full reuse, no LLM call, no Notice.
            Some(c) if c.covered_last_id == covered_last_id => {
                read = c.read_files.clone();
                modified = c.modified_files.clone();
                prose = c.text.clone();
            }
            // Span grew → summarize only the increment, seeded by cached summary + files.
            Some(c) if c.covered_last_id < covered_last_id => {
                read = c.read_files.clone();
                modified = c.modified_files.clone();
                let inc_start = thread[start..end]
                    .iter()
                    .position(|m| m.id > c.covered_last_id)
                    .map(|p| start + p)
                    .unwrap_or(start);
                let slice = &thread[inc_start..end];
                compaction::collect_file_paths(slice, &mut read, &mut modified);
                let rendered = compaction::render_span(slice);
                let prev = c.text.clone();
                match self.summarize_span(&rendered, Some(&prev)) {
                    Ok(t) => {
                        let _ = event_tx.send(AgentEvent::Notice(
                            "compacting context (summarizing earlier turns)…".into(),
                        ));
                        prose = t;
                    }
                    Err(_) => return tier1,
                }
            }
            // No cache, or id rewound (e.g. after /resume) → summarize the whole span.
            _ => {
                let slice = &thread[start..end];
                compaction::collect_file_paths(slice, &mut read, &mut modified);
                let rendered = compaction::render_span(slice);
                match self.summarize_span(&rendered, None) {
                    Ok(t) => {
                        let _ = event_tx.send(AgentEvent::Notice(
                            "compacting context (summarizing earlier turns)…".into(),
                        ));
                        prose = t;
                    }
                    Err(_) => return tier1,
                }
            }
        }

        self.tier2 = Some(Tier2Summary {
            covered_last_id,
            text: prose.clone(),
            read_files: read.clone(),
            modified_files: modified.clone(),
        });

        let summary_text = format!("{}{}", prose, compaction::render_file_blocks(&read, &modified));
        compaction::apply_tier2(&tier1, anchor_id, covered_last_id, &summary_text)
    }

    /// One turn: query → if the reply calls tools, execute them (permission-gated),
    /// feed results back, and re-query — repeating until the model stops calling
    /// tools or the iteration guard trips (ADR 0016/0018).
    /// Call the provider, retrying transient throttle/transport failures with a
    /// short backoff (ADR 0027 Wave 0 #3). The classifier lives in `crate::retry`;
    /// policy (budget, backoff, reporting) lives here. Account limits and context
    /// overflows are not retried. Aborts early if the turn was cancelled.
    fn complete_retrying(
        &self,
        req: &CompletionRequest,
        event_tx: &Sender<AgentEvent>,
    ) -> anyhow::Result<Completion> {
        const MAX_RETRIES: u32 = 2;
        let mut attempt = 0u32;
        loop {
            match self.provider.complete(req) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    let msg = e.to_string();
                    if attempt < MAX_RETRIES
                        && crate::retry::is_retryable(&msg)
                        && !self.cancel.is_cancelled()
                    {
                        attempt += 1;
                        let _ = event_tx.send(AgentEvent::Notice(format!(
                            "transient error, retrying ({attempt}/{MAX_RETRIES}): {msg}"
                        )));
                        std::thread::sleep(std::time::Duration::from_millis(200 * attempt as u64));
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Drain the steering queue (ADR 0029), appending each queued user message as a
    /// `Role::User` turn so the next provider call sees it. Returns whether anything
    /// was drained (the natural-stop point uses this to restart rather than stop).
    fn drain_steer(&mut self, event_tx: &Sender<AgentEvent>) -> bool {
        let queued = self.steer.drain();
        if queued.is_empty() {
            return false;
        }
        for text in queued {
            let _ = event_tx.send(AgentEvent::Notice(format!("steering: {text}")));
            self.append(Role::User, vec![MessageItem::Text { text }]);
        }
        true
    }

    /// If trust is still `Pending` (interactive, undecided, config on disk), ask
    /// the user once before the first turn runs (ADR 0028). The answer resolves the
    /// state and — when trusted — loads the project's disk "self" now. A dropped
    /// reply channel (no responder) defaults to Untrusted, the safe outcome.
    fn resolve_trust_if_pending(&mut self, event_tx: &Sender<AgentEvent>) {
        if self.trust != TrustState::Pending {
            return;
        }
        let (reply_tx, reply_rx) = channel();
        let _ = event_tx.send(AgentEvent::TrustPrompt { root: self.root.clone(), reply_tx });
        match reply_rx.recv() {
            Ok(TrustReply::Always) => {
                trust::record(&self.root, TrustDecision::Trusted);
                self.trust = TrustState::Trusted;
                self.load_self();
                let _ = event_tx.send(AgentEvent::Notice("project trusted; self loaded".into()));
            }
            Ok(TrustReply::Once) => {
                self.trust = TrustState::Trusted;
                self.load_self();
                let _ = event_tx.send(AgentEvent::Notice("project trusted for this session".into()));
            }
            Ok(TrustReply::Never) => {
                trust::record(&self.root, TrustDecision::Untrusted);
                self.trust = TrustState::Untrusted;
                let _ = event_tx.send(AgentEvent::Notice(
                    "project not trusted; AGENTS.md/skills/capabilities and codecoder.json skipped".into(),
                ));
            }
            Err(_) => {
                // No one answered → don't load the disk self, and don't ask again.
                self.trust = TrustState::Untrusted;
            }
        }
    }

    fn process_turn(&mut self, text: String, event_tx: &Sender<AgentEvent>) {
        // Special message: `/verify` command — run the test suite.
        if text == "__verify__" {
            self.run_verify(event_tx);
            return;
        }
        self.resolve_trust_if_pending(event_tx);
        self.append(Role::User, vec![MessageItem::Text { text }]);

        let mut hit_tool_cap = true; // cleared on every non-exhaustion exit
        // 自适应截断根治(迭代 2):有效 max_tokens 每 turn 从配置值起,命中 Length 翻倍上调。
        let mut effective_max_tokens = self.max_tokens;
        // no-op 探索兜底(迭代 4):统计连续「纯探索」迭代,达阈值注入一次 nudge。
        let mut consecutive_explore_iters = 0usize;
        let mut nudged_this_turn = false;
        for _ in 0..self.tool_cap {
            if self.cancel.is_cancelled() {
                hit_tool_cap = false;
                break;
            }

            // Inject any mid-turn user input (ADR 0029) as User messages before the
            // next provider call — this is steering (redirect the running turn).
            self.drain_steer(event_tx);

            // Only the derived working set is sent to the provider (ADR 0023),
            // prefixed by the System prompt (AGENTS.md + catalog, ADR 0020).
            let working = self.context_working_set(event_tx);
            let mut messages = Vec::with_capacity(working.len() + 1);
            // Refresh the system_prompt from the shared Registry JUST BEFORE we
            // push it as the System message — so non-chat paths (`__verify__`
            // and other early returns above) skip the refresh, and the refresh
            // takes effect for the current turn. (Important #5.)
            self.refresh_system_prompt_if_shared();
            if !self.system_prompt.is_empty() {
                messages.push(Message::text(u64::MAX, Role::System, self.system_prompt.clone()));
            }
            messages.extend(working);

            // A navigate onto a mid-tool-call assistant can leave an assistant
            // ToolCall whose result is off the active path; the provider rejects
            // such unpaired tool_calls with a 400. Sanitize the in-memory copy.
            crate::message::sanitize_unpaired_tool_calls(&mut messages);

            // Accurate token count for the status bar + compaction (ADR 0023).
            let used = crate::tokenizer::count_tokens(&self.model, &messages);
            self.session.token_count = used;
            let pct = ((used * 100) / self.model_window.max(1)).min(100) as u16;
            let _ = event_tx.send(AgentEvent::Context { pct });

            let req = CompletionRequest {
                model: self.session.model.clone(),
                messages,
                max_tokens: effective_max_tokens,
                temperature: self.temperature,
                tools: self.toolbox.wire_schemas(),
            };

            let (reply, stop_reason) = match self.complete_retrying(&req, event_tx) {
                Ok(c) => (c.message, c.stop_reason),
                Err(e) => {
                    // A context overflow (ADR 0027 #2) won't recover on retry; give
                    // the user an actionable hint instead of a bare error.
                    let msg = e.to_string();
                    if crate::retry::is_context_overflow(&msg) {
                        let _ = event_tx.send(AgentEvent::Notice(
                            "context window exceeded — /clear or start a new session to continue".into(),
                        ));
                    }
                    let _ = event_tx.send(AgentEvent::StreamDelta(format!("error: {e}")));
                    self.last_error = Some(msg.clone());
                    hit_tool_cap = false;
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

            // 截断根治(迭代 2 / ADR 0038):响应在 max_tokens 处被截断时,先 neutralize 任何
            // 半序列化的 tool call(绝不执行),再自适应上调本 turn 的有效预算重试;达封顶
            // 仍截断则收尾。此判定必须在 `tool_calls.is_empty()` 收尾之前——否则截断的纯
            // 文本响应会被当成 turn 正常结束而静默收尾。
            if stop_reason == StopReason::Length {
                if !tool_calls.is_empty() {
                    let results = tool_calls
                        .iter()
                        .map(|(call_id, name, _)| {
                            let output = "tool call truncated: the response hit max_tokens before the \
                                 arguments finished. Not executed. The model's reasoning was too long, \
                                 leaving no room for the file content. Retry with a MUCH shorter thought \
                                 process, or if this was a write_file, consider whether the file was \
                                 partially created — you can append to it with append=true."
                                .to_string();
                            let _ = event_tx.send(AgentEvent::ToolFinished {
                                name: name.clone(),
                                is_error: true,
                                output: output.clone(),
                            });
                            MessageItem::ToolResult {
                                call_id: call_id.clone(),
                                output,
                                is_error: true,
                            }
                        })
                        .collect();
                    self.append(Role::Tool, results);
                }
                if effective_max_tokens < self.max_tokens_ceiling {
                    effective_max_tokens = effective_max_tokens.saturating_mul(2).min(self.max_tokens_ceiling);
                    let _ = event_tx.send(AgentEvent::Notice(format!(
                        "response truncated at max_tokens; raising to {effective_max_tokens} and retrying"
                    )));
                    continue; // 带更大预算重试
                }
                // 已达封顶:tool_calls 情形已追加 is_error(交模型重试);空 tool_calls 情形收尾。
                if tool_calls.is_empty() {
                    hit_tool_cap = false;
                    break;
                }
                continue;
            }

            if tool_calls.is_empty() {
                // The turn would end here. If the user steered in the meantime
                // (ADR 0029), restart the loop with that input instead of stopping.
                if self.drain_steer(event_tx) {
                    continue;
                }
                hit_tool_cap = false;
                break; // no tools requested and nothing steered → turn is done
            }

            // 分类本迭代是否「纯探索」(tool_calls 非空且全部 ∈ EXPLORATION_TOOLS)。tool_calls
            // 随后在 dispatch 循环被移动,故先算。
            let all_exploration = !tool_calls.is_empty()
                && tool_calls.iter().all(|(_, name, _)| EXPLORATION_TOOLS.contains(&name.as_str()));

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
            // no-op 兜底:更新连续纯探索计数;达阈值且本 turn 未 nudge 过 → 注入一次 steering。
            if all_exploration {
                consecutive_explore_iters += 1;
            } else {
                consecutive_explore_iters = 0;
            }
            if self.noop_nudge_threshold > 0
                && consecutive_explore_iters >= self.noop_nudge_threshold
                && !nudged_this_turn
            {
                let n = self.noop_nudge_threshold;
                self.append(Role::User, vec![MessageItem::Text {
                    text: format!(
                        "You have only explored (read/glob/grep/diff) for {n} tool steps without \
                         making a change. Make a concrete edit or run a command now, or explicitly \
                         state that you are blocked and why."
                    ),
                }]);
                let _ = event_tx.send(AgentEvent::Notice(format!(
                    "no-op backstop: nudged to act after {n} exploration-only steps"
                )));
                nudged_this_turn = true;
            }
            if cancelled {
                hit_tool_cap = false;
                break;
            }
        }
        if hit_tool_cap {
            // The loop exhausted MAX_TOOL_ITERATIONS while the agent kept calling
            // tools — the turn was capped, not finished. Surface it so the abrupt
            // stop isn't mistaken for completion.
            let _ = event_tx.send(AgentEvent::Notice(format!(
                "turn stopped at the {}-tool-iteration cap; the task may be incomplete — send another message to continue.", self.tool_cap
            )));
        }

        let _ = event_tx.send(AgentEvent::TurnComplete);
    }

    /// Run the verify test suite and stream progress events.
    fn run_verify(&mut self, event_tx: &Sender<AgentEvent>) {
        use crate::verify::VerifyRunner;
        use crate::verify::scenario::all_scenarios;
        use crate::verify::runner::L4Runner;

        let _ = event_tx.send(AgentEvent::Notice("verify mode starting".into()));

        // === 阶段 0: L1-L3 现有测试 ===
        let mut runner = VerifyRunner::start_l1(&self.root, event_tx.clone());
        loop {
            if self.cancel.is_cancelled() {
                runner.cancel();
                break;
            }
            if runner.poll().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        if self.cancel.is_cancelled() {
            let _ = event_tx.send(AgentEvent::TurnComplete);
            return;
        }

        // === 阶段 1: L4 骨架场景 ===
        let _ = event_tx.send(AgentEvent::Notice("L4 验证开始".into()));
        let scenarios = all_scenarios();

        // L4Runner::run_scenarios 会 emit 场景事件，
        // 由 TUI 的 apply_l4_scenario 自动创建场景状态。
        let all_critical_passed = L4Runner::run_scenarios(
            &scenarios,
            event_tx,
            &self.cancel,
            &self.root,
        );

        if self.cancel.is_cancelled() || !all_critical_passed {
            if !all_critical_passed {
                let _ = event_tx.send(AgentEvent::Notice(
                    "L4 验证失败：核心工具场景未通过，停止验证".into()
                ));
            }
            let _ = event_tx.send(AgentEvent::TurnComplete);
            return;
        }

        // === 阶段 2: L4 自驱动探索 ===
        let _ = event_tx.send(AgentEvent::Notice("L4 自驱动探索开始".into()));
        L4Runner::run_exploration(event_tx, &self.cancel, &self.root);

        let _ = event_tx.send(AgentEvent::Notice("L4 验证完成".into()));
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
        if self.headless && (name == "ask_user" || name == "confirm") {
            let output = format!("denied: '{name}' requires a user, none present (headless)");
            // Emit a ToolFinished error so the denial is observable in the event
            // stream (the Background Agent runner records these into BgOutcome.denied).
            let _ = event_tx.send(AgentEvent::ToolFinished {
                name: name.to_string(),
                is_error: true,
                output: output.clone(),
            });
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output,
                is_error: true,
            });
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
            // Hand the sub-agent the drift rubric + output contract, then parse
            // its prose into a structured verdict (docs/design/2026-07-19-review-verdict-rubric.md).
            let (outcome, raw) = self.run_review(target, event_tx);
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: crate::review::format_result(&outcome, &raw),
                is_error: false,
            });
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
            if !self.allowlist.allows(&key) && !self.project_allowlist.allows(&key) {
                if self.headless {
                    // Name the root cause (ADR 0028): an untrusted project's
                    // codecoder.json allowlist is NOT loaded, so a key the operator
                    // believes they pre-authorized is denied. Distinguish that from a
                    // genuinely-absent key (trust == Trusted, allowlist loaded).
                    let output = if self.trust != TrustState::Trusted {
                        format!(
                            "denied: '{key}' — project not trusted (headless); the \
                             codecoder.json allowlist is NOT loaded. Set \
                             CODECODER_DEFAULT_TRUST=always (or record trust) to enable \
                             pre-authorized tools."
                        )
                    } else {
                        format!("denied: no user present; '{key}' not in project allowlist")
                    };
                    // Emit a ToolFinished error so the denial is observable in the
                    // event stream (BgOutcome.denied is drained from these events).
                    let _ = event_tx.send(AgentEvent::ToolFinished {
                        name: name.to_string(),
                        is_error: true,
                        output: output.clone(),
                    });
                    return ToolOutcome::Result(MessageItem::ToolResult {
                        call_id: call_id.to_string(),
                        output,
                        is_error: true,
                    });
                }
                let (reply_tx, reply_rx) = channel();
                let _ = event_tx.send(AgentEvent::PermissionRequest {
                    key: key.clone(),
                    preview: format!("{name}  {}", preview_args(&args)),
                    reply_tx,
                });
                match reply_rx.recv() {
                    Ok(PermissionReply::Grant(scope)) => match scope {
                        PermScope::Once => {}
                        // Persist to codecoder.json (ADR 0005) — but honor the
                        // ceiling rule (ADR 0022): a Shell-env capability may never
                        // reach project scope, so cap it at the session set.
                        PermScope::AlwaysThisProject
                            if scope_ceiling(&key) == PermScope::AlwaysThisProject =>
                        {
                            if let Err(e) = self.project_allowlist.grant(&self.root, key) {
                                let _ = event_tx.send(AgentEvent::Notice(format!(
                                    "could not persist project permission: {e}"
                                )));
                            }
                        }
                        PermScope::AlwaysThisSession | PermScope::AlwaysThisProject => {
                            self.allowlist.grant(key);
                        }
                    },
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
        let mut ctx = ToolCtx::with_cancel(&self.root, &self.cancel);
        let mut output = match self.toolbox.get(name).unwrap().run(args, &mut ctx) {
            Ok(o) => o,
            Err(e) => crate::tool::ToolOutput::err(format!("tool error: {e}")),
        };
        if let Some(mark) = output.session_meta_mark.take() {
            self.apply_session_meta_mark(mark);
        }
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
        let output = self.spawn_sub_agent_text(task, event_tx);
        ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output,
            is_error: false,
        })
    }

    /// Run an independent read-only review sub-agent against `target` and parse
    /// its prose into a structured verdict. Returns both the parsed outcome and
    /// the sub-agent's raw prose. Reused by the `review` tool and the Background
    /// review gate (ADR 0039).
    pub fn run_review(
        &mut self,
        target: &str,
        event_tx: &Sender<AgentEvent>,
    ) -> (crate::review::ReviewOutcome, String) {
        let raw = self.spawn_sub_agent_text(crate::review::review_task(target), event_tx);
        let outcome = crate::review::parse_review(&raw);
        (outcome, raw)
    }

    /// Run a read-only sub-agent to completion and return its final assistant
    /// text. Shared by `agent` (wraps it verbatim) and `review` (parses it into
    /// a structured verdict). Emits coarse `SubAgentMilestone` events; the
    /// child's own token stream is not forwarded.
    fn spawn_sub_agent_text(&mut self, task: String, event_tx: &Sender<AgentEvent>) -> String {
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

        for ev in child_rx {
            if let AgentEvent::ToolStarted { name, .. } = ev {
                let _ = event_tx.send(AgentEvent::SubAgentMilestone(name));
            }
        }
        let output = handle.join().unwrap_or_else(|_| "sub-agent panicked".into());
        let _ = event_tx.send(AgentEvent::SubAgentMilestone("done".into()));
        output
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

        // headless 模式:自动批准并写入 PLAN.md,不发送 PlanApproval 事件。
        if self.headless {
            let _ = std::fs::write(self.root.join("PLAN.md"), format!("# Plan\n\n{text}\n"));
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: "plan approved (headless) and recorded to PLAN.md".to_string(),
                is_error: false,
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
        let thread = self.session.active_thread();
        thread
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

    /// If the workgraph has ready milestones, auto-drive them (Plan #2). Reads the
    /// graph, picks the next ready node, and runs a turn for it. Repeats until no
    /// more are ready, up to MAX_AUTO turns, so the user's one command can advance
    /// several milestones in sequence. After each turn, parses the assistant text
    /// for a review verdict and auto-updates the milestone status.
    fn drive_workgraph(&mut self, event_tx: &Sender<AgentEvent>) {
        use crate::workgraph::{NodeStatus, WorkGraph};
        const MAX_AUTO: usize = 3;
        for _ in 0..MAX_AUTO {
            let milestone_id = {
                let g = WorkGraph::read(&self.root);
                match g.next_ready() {
                    Some(n) => n.id,
                    None => break,
                }
            };
            // Build task text (no longer borrowing `g`).
            let (task, title) = {
                let g = WorkGraph::read(&self.root);
                let n = g.get(milestone_id).expect("just read, must exist");
                let t = format!(
                    "workgraph milestone #{}: {}\nacceptance: {}\n\n\
                     Complete this milestone, then self-review. You MUST end your reply \
                     with a final line in EXACTLY this format (nothing after it) so the \
                     kernel can parse and auto-update the milestone status:\n\
                     VERDICT: <pass|needs_fix|rebuild>",
                    n.id,
                    n.title,
                    if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
                );
                (t, n.title.clone())
            };
            self.cancel.reset();
            self.process_turn(task, event_tx);

            // Auto-writeback: parse the turn's final assistant text for a review
            // verdict. When found, update the milestone status accordingly.
            let text = self.last_assistant_text();
            let outcome = crate::review::parse_review(&text);
            if !outcome.unparsed {
                let (status, verdict_str) = match outcome.verdict {
                    crate::review::Verdict::Pass => (NodeStatus::Done, "pass"),
                    crate::review::Verdict::NeedsFix => (NodeStatus::NeedsFix, "needs_fix"),
                    crate::review::Verdict::Rebuild => (NodeStatus::NeedsFix, "rebuild"),
                };
                let _ = WorkGraph::with_lock(&self.root, |g| {
                    g.set_status(milestone_id, status);
                    if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                        n.verdict = Some(verdict_str.to_string());
                    }
                    Ok(())
                });
                let _ = event_tx.send(AgentEvent::Notice(format!(
                    "milestone #{} ({}) auto-updated: {}",
                    milestone_id, title, verdict_str,
                )));

                // Non-blocking auto-memory nudge: after a Pass, prompt the agent
                // to write memory entries. Best-effort — failure is silently ignored.
                if matches!(status, NodeStatus::Done) {
                    let memory_task = format!(
                        "Milestone #{} ({}) passed. Run `use_skill auto-memory` to write \
                         knowledge entries about what you learned. This is non-blocking \
                         — skip if you cannot complete it.",
                        milestone_id, title,
                    );
                    self.cancel.reset();
                    self.process_turn(memory_task, event_tx);
                }
            } else {
                // No VERDICT: line parsed — surface it instead of leaving the
                // milestone silently stuck in_progress.
                let _ = event_tx.send(AgentEvent::Notice(format!(
                    "milestone #{} ({}) ran but emitted no VERDICT: line; status left unchanged",
                    milestone_id, title,
                )));
            }
        }
        // The loop exhausted MAX_AUTO; if a milestone is still ready, more work
        // remains — surface it rather than stopping silently mid-graph.
        if WorkGraph::read(&self.root).next_ready().is_some() {
            let _ = event_tx.send(AgentEvent::Notice(format!(
                "workgraph auto-advance capped at {MAX_AUTO} milestones this turn; more are ready — send another message to continue."
            )));
        }
    }
}

/// 小步写纪律(迭代 2):始终注入,减少单次巨量 write_file 被 max_tokens 截断。
const SMALL_STEP_WRITE_GUIDANCE: &str =
    "When writing a large file, prefer building it up in smaller chunks \
     (multiple append-style edit_file / write_file calls) rather than one \
     giant write_file — a single oversized tool call can be cut off at \
     max_tokens and fail.";

/// Build the System prompt from AGENTS.md (identity) + the already-rendered
/// catalog string (ADR 0020). "Filesystem as self": both come from disk, not
/// hardcoded. The caller renders the catalog under a brief Registry read-lock
/// and DROPS that lock before this function does any disk I/O
/// (`AGENTS.md` read, `WorkGraph::read`) — no `RwLock` guard may span those
/// reads, otherwise the reload thread's writer would be blocked (and a panic
/// during I/O would poison the lock).
fn build_system_prompt_with_catalog(root: &std::path::Path, catalog: &str) -> String {
    let mut parts = Vec::new();
    parts.push(SMALL_STEP_WRITE_GUIDANCE.to_string());
    if let Ok(agents) = std::fs::read_to_string(root.join("AGENTS.md")) {
        let agents = agents.trim();
        if !agents.is_empty() {
            parts.push(agents.to_string());
        }
    }
    if !catalog.is_empty() {
        parts.push(catalog.to_string());
    }
    // Append workgraph status (Plan #2) so the agent is always aware of
    // outstanding milestones. Renders nothing when the graph is empty.
    let wg = crate::workgraph::WorkGraph::read(root);
    let wg_text = wg.render_for_prompt();
    if !wg_text.is_empty() {
        parts.push(wg_text);
    }
    parts.join("\n\n")
}

/// 兼容旧路径：自扫一次。TUI/sub-agent 用此（无共享 Registry）。
fn build_system_prompt(root: &std::path::Path) -> String {
    let catalog = Registry::scan(root).render_catalog();
    build_system_prompt_with_catalog(root, &catalog)
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
    use crate::provider::{Completion, CompletionRequest, StopReason};
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
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
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
                }.into())
            } else {
                // The tool result must have been fed back into the request.
                let fed = req.messages.iter().any(|m| {
                    m.role == Role::Tool
                        && m.items.iter().any(|it| matches!(it,
                            MessageItem::ToolResult { output, .. } if output.contains("hello world")))
                });
                *self.saw_result.lock().unwrap() = fed;
                Ok(Message::text(0, Role::Assistant, "done").into())
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
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "a1".into(),
                        name: "agent".into(),
                        args: serde_json::json!({ "task": "research the thing" }),
                    }],
                }.into()),
                1 => Ok(Message::text(0, Role::Assistant, "sub findings").into()),
                _ => Ok(Message::text(0, Role::Assistant, "final answer").into()),
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
        assert!(agent.session.active_thread().iter().any(|m| m.role == Role::Tool
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
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "q1".into(),
                        name: "ask_user".into(),
                        args: serde_json::json!({ "question": "favorite color?" }),
                    }],
                }.into())
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
                Ok(Message::text(0, Role::Assistant, format!("you said {answer}")).into())
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
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "p1".into(),
                        name: "plan".into(),
                        args: serde_json::json!({ "steps": ["do a", "do b"] }),
                    }],
                }.into())
            } else {
                Ok(Message::text(0, Role::Assistant, "proceeding").into())
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
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Message {
                    id: 0,
                    role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: "c1".into(),
                        name: "confirm".into(),
                        args: serde_json::json!({ "prompt": "proceed?" }),
                    }],
                }.into())
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
                Ok(Message::text(0, Role::Assistant, format!("answer={ans}")).into())
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
        let last = agent.session.entries.last().unwrap();
        assert!(matches!(&last.message.items[0], MessageItem::Text { text } if text == "done"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn headless_auto_denies_unauthorized_ask_tool_without_prompting() {
        use std::sync::mpsc::channel;
        // Scripted provider: first reply calls write_file; second reply is bare text
        // so the tool loop terminates.
        struct WriteThenStop { n: std::sync::Mutex<u32> }
        impl Provider for WriteThenStop {
            fn name(&self) -> &str { "write-then-stop" }
            fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
                let mut n = self.n.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    Ok(Message::new(0, Role::Assistant, vec![MessageItem::ToolCall {
                        id: "c1".into(), name: "write_file".into(),
                        args: serde_json::json!({"path": "hacked.txt", "content": "x"}),
                    }]).into())
                } else {
                    Ok(Message::text(0, Role::Assistant, "done").into())
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("cc_bg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = std::sync::Arc::new(WriteThenStop { n: std::sync::Mutex::new(0) });
        let mut agent = AgentLoop::new_background(provider, "test-model", 4096, 0.0, dir.clone());
        let (tx, rx) = channel();
        agent.run_one_turn("write a file".into(), &tx);
        drop(tx);
        let events: Vec<_> = rx.into_iter().collect();
        // No permission prompt was emitted (no one to answer in headless).
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })),
            "headless must not emit PermissionRequest");
        // The unauthorized write did not happen.
        assert!(!dir.join("hacked.txt").exists(), "unauthorized write_file must be denied");
        // ADR 0028: the project is untrusted (headless, no recorded decision) so
        // the codecoder.json allowlist is NOT loaded. The denial must name that
        // root cause and offer the remediation — not just "not in allowlist".
        let denial = events.iter().find_map(|e| match e {
            AgentEvent::ToolFinished { is_error: true, output, .. } => Some(output.as_str()),
            _ => None,
        });
        if let Some(msg) = denial {
            assert!(msg.contains("not trusted"), "denial should name the trust root cause: {msg}");
            assert!(msg.contains("NOT loaded"), "denial should explain the allowlist is not loaded: {msg}");
            assert!(msg.contains("CODECODER_DEFAULT_TRUST"), "denial should offer the remediation: {msg}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn turn_emits_notice_when_tool_iteration_cap_hit() {
        use std::sync::mpsc::channel;
        // A provider that ALWAYS requests a read-only tool (Permission::None, so
        // it runs without prompts). The loop must exhaust MAX_TOOL_ITERATIONS and
        // surface a cap Notice rather than stopping silently.
        struct AlwaysTool;
        impl Provider for AlwaysTool {
            fn name(&self) -> &str { "always-tool" }
            fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
                Ok(Message::new(0, Role::Assistant, vec![MessageItem::ToolCall {
                    id: "c".into(), name: "list_directory".into(), args: serde_json::json!({}),
                }]).into())
            }
        }
        let dir = std::env::temp_dir().join(format!("cc_cap_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = std::sync::Arc::new(AlwaysTool);
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());
        let (tx, rx) = channel();
        agent.run_one_turn("go".into(), &tx);
        drop(tx);
        let events: Vec<_> = rx.into_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("tool-iteration cap"))),
            "exhausting MAX_TOOL_ITERATIONS should emit a cap Notice (found {} events, none matching)",
            events.len()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// call 0 → a tool call in a response the provider TRUNCATED at max_tokens
    /// (StopReason::Length); call 1 → recovery text. The truncated tool call must
    /// NOT be executed (its args may be half-serialized), roadmap #1 / ADR 0027.
    struct TruncatedToolCall {
        calls: AtomicUsize,
    }
    impl Provider for TruncatedToolCall {
        fn name(&self) -> &str {
            "truncated"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Completion {
                    message: Message::new(
                        0,
                        Role::Assistant,
                        vec![MessageItem::ToolCall {
                            id: "t1".into(),
                            name: "read_file".into(),
                            args: serde_json::json!({ "path": "note.txt" }),
                        }],
                    ),
                    stop_reason: StopReason::Length,
                    usage: None,
                })
            } else {
                Ok(Message::text(0, Role::Assistant, "recovered").into())
            }
        }
    }

    #[test]
    fn truncated_tool_call_is_not_executed_and_loop_recovers() {
        let dir = std::env::temp_dir().join(format!("cc_trunc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "hello world").unwrap();

        let provider = Arc::new(TruncatedToolCall { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        agent.process_turn("read note.txt".into(), &etx);
        drop(etx);
        let events: Vec<_> = erx.into_iter().collect();

        // The truncated tool call must never have started executing.
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "read_file")),
            "a truncated tool call must not be dispatched"
        );
        // Instead, an error ToolResult was fed back so the model can retry.
        assert!(
            agent.session.active_thread().iter().any(|m| m.role == Role::Tool
                && m.items.iter().any(|it| matches!(it,
                    MessageItem::ToolResult { output, is_error, .. }
                        if *is_error && output.contains("truncated")))),
            "an is_error truncation ToolResult should be appended"
        );
        // ...and the loop continued to the recovery turn.
        assert_eq!(agent.last_assistant_text(), "recovered");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 记录每次请求的 max_tokens;前 fail_times 次以 Length 截断(空 tool_calls),其后正常 Stop。
    struct RecordingLengthProvider {
        fail_times: usize,
        calls: Mutex<usize>,
        seen_max_tokens: Mutex<Vec<u32>>,
    }
    impl Provider for RecordingLengthProvider {
        fn name(&self) -> &str {
            "recording-length"
        }
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
            self.seen_max_tokens.lock().unwrap().push(req.max_tokens);
            let mut c = self.calls.lock().unwrap();
            let i = *c;
            *c += 1;
            let msg = Message {
                id: 0,
                role: Role::Assistant,
                items: vec![MessageItem::Text {
                    text: if i < self.fail_times { "partial".into() } else { "done".into() },
                }],
            };
            let stop = if i < self.fail_times { StopReason::Length } else { StopReason::Stop };
            Ok(Completion { message: msg, stop_reason: stop, usage: None })
        }
    }

    #[test]
    fn length_stop_bumps_effective_max_tokens_on_retry() {
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(RecordingLengthProvider {
            fail_times: 2,
            calls: Mutex::new(0),
            seen_max_tokens: Mutex::new(vec![]),
        });
        let mut agent =
            AgentLoop::new(p.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
        agent.set_max_tokens_ceiling(4096);
        let (tx, _rx) = std::sync::mpsc::channel();
        agent.run_one_turn("go".into(), &tx);
        let seen = p.seen_max_tokens.lock().unwrap().clone();
        // 256 → 截断 → 512 → 截断 → 1024 → Stop。翻倍链可见。
        assert_eq!(seen, vec![256, 512, 1024], "seen={seen:?}");
    }

    #[test]
    fn effective_max_tokens_caps_at_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        // 恒截断(fail_times 极大),空 tool_calls → 达 ceiling 后收尾。
        let p = Arc::new(RecordingLengthProvider {
            fail_times: 99,
            calls: Mutex::new(0),
            seen_max_tokens: Mutex::new(vec![]),
        });
        let mut agent =
            AgentLoop::new(p.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
        agent.set_max_tokens_ceiling(1024);
        let (tx, _rx) = std::sync::mpsc::channel();
        agent.run_one_turn("go".into(), &tx);
        let seen = p.seen_max_tokens.lock().unwrap().clone();
        // 256 → 512 → 1024(封顶,空 tool_calls → 不再翻倍,收尾)。绝不超过 1024。
        assert_eq!(seen, vec![256, 512, 1024], "seen={seen:?}");
        assert!(seen.iter().all(|&m| m <= 1024));
    }

    #[test]
    fn bump_resets_per_turn() {
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(RecordingLengthProvider {
            fail_times: 1,
            calls: Mutex::new(0),
            seen_max_tokens: Mutex::new(vec![]),
        });
        let mut agent =
            AgentLoop::new(p.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
        agent.set_max_tokens_ceiling(4096);
        let (tx, _rx) = std::sync::mpsc::channel();
        agent.run_one_turn("t1".into(), &tx); // 256 → 截断 → 512 → Stop
        // 下一 turn 让它立即 Stop:重置 calls 让 fail_times 已过。
        agent.run_one_turn("t2".into(), &tx); // 首个请求应回到 256(重置),非 512
        let seen = p.seen_max_tokens.lock().unwrap().clone();
        assert_eq!(seen[0], 256); // turn1 起点
        assert_eq!(seen[1], 512); // turn1 bump
        assert_eq!(seen[2], 256, "turn2 应从 self.max_tokens 重置, seen={seen:?}");
    }

    // --- no-op 探索兜底(迭代 4)---
    use std::sync::Mutex as StdMutex2;

    /// 按脚本逐次产出工具调用或结束文本:Some(name)→ToolCall,None→纯文本(结束 turn)。
    struct ScriptedTools {
        script: Vec<Option<&'static str>>,
        calls: StdMutex2<usize>,
    }
    impl Provider for ScriptedTools {
        fn name(&self) -> &str { "scripted-tools" }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            use crate::message::{Message, MessageItem, Role};
            let mut c = self.calls.lock().unwrap();
            let i = *c; *c += 1;
            let step = self.script.get(i).copied().flatten();
            let msg = match step {
                Some(name) => Message {
                    id: 0, role: Role::Assistant,
                    items: vec![MessageItem::ToolCall {
                        id: format!("t{i}"), name: name.to_string(),
                        args: serde_json::json!({"pattern": "*"}),
                    }],
                },
                None => Message {
                    id: 0, role: Role::Assistant,
                    items: vec![MessageItem::Text { text: "done".into() }],
                },
            };
            Ok(msg.into())
        }
    }

    fn count_noop_notices(rx: std::sync::mpsc::Receiver<AgentEvent>) -> usize {
        rx.into_iter().filter(|e| matches!(e, AgentEvent::Notice(m) if m.contains("no-op backstop"))).count()
    }

    fn run_scripted(script: Vec<Option<&'static str>>, threshold: usize) -> usize {
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(ScriptedTools { script, calls: StdMutex2::new(0) });
        let mut agent = AgentLoop::new(p as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
        agent.set_noop_nudge_threshold(threshold);
        let (tx, rx) = std::sync::mpsc::channel();
        agent.run_one_turn("go".into(), &tx);
        drop(tx);
        count_noop_notices(rx)
    }

    #[test]
    fn noop_backstop_nudges_after_threshold_explore_steps() {
        // glob×3 → 达阈值 3 → 恰一次 nudge;随后 text 结束。
        let n = run_scripted(vec![Some("glob"), Some("glob"), Some("glob"), None], 3);
        assert_eq!(n, 1, "expected exactly one no-op nudge, got {n}");
    }

    #[test]
    fn noop_backstop_no_nudge_under_threshold() {
        let n = run_scripted(vec![Some("glob"), Some("glob"), None], 3);
        assert_eq!(n, 0);
    }

    #[test]
    fn noop_backstop_disabled_when_threshold_zero() {
        let n = run_scripted(vec![Some("glob"), Some("glob"), Some("glob"), Some("glob"), None], 0);
        assert_eq!(n, 0);
    }

    #[test]
    fn noop_backstop_resets_on_non_exploration_tool() {
        // glob,glob,milestone(重置),glob,glob → 连续从不达 3 → 不 nudge。
        let n = run_scripted(vec![Some("glob"), Some("glob"), Some("milestone"), Some("glob"), Some("glob"), None], 3);
        assert_eq!(n, 0);
    }

    /// Errors with a transient 503 on the first call, then succeeds — exercises
    /// the retry loop (ADR 0027 #3).
    struct FlakyProvider {
        calls: AtomicUsize,
    }
    impl Provider for FlakyProvider {
        fn name(&self) -> &str {
            "flaky"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                anyhow::bail!("OpenAI API returned 503: service unavailable");
            }
            Ok(Message::text(0, Role::Assistant, "recovered after retry").into())
        }
    }

    #[test]
    fn transient_error_is_retried_then_succeeds() {
        let dir = std::env::temp_dir().join(format!("cc_flaky_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(FlakyProvider { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        agent.process_turn("go".into(), &etx);
        drop(etx);
        let events: Vec<_> = erx.into_iter().collect();

        assert_eq!(provider.calls.load(Ordering::SeqCst), 2, "should retry once then succeed");
        assert_eq!(agent.last_assistant_text(), "recovered after retry");
        assert!(
            events.iter().any(|e| matches!(e, AgentEvent::Notice(m) if m.contains("retrying"))),
            "a retry Notice should be emitted"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Always errors with a permanent 401 — must NOT be retried.
    struct AuthFailProvider {
        calls: AtomicUsize,
    }
    impl Provider for AuthFailProvider {
        fn name(&self) -> &str {
            "auth-fail"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("OpenAI API returned 401: invalid api key");
        }
    }

    #[test]
    fn permanent_error_is_not_retried() {
        let dir = std::env::temp_dir().join(format!("cc_authfail_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(AuthFailProvider { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.clone());

        let (etx, erx) = channel();
        agent.process_turn("go".into(), &etx);
        drop(etx);
        let events: Vec<_> = erx.into_iter().collect();

        assert_eq!(provider.calls.load(Ordering::SeqCst), 1, "permanent error must not be retried");
        assert!(
            events.iter().any(|e| matches!(e, AgentEvent::StreamDelta(m) if m.contains("error"))),
            "the error should be surfaced"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn provider_error_sets_last_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("cc_lasterr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(AuthFailProvider { calls: AtomicUsize::new(0) });
        let mut agent = AgentLoop::new(provider.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.clone());
        let (etx, erx) = channel();
        agent.process_turn("go".into(), &etx);
        drop(etx);
        for _ in erx {}
        assert!(agent.last_error().is_some(), "provider error should set last_error");
        assert!(
            agent.last_error().unwrap().contains("401"),
            "got: {:?}", agent.last_error()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// call 0 → plain text (no tool calls) but pushes a steering message mid-call,
    /// simulating the user typing while the turn runs; call 1 → reports whether the
    /// steering `User` message was in context (ADR 0029).
    struct SteeringProvider {
        calls: AtomicUsize,
        steer: SteerQueue,
    }
    impl Provider for SteeringProvider {
        fn name(&self) -> &str {
            "steering"
        }
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.steer.push("actually, focus on X".into());
                Ok(Message::text(0, Role::Assistant, "working").into())
            } else {
                let saw = req.messages.iter().any(|m| {
                    m.role == Role::User
                        && m.items.iter().any(|it| matches!(it,
                            MessageItem::Text { text } if text.contains("focus on X")))
                });
                Ok(Message::text(0, Role::Assistant, if saw { "steered" } else { "missed" }).into())
            }
        }
    }

    #[test]
    fn follow_up_steering_restarts_turn_instead_of_stopping() {
        let dir = std::env::temp_dir().join(format!("cc_steer_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let steer = SteerQueue::default();
        let provider = Arc::new(SteeringProvider { calls: AtomicUsize::new(0), steer: steer.clone() });
        let mut agent = AgentLoop::new(provider.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.clone());
        agent.steer = steer.clone(); // agent drains the same handle the provider pushes to

        let (etx, erx) = channel();
        agent.process_turn("start".into(), &etx);
        drop(etx);
        let _events: Vec<_> = erx.into_iter().collect();

        // The turn did not stop after the tool-less first reply — steering restarted it.
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2, "steering should restart the turn");
        assert_eq!(agent.last_assistant_text(), "steered", "the steering message must be in context");
        // The steering message is an ordinary User turn in the session.
        assert!(agent.session.active_thread().iter().any(|m| m.role == Role::User
            && m.items.iter().any(|it| matches!(it,
                MessageItem::Text { text } if text.contains("focus on X")))));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// call 0 → a tool call, and pushes steering mid-call; call 1 (after the tool
    /// result, once the iteration-top drain has injected the steering) → reports
    /// whether the steering was in context (ADR 0029, mid-tool-loop steering).
    struct SteeringToolProvider {
        calls: AtomicUsize,
        steer: SteerQueue,
    }
    impl Provider for SteeringToolProvider {
        fn name(&self) -> &str {
            "steering-tool"
        }
        fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.steer.push("switch to Y".into());
                Ok(Message::new(
                    0,
                    Role::Assistant,
                    vec![MessageItem::ToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        args: serde_json::json!({ "path": "note.txt" }),
                    }],
                )
                .into())
            } else {
                let saw = req.messages.iter().any(|m| {
                    m.role == Role::User
                        && m.items.iter().any(|it| matches!(it,
                            MessageItem::Text { text } if text.contains("switch to Y")))
                });
                Ok(Message::text(0, Role::Assistant, if saw { "steered" } else { "missed" }).into())
            }
        }
    }

    #[test]
    fn steering_injects_mid_tool_loop() {
        let dir = std::env::temp_dir().join(format!("cc_steertool_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "hello").unwrap();
        let steer = SteerQueue::default();
        let provider = Arc::new(SteeringToolProvider { calls: AtomicUsize::new(0), steer: steer.clone() });
        let mut agent = AgentLoop::new(provider as Arc<dyn Provider>, "m", 256, 0.0, dir.clone());
        agent.steer = steer.clone();

        let (etx, erx) = channel();
        agent.process_turn("start".into(), &etx);
        drop(etx);
        let _events: Vec<_> = erx.into_iter().collect();

        assert_eq!(agent.last_assistant_text(), "steered", "steering must be injected before the next call");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Returns a fixed summary text for any tier-2 summarization request.
    struct SummaryProvider;
    impl Provider for SummaryProvider {
        fn name(&self) -> &str {
            "summary"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
            Ok(Message::text(0, Role::Assistant, "SUMMARY-PROSE").into())
        }
    }

    #[test]
    fn context_working_set_summarizes_and_appends_file_blocks() {
        let dir = std::env::temp_dir().join(format!("cc_compact_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(SummaryProvider);
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());
        // Force compaction regardless of real token counts.
        agent.model_window = 10;
        // Build entries manually for the compaction test. The linear vec maps to
        // a chain (parent = previous id, leaf = last id).
        let mut entries: Vec<session::SessionEntry> = Vec::new();
        let msgs = vec![
            Message::text(0, Role::User, "goal"), // anchor
            Message {
                id: 1,
                role: Role::Assistant,
                items: vec![MessageItem::ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    args: serde_json::json!({ "path": "foo.rs" }),
                }],
            },
            Message {
                id: 2,
                role: Role::Tool,
                items: vec![MessageItem::ToolResult {
                    call_id: "c1".into(),
                    output: "contents".into(),
                    is_error: false,
                }],
            },
            Message::text(3, Role::Assistant, "did stuff"),
            Message::text(4, Role::User, "next"), // last user → span = ids 1..=3
            Message::text(5, Role::Assistant, "ok"),
        ];
        let mut prev: Option<u64> = None;
        for m in &msgs {
            entries.push(session::SessionEntry { message: m.clone(), parent: prev, meta: None });
            prev = Some(m.id);
        }
        agent.session.entries = entries;
        agent.session.leaf = Some(5);
        let (tx, _rx) = std::sync::mpsc::channel();

        let out = agent.context_working_set(&tx);
        let sys = out
            .iter()
            .find(|m| m.role == Role::System)
            .and_then(|m| m.items.iter().find_map(|it| match it {
                MessageItem::Text { text } => Some(text.clone()),
                _ => None,
            }))
            .expect("a System summary message should be inserted");
        assert!(sys.contains("SUMMARY-PROSE"), "got: {sys}");
        assert!(sys.contains("<read-files>"), "got: {sys}");
        assert!(sys.contains("foo.rs"), "got: {sys}");

        // Second call with an unchanged span reuses the cache (no panic, same blocks).
        let out2 = agent.context_working_set(&tx);
        assert!(out2.iter().any(|m| m.role == Role::System
            && m.items.iter().any(|it| matches!(it, MessageItem::Text { text } if text.contains("foo.rs")))));

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Trust gating (ADR 0028) ────────────────────────────────────────────────
    // These tests mutate the global CODECODER_TRUST_FILE env var; serialize them.
    static TRUST_ENV: Mutex<()> = Mutex::new(());

    fn stub_provider() -> Arc<dyn Provider> {
        Arc::new(crate::provider::stub::StubClient)
    }

    #[test]
    fn max_tokens_ceiling_defaults_and_setter_overrides() {
        let _g = crate::config::MAX_TOKENS_CEILING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("CODECODER_MAX_TOKENS_CEILING"); }
        let dir = tempfile::tempdir().unwrap();
        let mut agent = AgentLoop::new(stub_provider(), "m", 256, 0.0, dir.path().to_path_buf());
        // 默认来自 Config::from_env()(env 未设 → 32768)。
        assert_eq!(agent.max_tokens_ceiling, 32768);
        agent.set_max_tokens_ceiling(1024);
        assert_eq!(agent.max_tokens_ceiling, 1024);
    }

    #[test]
    fn untrusted_project_skips_agents_md_and_allowlist() {
        let _g = TRUST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("cc_untrust_{}", std::process::id()));
        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("AGENTS.md"), "EVIL injected identity").unwrap();
        std::fs::write(proj.join("codecoder.json"), r#"{"allowlist":["run_command:rm"]}"#).unwrap();

        let store = base.join("trust.json");
        unsafe { std::env::set_var("CODECODER_TRUST_FILE", &store) };
        crate::trust::record(&proj, crate::trust::TrustDecision::Untrusted);

        let agent = AgentLoop::new(stub_provider(), "m", 256, 0.0, proj.clone());
        assert_eq!(agent.trust, TrustState::Untrusted);
        assert!(agent.system_prompt.is_empty(), "untrusted AGENTS.md must not enter the system prompt");
        assert!(
            !agent.project_allowlist.allows("run_command:rm"),
            "untrusted codecoder.json allowlist must not load"
        );

        unsafe { std::env::remove_var("CODECODER_TRUST_FILE") };
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn pending_project_prompts_and_loads_self_on_always() {
        let _g = TRUST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("cc_pend_always_{}", std::process::id()));
        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("AGENTS.md"), "PENDING-IDENTITY").unwrap();

        let store = base.join("trust.json");
        unsafe { std::env::set_var("CODECODER_TRUST_FILE", &store) };
        // No recorded decision + config on disk + interactive → Pending.
        let mut agent = AgentLoop::new(stub_provider(), "m", 256, 0.0, proj.clone());
        assert_eq!(agent.trust, TrustState::Pending);
        assert!(agent.system_prompt.is_empty(), "pending must not load self yet");

        let (etx, erx) = channel();
        let responder = thread::spawn(move || {
            for ev in erx {
                if let AgentEvent::TrustPrompt { reply_tx, .. } = ev {
                    let _ = reply_tx.send(TrustReply::Always);
                }
            }
        });
        agent.process_turn("hi".into(), &etx);
        drop(etx);
        responder.join().unwrap();

        assert!(agent.system_prompt.contains("PENDING-IDENTITY"), "self loads after Always");
        // Always persists, so a fresh agent in the same project is now Trusted.
        assert_eq!(crate::trust::decide(&proj), Some(crate::trust::TrustDecision::Trusted));

        unsafe { std::env::remove_var("CODECODER_TRUST_FILE") };
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn pending_project_skips_self_on_never() {
        let _g = TRUST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("cc_pend_never_{}", std::process::id()));
        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("AGENTS.md"), "PENDING-IDENTITY").unwrap();

        let store = base.join("trust.json");
        unsafe { std::env::set_var("CODECODER_TRUST_FILE", &store) };
        let mut agent = AgentLoop::new(stub_provider(), "m", 256, 0.0, proj.clone());
        assert_eq!(agent.trust, TrustState::Pending);

        let (etx, erx) = channel();
        let responder = thread::spawn(move || {
            for ev in erx {
                if let AgentEvent::TrustPrompt { reply_tx, .. } = ev {
                    let _ = reply_tx.send(TrustReply::Never);
                }
            }
        });
        agent.process_turn("hi".into(), &etx);
        drop(etx);
        responder.join().unwrap();

        assert_eq!(agent.trust, TrustState::Untrusted);
        assert!(agent.system_prompt.is_empty(), "self must stay unloaded after Never");
        assert_eq!(crate::trust::decide(&proj), Some(crate::trust::TrustDecision::Untrusted));

        unsafe { std::env::remove_var("CODECODER_TRUST_FILE") };
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn trusted_project_loads_agents_md_and_allowlist() {
        let _g = TRUST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("cc_trustload_{}", std::process::id()));
        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("AGENTS.md"), "IDENTITY-MARKER").unwrap();
        std::fs::write(proj.join("codecoder.json"), r#"{"allowlist":["run_command:git"]}"#).unwrap();

        let store = base.join("trust.json");
        unsafe { std::env::set_var("CODECODER_TRUST_FILE", &store) };
        crate::trust::record(&proj, crate::trust::TrustDecision::Trusted);

        let agent = AgentLoop::new(stub_provider(), "m", 256, 0.0, proj.clone());
        assert_eq!(agent.trust, TrustState::Trusted);
        assert!(agent.system_prompt.contains("IDENTITY-MARKER"), "trusted AGENTS.md loads");
        assert!(agent.project_allowlist.allows("run_command:git"), "trusted allowlist loads");

        unsafe { std::env::remove_var("CODECODER_TRUST_FILE") };
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn build_system_prompt_uses_provided_registry() {
        use crate::registry::Registry;
        let dir = std::env::temp_dir().join(format!("cc_regshare_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(
            dir.join("skills/shared-skill.md"),
            "---\nname: shared-skill\ndescription: a shared skill\n---\nbody",
        ).unwrap();
        let reg = Registry::scan(&dir);
        // Caller renders the catalog (under whatever lock policy it chooses),
        // then passes the string — disk I/O inside the builder touches no lock.
        let catalog = reg.render_catalog();
        let prompt = build_system_prompt_with_catalog(&dir, &catalog);
        assert!(prompt.contains("shared-skill — a shared skill"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_prompt_includes_small_step_write_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let p = build_system_prompt(dir.path());
        assert!(p.contains("append"), "应含小步写引导, prompt={p}");
        assert!(p.to_lowercase().contains("max_tokens"), "应解释原因, prompt={p}");
    }

    #[test]
    fn refresh_system_prompt_picks_up_new_skill() {
        use std::sync::{Arc, RwLock};
        let dir = std::env::temp_dir().join(format!("cc_refresh_shared_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(
            dir.join("skills/old.md"),
            "---\nname: old\ndescription: o\n---\nbody",
        ).unwrap();
        let reg = Arc::new(RwLock::new(crate::registry::Registry::scan(&dir)));
        let mut agent = AgentLoop::new_daemon(
            Arc::new(crate::provider::stub::StubClient),
            String::from("gpt-4o"),
            4096,
            0.7,
            dir.clone(),
            reg.clone(),
        );
        agent.trust = crate::agent::TrustState::Trusted;
        // shared registry gains a new skill AFTER construction
        std::fs::write(
            dir.join("skills/new.md"),
            "---\nname: new\ndescription: n\n---\nbody",
        ).unwrap();
        reg.write().unwrap().reload(&dir);
        agent.refresh_system_prompt_if_shared();
        assert!(agent.system_prompt.contains("new"), "refresh must pick up the new skill");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_session_meta_mark_writes_current_leaf_meta() {
        use crate::message::{Message, Role};
        let dir = std::env::temp_dir().join(format!("cc_metamark_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut agent = AgentLoop::new(
            std::sync::Arc::new(crate::provider::stub::StubClient),
            "gpt-4o", 4096, 0.7, dir.clone(),
        );
        // give the session a leaf entry (id 0)
        agent.session.append(Message::text(0, Role::User, "hi"));
        assert_eq!(agent.session.leaf, Some(0));

        let mark = serde_json::json!({"causal_node": 5, "status": "hypothesis"});
        agent.apply_session_meta_mark(mark.clone());

        let meta = agent.session.entry_by_id(0).unwrap().meta.clone();
        assert_eq!(meta, Some(mark), "leaf meta must equal the applied mark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn navigate_abandon_records_ruling_on_linked_branch() {
        use crate::message::{Message, Role};
        let dir = std::env::temp_dir().join(format!("cc_navruling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // seed a causal tree with node 1
        std::fs::write(
            dir.join("causal_tree.json"),
            r#"{"nodes":[{"id":1,"question":"why slow?","status":"hypothesis"}],"next_id":2}"#,
        ).unwrap();

        // Create a scripted provider that returns a summary for the abandoned branch
        struct SummaryProvider {
            calls: std::sync::atomic::AtomicUsize,
        }
        impl crate::provider::Provider for SummaryProvider {
            fn name(&self) -> &str { "summary" }
            fn complete(&self, _req: &crate::provider::CompletionRequest) -> anyhow::Result<crate::provider::Completion> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Message::text(0, Role::Assistant, "ruled out: alternative approach failed").into())
            }
        }

        let mut agent = AgentLoop::new(
            std::sync::Arc::new(SummaryProvider { calls: std::sync::atomic::AtomicUsize::new(0) }),
            "gpt-4o", 4096, 0.7, dir.clone(),
        );
        // build a session tree: root(0) -> A(1, linked to causal node 1) ; navigate to 0 then append B(2)
        agent.session.append(Message::text(0, Role::User, "root"));
        agent.session.append(Message {
            id: 1, role: Role::Assistant,
            items: vec![crate::message::MessageItem::Text { text: "trying hypothesis".into() }],
        });
        // mark entry 1's leaf as linked (simulate `reason link`)
        agent.session.leaf = Some(1);
        agent.session.update_meta(1, |m| *m = Some(serde_json::json!({"causal_node":1,"status":"hypothesis"})));

        // Navigate to root (0): abandons entry 1
        let (tx, _rx) = std::sync::mpsc::channel::<AgentEvent>();
        agent.handle_navigate(0, &tx);

        // After navigation away from entry 1, entry 1's meta should be ruled_out:
        let m = agent.session.entry_by_id(1).unwrap().meta.clone();
        assert!(matches!(m, Some(ref v) if v.get("status") == Some(&serde_json::json!("ruled_out"))));
        let ruling = m.as_ref().and_then(|v| v.get("ruling").and_then(|r| r.as_str()));
        assert_eq!(ruling, Some("ruled out: alternative approach failed"));

        // Causal tree node 1 should have the ruling
        let causal_content = std::fs::read_to_string(dir.join("causal_tree.json")).unwrap();
        let causal: serde_json::Value = serde_json::from_str(&causal_content).unwrap();
        let node = causal["nodes"][0].clone();
        assert_eq!(node["ruling"], serde_json::json!("ruled out: alternative approach failed"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_review_returns_structured_outcome_under_stub() {
        use std::sync::mpsc::channel;
        let dir = tempfile::tempdir().unwrap();
        let mut agent = AgentLoop::new_background(
            stub_provider(),
            "stub".to_string(), 256, 0.0, dir.path().to_path_buf(),
        );
        let (tx, _rx) = channel();
        let (outcome, _raw) = agent.run_review("the current changes", &tx);
        // Stub yields parseable-or-default review text → a concrete verdict exists.
        let _ = outcome.verdict; // does not panic; type is ReviewOutcome
    }

    #[test]
    fn truncated_tool_call_error_mentions_append() {
        let err_msg = "tool call truncated: the response hit max_tokens before the \
         arguments finished. Not executed. The model's reasoning was too long, \
         leaving no room for the file content. Retry with a MUCH shorter thought \
         process, or if this was a write_file, consider whether the file was \
         partially created — you can append to it with append=true.";
        assert!(err_msg.contains("append=true"), "error should mention append=true");
    }
}
