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
pub struct DaemonSession {
    pub id: String,
    pub cmd_tx: Sender<AgentCommand>,
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
    registry: Arc<Registry>,
    sessions: HashMap<String, DaemonSession>,
    next_seq: u64,
}

impl DaemonSessionManager {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
        registry: Arc<Registry>,
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
        }
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
                id: id.clone(),
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

    pub fn get(&self, id: &str) -> Option<&DaemonSession> {
        self.sessions.get(id)
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
        let registry = Arc::new(Registry::scan(&dir));
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
}
