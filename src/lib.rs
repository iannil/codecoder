// src/lib.rs — public library surface for the behavioral test harness.
// main.rs is a thin shim over run(). Integration tests compile against THIS
// public API only — the black-box boundary is compiler-enforced.
#![allow(dead_code)]

pub mod agent;
pub mod background;
pub mod capability;
pub mod compaction;
pub mod config;
pub mod memory;
pub mod message;
pub mod permission;
pub mod provider;
pub mod registry;
pub mod retry;
pub mod review;
pub mod session;
pub mod tokenizer;
pub mod trust;
pub mod workgraph;
pub mod tool;
pub mod tui;
pub mod verify;
pub mod daemon;
pub mod client;

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

pub use agent::{AgentCommand, AgentEvent, AgentLoop, PermissionReply, SteerQueue, TrustReply};
pub use background::BgOutcome;
pub use config::Config;
pub use message::{Message, MessageItem, Role};
pub use permission::PermScope;
pub use provider::{Completion, CompletionRequest, Provider, StopReason};

/// Provider selection (ADR 0017). An env hook allows a scripted provider to be
/// injected for the pty smoke layer (L2); real runs use OpenAI or the stub.
pub fn select_provider(cfg: &Config) -> Arc<dyn Provider> {
    if let Ok(path) = std::env::var("CODECODER_SCRIPT") {
        return Arc::new(provider::stub::ScriptFileProvider::from_path(&path));
    }
    match cfg.api_key.as_deref() {
        Some(_) => Arc::new(provider::openai::OpenAiClient::new(cfg)),
        None => Arc::new(provider::stub::StubClient),
    }
}

/// Headless Background Agent entry (ADR 0026): pick the provider, run one task,
/// print a report to stdout, persist the session. Scheduling is external.
pub fn run_background(cfg: Config, task: String) -> anyhow::Result<()> {
    let provider = select_provider(&cfg);
    let outcome = background::run_background(
        provider,
        cfg.model.clone(),
        cfg.max_tokens,
        cfg.temperature,
        cfg.root.clone(),
        task,
    )?;
    println!("=== background agent result ===");
    if !outcome.final_text.trim().is_empty() {
        println!("{}", outcome.final_text.trim());
    }
    if !outcome.tool_calls.is_empty() {
        println!("tools executed: {}", outcome.tool_calls.join(", "));
    }
    if !outcome.denied.is_empty() {
        println!("denied/errors: {}", outcome.denied.join(" | "));
    }
    println!("=== summary: {} tools, {} denied ===", outcome.tool_calls.len(), outcome.denied.len());
    Ok(())
}

/// TUI 入口（ADR 0016/0024）：起 agent 线程 + 跑 ratatui 主循环。
pub fn run_tui(cfg: Config) -> anyhow::Result<()> {
    let provider = select_provider(&cfg);
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();

    let agent = AgentLoop::new(
        provider,
        cfg.model.clone(),
        cfg.max_tokens,
        cfg.temperature,
        cfg.root.clone(),
    );
    // Clone the cancel token before moving the agent into its thread so the TUI
    // can interrupt an in-flight turn directly (ADR 0016).
    let cancel = agent.cancel_token();
    let steer = agent.steer_handle();
    let agent_thread = thread::spawn(move || agent.run(cmd_rx, event_tx));

    let result = tui::run::run(cfg.model.clone(), cfg.root.clone(), cmd_tx.clone(), event_rx, cancel, steer);

    let _ = cmd_tx.send(AgentCommand::Shutdown);
    let _ = agent_thread.join();
    capability::shutdown_all();
    result
}

/// Daemon 入口（client-server 架构）：起长驻 daemon，无 TUI。socket/session 逻辑
/// 在 `daemon::Daemon::run` 中（Task 2 起填充）。
pub fn run_daemon(cfg: Config) -> anyhow::Result<()> {
    let daemon = daemon::Daemon::new(cfg);
    daemon.run()
}
