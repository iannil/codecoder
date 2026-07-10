// src/lib.rs — public library surface for the behavioral test harness.
// main.rs is a thin shim over run(). Integration tests compile against THIS
// public API only — the black-box boundary is compiler-enforced.
#![allow(dead_code)]

pub mod agent;
pub mod capability;
pub mod compaction;
pub mod config;
pub mod memory;
pub mod message;
pub mod permission;
pub mod provider;
pub mod registry;
pub mod session;
pub mod tokenizer;
pub mod tool;
pub mod tui;

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

pub use agent::{AgentCommand, AgentEvent, AgentLoop, PermissionReply};
pub use config::Config;
pub use message::{Message, MessageItem, Role};
pub use permission::PermScope;
pub use provider::{CompletionRequest, Provider};

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

/// Kernel wiring (ADR 0016): OS threads + channels; TUI owns the main thread (0024).
pub fn run(cfg: Config) -> anyhow::Result<()> {
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
    let agent_thread = thread::spawn(move || agent.run(cmd_rx, event_tx));

    let result = tui::run::run(cfg.model.clone(), cfg.root.clone(), cmd_tx.clone(), event_rx, cancel);

    let _ = cmd_tx.send(AgentCommand::Shutdown);
    let _ = agent_thread.join();
    capability::shutdown_all();
    result
}
