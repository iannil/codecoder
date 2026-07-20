// Daemon (ADR 待补): 长驻后台进程，管理 N 个 AgentLoop，对外暴露 Unix socket。
// 本文件随 Task 2 起逐步填充真实逻辑；当前仅提供可空跑的骨架。
use crate::config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub mod proto;
pub mod session_manager;
pub mod socket;
pub mod bus;

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
        let registry = Arc::new(std::sync::RwLock::new(crate::registry::Registry::scan(&self.cfg.root)));
        let registry_for_reload = Arc::clone(&registry);
        let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
            provider,
            self.cfg.model.clone(),
            self.cfg.max_tokens,
            self.cfg.temperature,
            self.cfg.root.clone(),
            registry,
        )));
        // Turn 令牌在 DaemonSessionManager 内创建、所有线程共享同一 Arc clone。
        // mgr 的 Mutex 在 drain 之前释放（Task 9a：保持多客户端活性），无法用它探测
        // 「有 turn 在跑」；turn_token 在 drain 全程持有，正是这个信号。
        let turn_token = mgr.lock().unwrap().turn_token();
        let shutdown = Arc::new(AtomicBool::new(false));
        let bus = Arc::new(crate::daemon::bus::EventBus::new());

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
        // 用户 active turn 优先——通过 try_lock(turn_token) 探测：
        //   - 用户 turn 在 `drain_agent_events` 全程持有 turn_token（见 socket.rs::handle_connection）；
        //   - mgr 的 Mutex 不能用于此探测——它在 drain 之前就释放了（Task 9a：多客户端活性）。
        // 持锁期间推进，确保「turn」与「tick」在 workgraph 上互斥，避免 lost-update。
        let shutdown_c2 = shutdown.clone();
        let cfg_for_wg = self.cfg.clone();
        let turn_token_for_wg = Arc::clone(&turn_token);
        let wg_handle = std::thread::spawn(move || {
            while !shutdown_c2.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(30));
                // 拿到 token 才推进，且跨整段 advance 持有；拿不到（有 turn 在跑）跳过。
                let _guard = match turn_token_for_wg.try_lock() {
                    Ok(g) => g,
                    Err(_) => continue,
                };
                // advance 内部自建 agent，不复用 mgr；故此处不锁 mgr
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

        // Registry 热重载线程：每 3s 无条件重新扫描 skills/capabilities/prompts。
        // 不再用目录 mtime 作为闸门——POSIX 下目录 mtime 不会因 in-place 编辑而变化，
        // 会导致内容修改被漏掉。tick_reload 在锁外做磁盘 I/O，锁内只做廉价 swap。
        let root_for_reload = self.cfg.root.clone();
        let shutdown_for_reload = Arc::clone(&shutdown);
        let reload_handle = std::thread::spawn(move || {
            while !shutdown_for_reload.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(3));
                crate::registry::tick_reload(&registry_for_reload, &root_for_reload);
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
            let turn_token_c = Arc::clone(&turn_token);
            let bus_c = Arc::clone(&bus);
            std::thread::spawn(move || {
                if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown, &turn_token_c, &bus_c) {
                    eprintln!("ccd: connection error: {e}");
                }
            });
        }
        let _ = sup_handle.join();
        let _ = wg_handle.join();
        let _ = reload_handle.join();
        crate::capability::shutdown_all();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{tick_reload, Registry};
    use std::sync::{Arc, RwLock};
    use std::sync::atomic::{AtomicBool, Ordering};

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

    #[test]
    fn reload_loop_picks_up_written_skill() {
        let dir = std::env::temp_dir().join(format!("cc_reload_thread_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let reg = Arc::new(RwLock::new(Registry::scan(&dir)));
        let reg_for_thread = Arc::clone(&reg);
        let root_for_thread = dir.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_thread = shutdown.clone();
        let handle = std::thread::spawn(move || {
            while !shutdown_for_thread.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50)); // fast tick for the test
                tick_reload(&reg_for_thread, &root_for_thread);
            }
        });
        // Give the thread time to start
        std::thread::sleep(std::time::Duration::from_millis(20));
        // write a new skill AFTER the loop started
        std::fs::write(
            dir.join("skills/x.md"),
            "---\nname: x\ndescription: d\n---\nbody",
        ).unwrap();
        // allow at least one tick after the write
        std::thread::sleep(std::time::Duration::from_millis(200));
        shutdown.store(true, Ordering::SeqCst);
        handle.join().unwrap();
        let has_x = reg.read().unwrap().catalog.iter().any(|e| e.name == "x");
        assert!(has_x, "reload loop must pick up the written skill");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
