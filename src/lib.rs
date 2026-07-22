// src/lib.rs — public library surface for the behavioral test harness.
// main.rs is a thin shim over run(). Integration tests compile against THIS
// public API only — the black-box boundary is compiler-enforced.
#![allow(dead_code)]

pub mod agent;
pub mod background;
pub mod bg_gate;
pub mod bg_ledger;
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
pub mod verify;
pub mod daemon;
pub mod client;

use std::sync::Arc;

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
    let task_label = if task.trim().is_empty() { "workgraph".to_string() } else { task.clone() };
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
    // 账本(ADR 0033):追加一条 JSONL;失败仅警告,不拖垮主流程。
    if let Err(e) = crate::bg_ledger::append(&cfg.root, &outcome, &task_label) {
        eprintln!("bg ledger append failed: {e}");
    }
    let code = crate::bg_ledger::mission_exit_code(&outcome.mission_state);
    if code == 0 {
        Ok(())
    } else {
        // 非零退出码:外部调度器(systemd OnFailure / cron)据此告警。
        std::process::exit(code);
    }
}

/// Daemon 入口（client-server 架构）：起长驻 daemon，无 TUI。socket/session 逻辑
/// 在 `daemon::Daemon::run` 中（Task 2 起填充）。
pub fn run_daemon(cfg: Config) -> anyhow::Result<()> {
    let daemon = daemon::Daemon::new(cfg);
    daemon.run()
}

#[cfg(test)]
mod tests {
    use crate::background::BgOutcome;
    use crate::bg_gate::MissionState;

    #[test]
    fn run_background_ledger_append_and_exit_code() {
        // 验证 run_background 末尾将调用的 append + mission_exit_code 链路可用
        // (进程退出码本身不在进程内断言;断言函数返回值 + 账本读写一致)。
        let dir = std::env::temp_dir().join(format!("cc_lib_ledger_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut o = BgOutcome::default();
        o.mission_state = MissionState::BlockedAt(9);
        crate::bg_ledger::append(&dir, &o, "workgraph").unwrap();
        let recs = crate::bg_ledger::read_recent(&dir, 5, false);
        assert_eq!(recs.len(), 1);
        assert_eq!(crate::bg_ledger::mission_exit_code(&recs[0].mission_state), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
