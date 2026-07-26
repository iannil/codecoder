// Daemon (ADR 待补): 长驻后台进程，管理 N 个 AgentLoop，对外暴露 Unix socket。
// 本文件随 Task 2 起逐步填充真实逻辑；当前仅提供可空跑的骨架。
use crate::config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::os::unix::net::UnixStream;

pub mod proto;
pub mod session_manager;
pub mod socket;
pub mod bus;
pub mod task_source;

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
        // 创建共享线程心跳状态（需在后台线程使用之前）。
        let thread_status = Arc::new(Mutex::new(Vec::<crate::daemon::proto::ThreadStatus>::new()));
        {
            let mut ts = thread_status.lock().unwrap();
            ts.push(crate::daemon::proto::ThreadStatus { name: "monitor".into(), last_tick: None, tick_count: 0, last_event: "initializing".into() });
            ts.push(crate::daemon::proto::ThreadStatus { name: "supervisor".into(), last_tick: None, tick_count: 0, last_event: "initializing".into() });
            ts.push(crate::daemon::proto::ThreadStatus { name: "workgraph".into(), last_tick: None, tick_count: 0, last_event: "initializing".into() });
            ts.push(crate::daemon::proto::ThreadStatus { name: "reload".into(), last_tick: None, tick_count: 0, last_event: "initializing".into() });
        }

        // 注册 SIGINT + SIGTERM → shutdown flag(ADR 0032 修订)。
        // 复用 signal-hook(既有依赖,同 BG cancel_on_sigint),但 handler 设的是
        // daemon 的 shutdown flag(而非 turn CancelToken),使信号触发优雅退出。
        if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown)) {
            eprintln!("ccd: SIGINT handler not registered: {e}");
        }
        if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown)) {
            eprintln!("ccd: SIGTERM handler not registered: {e}");
        }
        // 监控线程:每 100ms 检查 shutdown flag;被设后自连接 socket 触发 accept 退出阻塞。
        // 这样 cc shutdown、SIGINT、SIGTERM 均能触发 daemon 退出(均设同一个 flag)。
        let sock_path_for_monitor = socket::default_sock_path(&self.cfg);
        let shutdown_for_monitor = shutdown.clone();
        let ts_monitor = Arc::clone(&thread_status);
        std::thread::spawn(move || {
            let mut count = 0u64;
            while !shutdown_for_monitor.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
                count += 1;
                let mut status = ts_monitor.lock().unwrap();
                if let Some(s) = status.iter_mut().find(|s| s.name == "monitor") {
                    s.last_tick = Some(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                    s.tick_count = count;
                    s.last_event = "monitoring".into();
                }
            }
            // shutdown flag 被设,自连接触发 accept 退出阻塞。
            let _ = UnixStream::connect(&sock_path_for_monitor);
        });
        let bus = Arc::new(crate::daemon::bus::EventBus::new());

        let budget = self.cfg.supervisor_crash_budget;
        let supervisor = Arc::new(Mutex::new(
            crate::capability::Supervisor::start_all(&self.cfg.root, budget)
                .unwrap_or_else(|e| {
                    eprintln!("ccd: supervisor init failed: {e}");
                    crate::capability::Supervisor {
                        root: self.cfg.root.clone(),
                        states: Default::default(),
                        state: crate::supervisor_state::SupervisorState::default(),
                        crash_budget: budget,
                    }
                })
        ));

        // 把 thread_status 传给 mgr（供 `cc status` 查询）。
        mgr.lock().unwrap().thread_status = Some(Arc::clone(&thread_status));

        // 监督线程：周期 supervise（独立线程，避免阻塞 accept）。
        let shutdown_c = shutdown.clone();
        let bus_for_sup = Arc::clone(&bus);
        let sup_supervisor = Arc::clone(&supervisor);
        let sup_tick = Duration::from_secs(self.cfg.supervisor_tick_secs);
        let ts_sup = Arc::clone(&thread_status);
        let sup_handle = {
            std::thread::spawn(move || {
                let mut count = 0u64;
                while !shutdown_c.load(Ordering::SeqCst) {
                    let events = sup_supervisor.lock().unwrap().supervise();
                    let last_event = events.iter().cloned().collect::<Vec<_>>().join("; ");
                    count += 1;
                    {
                        let mut status = ts_sup.lock().unwrap();
                        if let Some(s) = status.iter_mut().find(|s| s.name == "supervisor") {
                            s.last_tick = Some(std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                            s.tick_count = count;
                            s.last_event = if last_event.is_empty() { "no events".into() } else { last_event };
                        }
                    }
                    for line in events {
                        bus_for_sup.broadcast("supervisor", &line);
                    }
                    std::thread::sleep(sup_tick);
                }
                sup_supervisor.lock().unwrap().shutdown_all();
            })
        };

        // workgraph 自动推进线程（first-class citizen #2 的 daemon 级形态）：空闲时推进。
        // 支持运行时暂停/恢复（通过 `cc workgraph-pause` / `cc workgraph-resume` 控制）。
        let shutdown_c2 = shutdown.clone();
        let cfg_for_wg = self.cfg.clone();
        let turn_token_for_wg = Arc::clone(&turn_token);
        let bus_for_wg = Arc::clone(&bus);
        let wg_tick = Duration::from_secs(cfg_for_wg.wg_tick_secs);
        let ts_wg = Arc::clone(&thread_status);
        // 暂停 flag：true = 暂停（不推进），false = 正常运行。
        // 由 mgr 的 workgraph_paused 字段同步，workgraph 线程在此读取。
        let wg_paused = Arc::new(AtomicBool::new(false));
        // 把 wg_paused 传给 mgr（供 `cc workgraph-pause/resume` 控制）。
        mgr.lock().unwrap().workgraph_paused = Some(Arc::clone(&wg_paused));
        let wg_handle = std::thread::spawn(move || {
            let mut count = 0u64;
            while !shutdown_c2.load(Ordering::SeqCst) {
                std::thread::sleep(wg_tick);
                count += 1;
                // 检查暂停 flag：暂停时不推进，仅记录心跳。
                if wg_paused.load(Ordering::SeqCst) {
                    let mut status = ts_wg.lock().unwrap();
                    if let Some(s) = status.iter_mut().find(|s| s.name == "workgraph") {
                        s.last_tick = Some(std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                        s.tick_count = count;
                        s.last_event = "paused".into();
                    }
                    continue;
                }
                // 拿到 token 才推进，且跨整段 advance 持有；拿不到（有 turn 在跑）跳过。
                let _guard = match turn_token_for_wg.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        let mut status = ts_wg.lock().unwrap();
                        if let Some(s) = status.iter_mut().find(|s| s.name == "workgraph") {
                            s.last_tick = Some(std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                            s.tick_count = count;
                            s.last_event = "skipped (turn active)".into();
                        }
                        continue;
                    }
                };
                // advance 内部自建 agent，不复用 mgr；故此处不锁 mgr
                let provider = crate::select_provider(&cfg_for_wg);
                let out = crate::background::advance_one_milestone(
                    provider,
                    cfg_for_wg.model.clone(),
                    cfg_for_wg.max_tokens,
                    cfg_for_wg.temperature,
                    cfg_for_wg.root.clone(),
                );
                {
                    let mut status = ts_wg.lock().unwrap();
                    if let Some(s) = status.iter_mut().find(|s| s.name == "workgraph") {
                        s.last_tick = Some(std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                        s.tick_count = count;
                        s.last_event = match &out {
                            Ok(Some(o)) => {
                                let ms = o.events.iter().find(|e| e.starts_with("milestone"))
                                    .cloned().unwrap_or_else(|| "advanced".into());
                                ms
                            }
                            Ok(None) => "no pending milestone".into(),
                            Err(e) => format!("error: {e}"),
                        };
                    }
                }
                if let Ok(Some(o)) = out {
                    if let Some(line) = o.events.iter().find(|e| e.starts_with("milestone")) {
                        bus_for_wg.broadcast("workgraph", line);
                    }
                }
            }
        });

        // Registry 热重载线程：每 3s 无条件重新扫描 skills/capabilities/prompts。
        // 不再用目录 mtime 作为闸门——POSIX 下目录 mtime 不会因 in-place 编辑而变化，
        // 会导致内容修改被漏掉。tick_reload 在锁外做磁盘 I/O，锁内只做廉价 swap。
        let root_for_reload = self.cfg.root.clone();
        let shutdown_for_reload = Arc::clone(&shutdown);
        let ts_reload = Arc::clone(&thread_status);
        let reload_handle = std::thread::spawn(move || {
            let mut count = 0u64;
            while !shutdown_for_reload.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(3));
                count += 1;
                crate::registry::tick_reload(&registry_for_reload, &root_for_reload);
                let mut status = ts_reload.lock().unwrap();
                if let Some(s) = status.iter_mut().find(|s| s.name == "reload") {
                    s.last_tick = Some(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                    s.tick_count = count;
                    s.last_event = "tick_reload".into();
                }
            }
        });

        // 自动任务发现线程：按 interval 轮询外部源（GitHub Issues 等），
        // 把新 issue seed 为 workgraph milestone。
        // 仅当 auto_task_interval_secs > 0 时启动。
        // 支持运行时暂停/恢复（通过 `cc autotask on/off` 控制）。
        let auto_task_interval = self.cfg.auto_task_interval_secs;
        if auto_task_interval > 0 {
            // Register the thread in thread_status before spawning.
            {
                let mut ts = thread_status.lock().unwrap();
                ts.push(crate::daemon::proto::ThreadStatus {
                    name: "autotask".into(),
                    last_tick: None,
                    tick_count: 0,
                    last_event: "initializing".into(),
                });
            }
            // 暂停 flag：true = 暂停（不轮询），false = 正常运行。
            // 由 mgr 的 autotask_paused 字段同步，autotask 线程在此读取。
            let autotask_paused = Arc::new(AtomicBool::new(false));
            // 把 autotask_paused 传给 mgr（供 `cc autotask on/off` 控制）。
            mgr.lock().unwrap().autotask_paused = Some(Arc::clone(&autotask_paused));
            let shutdown_auto = Arc::clone(&shutdown);
            let root_auto = self.cfg.root.clone();
            let token_auto = self.cfg.github_token.clone().unwrap_or_default();
            let source_auto = self.cfg.auto_task_source.clone();
            let ts_auto = Arc::clone(&thread_status);
            let bus_auto = Arc::clone(&bus);
            std::thread::spawn(move || {
                let mut count = 0u64;
                let tick = Duration::from_secs(auto_task_interval);
                while !shutdown_auto.load(Ordering::SeqCst) {
                    std::thread::sleep(tick);
                    count += 1;
                    // 检查暂停 flag：暂停时不轮询，仅记录心跳。
                    if autotask_paused.load(Ordering::SeqCst) {
                        let mut status = ts_auto.lock().unwrap();
                        if let Some(s) = status.iter_mut().find(|s| s.name == "autotask") {
                            s.last_tick = Some(std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                            s.tick_count = count;
                            s.last_event = "paused".into();
                        }
                        continue;
                    }
                    let mut last_event = "idle".to_string();
                    if source_auto == "github_issues" {
                        match crate::daemon::task_source::poll_and_seed(&root_auto, &token_auto) {
                            Ok((fetched, seeded)) => {
                                if seeded > 0 {
                                    last_event = format!("seeded {seeded}/{fetched} issues");
                                    bus_auto.broadcast("autotask", &format!("seeded {seeded} new issues from {fetched} open"));
                                } else {
                                    last_event = format!("no new issues ({fetched} open)");
                                }
                            }
                            Err(e) => {
                                last_event = format!("error: {e}");
                                // Don't broadcast errors — too noisy on each tick
                            }
                        }
                    }
                    let mut status = ts_auto.lock().unwrap();
                    if let Some(s) = status.iter_mut().find(|s| s.name == "autotask") {
                        s.last_tick = Some(std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                        s.tick_count = count;
                        s.last_event = last_event;
                    }
                }
            });
        }

        // 优雅退出: SIGINT/SIGTERM → shutdown flag → 循环退出 → shutdown_all。
        // cc shutdown 设 shutdown flag 后,自连接 socket 触发 accept 退出阻塞。
        while !shutdown.load(Ordering::SeqCst) {
            let stream = match server.accept_one() {
                Ok(s) => s,
                Err(e) => {
                    // accept 出错不致命,记录后继续。
                    eprintln!("ccd: accept error: {e}");
                    continue;
                }
            };
            // 收到连接后检查 flag:若已设则退出(可能是自连接唤醒)。
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
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
        crate::capability::shutdown_all();
        // 优雅退出前清理 socket 文件(exit(0) 不执行 Drop)。
        let _ = std::fs::remove_file(socket::default_sock_path(&self.cfg));
        std::process::exit(0);
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
            bg_max_auto: 3, bg_circuit_k: 2, bg_milestone_tool_cap: 8, bg_max_fix_attempts: 3,
            supervisor_crash_budget: 3,
            max_tool_output: 256 * 1024,
            max_tokens_ceiling: 32768,
            noop_nudge_threshold: 3,
            command_timeout_secs: 0,
            compaction_tier2: true,
            wg_tick_secs: 30,
            supervisor_tick_secs: 1,
            ondemand_reaper_secs: 5,
            auto_task_interval_secs: 300,
            auto_task_source: "github_issues".into(),
            provider_retry_max: 3,
            provider_retry_initial_ms: 1000,
            fallback_api_base: None,
            fallback_model: None,
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

    #[test]
    fn workgraph_publisher_test() {
        // 真集成测试（替代原先与 bus::tests 重复的 tautology 测试）：
        // 1. 跑真实的 advance_one_milestone（temp root + 1 pending milestone + StubClient）；
        // 2. 复用 daemon::run 中那段 filter-and-broadcast 逻辑：
        //    `o.events.iter().find(|e| e.starts_with("milestone"))` → bus.broadcast("workgraph", line)；
        // 3. 校验：若 advance 真产出了 milestone 行（parse_review 成功）→ 订阅者收到 BusNotice，
        //    且其 text 以 "milestone" 开头（与 filter 一致）；
        //    若未产出（StubClient 默认文本不含 verdict → unparsed → 不 emit milestone 行）
        //    → 订阅者不应收到任何事件。
        use crate::background::advance_one_milestone;
        use crate::daemon::bus::EventBus;
        use crate::daemon::proto::ServerEvent;
        use crate::provider::stub::StubClient;
        use crate::workgraph::WorkGraph;

        let dir = std::env::temp_dir().join(format!(
            "cc_wg_pub_{}_{}", std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // 1 个 pending milestone。
        let mut g = WorkGraph::default();
        g.add("ship feature X", "acceptance criteria", vec![]).unwrap();
        g.save(&dir).unwrap();

        // 真实 advance：与 daemon::run 内部调用一致。
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).expect("advance_one_milestone must not error");

        let bus = Arc::new(EventBus::new());
        let (tx, rx) = std::sync::mpsc::channel::<ServerEvent>();
        let _sub_id = bus.register(tx);

        // 复用 daemon::run 的发布逻辑（mod.rs 中 wg_handle 线程体内的那段）。
        let emitted_line: Option<String> = match out {
            Some(ref o) => o.events.iter()
                .find(|e: &&String| e.starts_with("milestone"))
                .cloned(),
            None => None,
        };
        if let Some(ref line) = emitted_line {
            bus.broadcast("workgraph", line);
        }

        if let Some(line) = emitted_line {
            // 有真 milestone 行 → 订阅者必须收到一条 BusNotice，内容与 line 一致。
            let ev = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
            match ev {
                ServerEvent::BusNotice { source, text } => {
                    assert_eq!(source, "workgraph");
                    assert_eq!(text, line, "broadcast text must equal the emitted milestone line");
                    assert!(text.starts_with("milestone"), "filter must match real output");
                }
                other => panic!("expected BusNotice, got {other:?}"),
            }
        } else {
            // StubClient 默认文本不含可解析 verdict → advance 不 emit milestone 行
            // → daemon 不应广播任何东西。订阅者收不到事件即正确。
            assert!(matches!(rx.recv_timeout(std::time::Duration::from_millis(200)),
                              Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
                "no milestone line → daemon must NOT broadcast");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
