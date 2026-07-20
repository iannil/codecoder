// Daemon (ADR 待补): 长驻后台进程，管理 N 个 AgentLoop，对外暴露 Unix socket。
// 本文件随 Task 2 起逐步填充真实逻辑；当前仅提供可空跑的骨架。
use crate::config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub mod proto;
pub mod session_manager;
pub mod socket;

pub struct Daemon {
    cfg: Config,
}

impl Daemon {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    pub fn run(&self) -> anyhow::Result<()> {
        let sock_path = socket::default_sock_path(&self.cfg);
        let server = socket::SocketServer::bind(&sock_path)?;
        let provider = crate::select_provider(&self.cfg);
        let registry = Arc::new(crate::registry::Registry::scan(&self.cfg.root));
        let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
            provider,
            self.cfg.model.clone(),
            self.cfg.max_tokens,
            self.cfg.temperature,
            self.cfg.root.clone(),
            registry,
        )));
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut supervisor = crate::capability::Supervisor::start_all(&self.cfg.root)
            .unwrap_or_else(|e| {
                eprintln!("ccd: supervisor init failed: {e}");
                crate::capability::Supervisor { max_restarts: 3, window_secs: 60, root: self.cfg.root.clone(), states: Default::default() }
            });

        // 监督线程：周期 supervise（独立线程，避免阻塞 accept）。
        let shutdown_c = shutdown.clone();
        let sup_handle = {
            std::thread::spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    supervisor.supervise();
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                supervisor.shutdown_all();
            })
        };

        // workgraph 自动推进线程（first-class citizen #2 的 daemon 级形态）：空闲时推进。
        // 用户 active turn 优先——通过 try_lock(mgr) 探测：拿不到锁说明有 turn 在跑，skip。
        let shutdown_c2 = shutdown.clone();
        let cfg_for_wg = self.cfg.clone();
        let mgr_for_wg = mgr.clone();
        let wg_handle = std::thread::spawn(move || {
            while !shutdown_c2.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(30));
                // 仅当无 active turn（mgr 锁可立即取得）时推进
                if mgr_for_wg.try_lock().is_err() { continue; }
                // 释放锁后再跑（advance 内部自建 agent，不复用 mgr）
                let provider = crate::select_provider(&cfg_for_wg);
                let _ = crate::background::advance_one_milestone(
                    provider,
                    cfg_for_wg.model.clone(),
                    cfg_for_wg.max_tokens,
                    cfg_for_wg.temperature,
                    cfg_for_wg.root.clone(),
                );
            }
        });

        // 优雅退出：SIGINT/daemon 被 shutdown 请求后，退出时杀常驻 Capability（ADR 0021）。
        while !shutdown.load(Ordering::SeqCst) {
            let stream = match server.accept_one() {
                Ok(s) => s,
                Err(e) => {
                    // accept 出错不致命，记录后继续（真实 daemon 会 log；此处 best-effort）。
                    eprintln!("ccd: accept error: {e}");
                    continue;
                }
            };
            let mgr = mgr.clone();
            let shutdown = shutdown.clone();
            std::thread::spawn(move || {
                if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown) {
                    eprintln!("ccd: connection error: {e}");
                }
            });
        }
        let _ = sup_handle.join();
        let _ = wg_handle.join();
        crate::capability::shutdown_all();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Task 1 的 stub 测试仍保留语义：daemon 可构造。
    #[test]
    fn daemon_constructs_with_temp_root() {
        let dir = std::env::temp_dir().join(format!("cc_daemon_ctor_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            api_key: None, model: "gpt-4o".into(), api_base: "https://api.openai.com/v1".into(),
            max_tokens: 4096, temperature: 0.7, root: dir.clone(), github_token: None,
        };
        let _d = Daemon::new(cfg); // 仅构造，不 run（run 会阻塞 accept）
        let _ = std::fs::remove_dir_all(&dir);
    }
}
