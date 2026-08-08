// src/lib.rs — public library surface for the behavioral test harness.
// main.rs is a thin shim over run(). Integration tests compile against THIS
// public API only — the black-box boundary is compiler-enforced.
#![allow(dead_code)]

pub mod agent;
pub mod alert;
pub mod background;
pub mod bg_ledger;
pub mod bg_observer;
pub mod capability;
pub mod compaction;
pub mod config;
pub mod help;
pub mod memory;
pub mod message;
pub mod permission;
pub mod provider;
pub mod provider_health;
pub mod registry;
pub mod retry;
pub mod review;
pub mod sandbox;
pub mod session;
pub mod supervisor_state;
pub mod tokenizer;
pub mod trust;
pub mod workgraph;
pub mod tool;
pub mod trace;
pub mod verify;
pub mod daemon;
pub mod client;
pub mod visual;
pub mod recovery;

use std::sync::Arc;

pub use agent::{AgentCommand, AgentEvent, AgentLoop, PermissionReply, SteerQueue, TrustReply};
pub use background::BgOutcome;
pub use config::Config;
pub use message::{Message, MessageItem, Role};
pub use permission::PermScope;
pub use provider::{Completion, CompletionRequest, FallbackProvider, Provider, StopReason};

/// Provider selection (ADR 0017). An env hook allows a scripted provider to be
/// injected for the pty smoke layer (L2); real runs use OpenAI or the stub.
pub fn select_provider(cfg: &Config) -> Arc<dyn Provider> {
    if let Ok(path) = std::env::var("CODECODER_SCRIPT") {
        return Arc::new(provider::stub::ScriptFileProvider::from_path(&path));
    }
    let primary = match cfg.api_key.as_deref() {
        Some(_) => Arc::new(provider::openai::OpenAiClient::new(cfg)) as Arc<dyn Provider>,
        None => Arc::new(provider::stub::StubClient) as Arc<dyn Provider>,
    };
    // When a fallback API base is configured, wrap the primary and a second
    // OpenAiClient (pointed at the fallback endpoint) in a FallbackProvider.
    if let Some(ref fallback_base) = cfg.fallback_api_base {
        let fallback_model = cfg.fallback_model.clone().unwrap_or_else(|| cfg.model.clone());
        // Build a config clone with the fallback endpoint so OpenAiClient uses it.
        let mut fb_cfg = cfg.clone();
        fb_cfg.api_base = fallback_base.clone();
        fb_cfg.model = fallback_model;
        let fallback = Arc::new(provider::openai::OpenAiClient::new(&fb_cfg));
        return Arc::new(provider::FallbackProvider::new(primary, fallback));
    }
    primary
}

/// BG 模式的 env 路由结果(ADR 0033)。空 task→workgraph 分支由 background.rs 处理。
pub enum BgMode {
    Explicit(String),
    Workgraph,
}

/// 从 env 解析 BG 模式。优先级:显式非空 task > WORKGRAPH 哨兵 > None(走 daemon)。
/// `CODECODER_BG_WORKGRAPH=1`(无显式 task)→ workgraph 逐里程碑模式(空 task);
/// `CODECODER_BG_TASK=<非空>` → 显式单 shot(同既有);否则 None。
pub fn bg_mode_from_env() -> Option<BgMode> {
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return Some(BgMode::Explicit(task));
        }
    }
    if std::env::var("CODECODER_BG_WORKGRAPH")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Some(BgMode::Workgraph);
    }
    None
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

    // 告警:当配置了 webhook 时,根据 alert_on_failure_only 决定是否发送。
    if let Some(ref webhook) = cfg.alert_webhook {
        let should_alert = if cfg.alert_on_failure_only { code != 0 } else { true };
        if should_alert {
            let mission_state_str = format!("{:?}", outcome.mission_state);
            let summary = if outcome.final_text.trim().is_empty() {
                format!("{} tools, {} denied", outcome.tool_calls.len(), outcome.denied.len())
            } else {
                let mut s = outcome.final_text.trim().to_string();
                if s.len() > 200 {
                    s.truncate(200);
                    s.push_str("...");
                }
                s
            };
            let msg = crate::alert::format_bg_alert(code, &mission_state_str, &summary);
            if let Err(e) = crate::alert::send_alert(webhook, &msg) {
                eprintln!("alert send failed: {e}");
            }
        }
    }

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
    use crate::bg_ledger::MissionState;

    #[test]
    fn run_background_ledger_append_and_exit_code() {
        // 验证 run_background 末尾将调用的 append + mission_exit_code 链路可用
        // (进程退出码本身不在进程内断言;断言函数返回值 + 账本读写一致)。
        let dir = std::env::temp_dir().join(format!("cc_lib_ledger_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut o = BgOutcome::default();
        o.mission_state = MissionState::Error("x".into());
        crate::bg_ledger::append(&dir, &o, "workgraph").unwrap();
        let recs = crate::bg_ledger::read_recent(&dir, 5, false);
        assert_eq!(recs.len(), 1);
        assert_eq!(crate::bg_ledger::mission_exit_code(&recs[0].mission_state), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bg_mode_from_env_routes_correctly() {
        use crate::{BgMode, bg_mode_from_env};
        use std::sync::Mutex;
        // env::set_var is process-global; serialize against other env-touching tests.
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("CODECODER_BG_TASK");
            std::env::remove_var("CODECODER_BG_WORKGRAPH");
        }
        assert!(bg_mode_from_env().is_none(), "nothing set → None (daemon)");

        unsafe { std::env::set_var("CODECODER_BG_WORKGRAPH", "1"); }
        assert!(
            matches!(bg_mode_from_env(), Some(BgMode::Workgraph)),
            "WORKGRAPH=1 → Workgraph"
        );

        unsafe { std::env::set_var("CODECODER_BG_TASK", "do X"); }
        assert!(
            matches!(bg_mode_from_env(), Some(BgMode::Explicit(t)) if t == "do X"),
            "explicit task wins over WORKGRAPH"
        );

        unsafe {
            std::env::remove_var("CODECODER_BG_TASK");
            std::env::set_var("CODECODER_BG_WORKGRAPH", "0");
        }
        assert!(bg_mode_from_env().is_none(), "WORKGRAPH=0 (not '1') → None");

        unsafe { std::env::set_var("CODECODER_BG_TASK", "   "); }
        assert!(bg_mode_from_env().is_none(), "whitespace-only task → None");

        unsafe {
            std::env::remove_var("CODECODER_BG_TASK");
            std::env::remove_var("CODECODER_BG_WORKGRAPH");
        }
    }
}
