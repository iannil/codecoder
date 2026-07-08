// CodeCoder — autonomous AI agent, event-driven, filesystem-as-self.
// See CONTEXT.md for terminology and docs/adr/* for the decisions behind this shape.
#![allow(dead_code)]

mod agent;
mod capability;
mod compaction;
mod config;
mod memory;
mod message;
mod permission;
mod provider;
mod registry;
mod session;
mod tokenizer;
mod tool;
mod tui;

use agent::{AgentCommand, AgentEvent, AgentLoop};
use provider::{Provider, openai::OpenAiClient, stub::StubClient};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

fn main() -> anyhow::Result<()> {
    let cfg = config::Config::from_env();

    // Provider selection: real LLM when a key is set, else the StubClient (ADR 0017).
    let provider: Arc<dyn Provider> = match cfg.api_key.as_deref() {
        Some(_) => Arc::new(OpenAiClient::new(&cfg)),
        None => Arc::new(StubClient),
    };

    // Kernel: OS threads + channels (ADR 0016). cmd_tx: TUI→agent; event_rx: agent→TUI.
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();

    let agent = AgentLoop::new(
        provider,
        cfg.model.clone(),
        cfg.max_tokens,
        cfg.temperature,
        cfg.root.clone(),
    );
    let agent_thread = thread::spawn(move || agent.run(cmd_rx, event_tx));

    // TUI owns the main thread (ADR 0024).
    let result = tui::run::run(cfg.model.clone(), cfg.root.clone(), cmd_tx.clone(), event_rx);

    let _ = cmd_tx.send(AgentCommand::Shutdown);
    let _ = agent_thread.join();
    // Kill any persistent capability services (ADR 0021: bound to process lifetime).
    capability::shutdown_all();
    result
}
