// daemon 级 session 管理：每个 session = 一个 OS 线程跑 AgentLoop::run(cmd_rx, event_tx)。
// 管理器持有每个 session 的 cmd_tx 与 event_rx；turn 级把 AgentEvent 翻译成 ServerEvent
// 推入临时 mpsc，由 socket 层读出写回客户端。
use super::proto::ServerEvent;
use crate::agent::{AgentCommand, AgentEvent, AgentLoop};
use crate::provider::Provider;
use crate::registry::Registry;
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

    /// 发一条消息，返回该 turn 的 ServerEvent 流（以 TurnComplete 或 Error 收尾）。
    /// 同 session 的 turn 被 `event_rx` 的 Mutex 天然串行化。
    pub fn send_message(
        &mut self,
        id: &str,
        content: String,
    ) -> anyhow::Result<Receiver<ServerEvent>> {
        let cmd_tx = self.sessions.get(id).map(|s| s.cmd_tx.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown session: {id}"))?;
        let event_rx_mutex = Arc::clone(&self.sessions.get(id).expect("just checked").event_rx);
        let (out_tx, out_rx) = mpsc::channel::<ServerEvent>();
        cmd_tx.send(AgentCommand::ProcessMessage(content))
            .map_err(|_| anyhow::anyhow!("agent thread closed"))?;

        // Now spawn a thread that will acquire the lock when needed
        let out_tx_clone = out_tx;
        thread::spawn(move || {
            // Acquire the lock inside the spawned thread
            let rx = event_rx_mutex.lock().unwrap();
            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(120)) {
                    Ok(ev) => {
                        if let Some(se) = translate(ev) {
                            let terminal = matches!(se, ServerEvent::TurnComplete);
                            if out_tx_clone.send(se).is_err() { break; }
                            if terminal { break; }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let _ = out_tx_clone.send(ServerEvent::Error {
                            message: "turn timed out (agent unresponsive)".into()
                        });
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = out_tx_clone.send(ServerEvent::Error {
                            message: "agent disconnected".into()
                        });
                        break;
                    }
                }
            }
        });
        Ok(out_rx)
    }
}

/// AgentEvent → Option<ServerEvent>。丢弃进程内专属、不可回传客户端的事件
/// （PermissionRequest/AskUser 等带 oneshot 的事件由后续 task 处理；当前 daemon
/// 测试用 StubClient，不会产生它们）。
fn translate(ev: AgentEvent) -> Option<ServerEvent> {
    match ev {
        AgentEvent::StreamDelta(text) => Some(ServerEvent::StreamDelta { text }),
        AgentEvent::Notice(text) => Some(ServerEvent::Notice { text }),
        AgentEvent::Context { pct } => Some(ServerEvent::Context { pct }),
        AgentEvent::ToolStarted { name, preview } => Some(ServerEvent::ToolStarted { name, preview }),
        AgentEvent::ToolFinished { name, is_error, output } => {
            Some(ServerEvent::ToolFinished { name, is_error, output })
        }
        AgentEvent::TurnComplete => Some(ServerEvent::TurnComplete),
        _ => None,
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
                ServerEvent::StreamDelta { .. } => saw_delta = true,
                ServerEvent::TurnComplete => {
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
