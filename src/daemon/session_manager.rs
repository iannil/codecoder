// daemon 级 session 管理：每个 session = 一个 OS 线程跑 AgentLoop::run(cmd_rx, event_tx)。
// 管理器持有每个 session 的 cmd_tx 与 event_rx；turn 级返回原始 AgentEvent，
// 由 socket 层进行翻译及交互式提示处理（Task 9a）。
use crate::agent::{AgentCommand, AgentEvent, AgentLoop};
use crate::provider::Provider;
use crate::registry::Registry;
use crate::session::SessionManager;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// 一个被 daemon 托管的 session：发命令用 cmd_tx；agent 线程产出的事件汇总到
/// `event_rx`（由 forwarder 线程把 AgentEvent 搬到这里）。
struct DaemonSession {
    cmd_tx: Sender<AgentCommand>,
    /// 单一 drainer 串行化同 session 的 turn（Mutex 锁住接收端）。
    event_rx: Arc<std::sync::Mutex<Receiver<AgentEvent>>>,
    _agent: JoinHandle<()>,
    _forward: JoinHandle<()>,
}

pub struct DaemonSessionManager {
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    registry: Arc<std::sync::RwLock<Registry>>,
    sessions: HashMap<String, DaemonSession>,
    next_seq: u64,
    /// Turn 令牌：用户 turn 在 `drain_agent_events` 全程持有此 Mutex，
    /// workgraph tick 线程用 `try_lock` 探测——拿不到说明有 turn 在跑，跳过。
    /// 这与 `mgr` 的 Mutex 解耦（mgr 锁在 drain 之前释放以保持多客户端活性，
    /// Task 9a）；两者不可合用，否则又退化成单客户端。
    turn_token: Arc<std::sync::Mutex<()>>,
}

impl DaemonSessionManager {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
        registry: Arc<std::sync::RwLock<Registry>>,
    ) -> Self {
        Self {
            provider,
            model,
            max_tokens,
            temperature,
            root,
            registry,
            sessions: HashMap::new(),
            next_seq: 0,
            turn_token: Arc::new(std::sync::Mutex::new(())),
        }
    }

    /// 共享的 turn 令牌：`drain_agent_events` 全程持有，workgraph tick 线程探测。
    pub fn turn_token(&self) -> Arc<std::sync::Mutex<()>> {
        self.turn_token.clone()
    }

    /// 新建一个 session，返回其 id。agent 线程立刻进入 `run` 阻塞循环等待命令。
    pub fn create(&mut self) -> String {
        let id = format!("s{:04}", self.next_seq);
        self.next_seq += 1;
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
        let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();

        let agent = AgentLoop::new_daemon(
            self.provider.clone(),
            self.model.clone(),
            self.max_tokens,
            self.temperature,
            self.root.clone(),
            self.registry.clone(),
        );
        let agent = thread::spawn(move || agent.run(cmd_rx, event_tx));

        let forward = thread::spawn(move || {
            // event_rx 的所有权随 forward 线程；下面 send_message 用 Mutex 取用。
            // 这里不能持有——DaemonSession 持有 event_rx。故此线程只做 agent 的存活托管。
            drop(agent);
        });

        self.sessions.insert(
            id.clone(),
            DaemonSession {
                cmd_tx,
                event_rx: Arc::new(Mutex::new(event_rx)),
                _agent: forward,
                _forward: thread::spawn(|| ()),
            },
        );
        id
    }

    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sessions.keys().cloned().collect();
        v.sort();
        v
    }

    /// 通用：向某 session 发一条 AgentCommand，返回该轮原始 AgentEvent 流
    ///（由 socket 层翻译成 ServerEvent 并处理交互式提示）。
    fn dispatch(&mut self, id: &str, cmd: AgentCommand) -> anyhow::Result<Receiver<AgentEvent>> {
        let sess = self.sessions.get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {id}"))?;
        let cmd_tx = sess.cmd_tx.clone();
        let event_rx_mutex = Arc::clone(&sess.event_rx);
        let (out_tx, out_rx) = mpsc::channel::<AgentEvent>();
        cmd_tx.send(cmd).map_err(|_| anyhow::anyhow!("agent thread closed"))?;

        // drainer 线程：持有 event_rx Mutex 锁，转发原始 AgentEvent 到临时 mpsc
        //（recv_timeout(120s) 检测 agent 僵死；Timeout/Disconnected 时直接 drop out_tx，
        // 让接收端的 recv_timeout 观察 Disconnected）。
        let out_tx_clone = out_tx;
        thread::spawn(move || {
            let rx = event_rx_mutex.lock().unwrap();
            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(120)) {
                    Ok(ev) => {
                        let terminal = matches!(ev, AgentEvent::TurnComplete);
                        if out_tx_clone.send(ev).is_err() { break; }
                        if terminal { break; }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) |
                         Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // 直接 drop out_tx，让接收端观察到 Disconnected
                        drop(out_tx_clone);
                        break;
                    }
                }
            }
        });
        Ok(out_rx)
    }

    pub fn send_message(&mut self, id: &str, content: String) -> anyhow::Result<Receiver<AgentEvent>> {
        self.dispatch(id, AgentCommand::ProcessMessage(content))
    }

    /// 按 id/前缀解析磁盘 session；内存无此 session 则新建一个并对其发 Resume。
    pub fn resume(&mut self, id_or_prefix: &str) -> anyhow::Result<Receiver<AgentEvent>> {
        let sm = SessionManager::new(&self.root);
        let resolved = sm.find(id_or_prefix);
        let target = match resolved {
            Some(_id) => self.list().first().cloned().unwrap_or_else(|| self.create()),
            None => self.create(),
        };
        self.dispatch(&target, AgentCommand::Resume)
    }

    /// 磁盘上的全部 session id（daemon `ListSessions` 用此，而非内存 session 列表）。
    pub fn disk_sessions(&self) -> Vec<String> {
        SessionManager::new(&self.root).list().into_iter().map(|m| m.id).collect()
    }

    /// 导航活动 session 的 leaf 到 target（`cc fork <id>`）。复用 `AgentCommand::Navigate`：
    /// agent 改 leaf + Phase C 摘要废弃分支 + 自动落盘，发 Notice + TurnComplete。
    pub fn navigate(&mut self, session_id: &str, target: u64) -> anyhow::Result<Receiver<AgentEvent>> {
        self.dispatch(session_id, AgentCommand::Navigate(target))
    }

    /// 暴露 daemon root 路径（供 socket 层读 session 文件）。
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

// 上面用到了 std::thread；显式引入避免与 crate 内部歧义。
use std::thread;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::stub::StubClient;

    fn mgr_with_temp_root() -> (DaemonSessionManager, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cc_sessmgr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = DaemonSessionManager::new(
            Arc::new(StubClient),
            "gpt-4o".into(),
            4096,
            0.7,
            dir.clone(),
            registry,
        );
        (mgr, dir)
    }

    #[test]
    fn create_then_send_message_yields_turncomplete() {
        let (mut mgr, dir) = mgr_with_temp_root();
        let id = mgr.create();
        assert_eq!(mgr.list(), vec![id.clone()]);
        let rx = mgr.send_message(&id, "hello".into()).unwrap();
        let mut saw_delta = false;
        let mut saw_complete = false;
        for ev in rx.iter() {
            match ev {
                AgentEvent::StreamDelta(_) => saw_delta = true,
                AgentEvent::TurnComplete => {
                    saw_complete = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_complete, "turn must terminate with TurnComplete");
        // StubClient 产出的回复带文本 → 至少一个 StreamDelta。
        assert!(saw_delta, "stub reply should stream some text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_to_unknown_session_errors() {
        let (mut mgr, dir) = mgr_with_temp_root();
        let err = mgr.send_message("nope", "x".into()).unwrap_err();
        assert!(format!("{err}").contains("unknown session"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C1: turn_token 必须 lock 与 try_lock 行为正确，且跨线程互斥。
    #[test]
    fn turn_token_locks_and_try_locks_correctly() {
        let (mgr, dir) = mgr_with_temp_root();
        let token = mgr.turn_token();
        let token2 = mgr.turn_token();
        assert!(Arc::ptr_eq(&token, &token2), "turn_token() returns clones of same Arc");

        // 单线程：可直接 lock、try_lock 成功；持有时 try_lock 失败。
        {
            let _g = token.lock().unwrap();
            assert!(token.try_lock().is_err(), "held token must block try_lock");
        }
        assert!(token.try_lock().is_ok(), "released token must be try_lockable");

        // 跨线程互斥：子线程持有 token 期间，主线程 try_lock 必须返回 Err。
        let token_c = Arc::clone(&token);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let barrier_c = Arc::clone(&barrier);
        let h = std::thread::spawn(move || {
            let _g = token_c.lock().unwrap();
            barrier_c.wait(); // 子线程已持有 token
            std::thread::sleep(std::time::Duration::from_millis(80));
            // _g 释放后退出
        });
        barrier.wait(); // 子线程已持有 token
        match token.try_lock() {
            Err(std::sync::TryLockError::WouldBlock) => (), // 预期：互斥成立
            other => panic!("expected WouldBlock while other thread holds token, got {other:?}"),
        }
        h.join().unwrap();
        // 子线程释放后，主线程可重新取得
        assert!(token.try_lock().is_ok(), "token must be re-lockable after holder exits");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
