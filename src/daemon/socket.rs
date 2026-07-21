// Unix socket listener：bind、accept、按行读写 JSON 帧。socket 路径默认
// `$CODECODER_ROOT/.ccd.sock`。
use crate::config::Config;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 保证连接关闭时（EOF / Err / panic）都执行：从 bus 注销 → drop 所有本地
/// combined_tx clone → join writer。没有它，bus 持有的 combined_tx clone 会让
/// writer 线程的 `for ev in combined_rx.iter()` 永久阻塞（线程泄漏）—— 尤其是
/// `?` 提前返回 / `mgr.lock().unwrap()` 中毒 panic 这两条非 EOF 出口。
///
/// 必须拥有「loop body 用的 sender clone」也至关重要：drop 顺序上，guard 比
/// body_tx 先 drop（声明在前），若 guard 在 join 时 body_tx 还活着，combined_rx
/// 永不关闭、writer 永远 join 不上。所以 guard 把所有 sender clone 都接管了。
struct ConnGuard {
    bus: Arc<super::bus::EventBus>,
    sub: super::bus::SubscriptionId,
    /// 由 loop body 通过 `as_ref()` 借用做 inline send / drain；Drop 时 take() 释放。
    body_tx: Option<std::sync::mpsc::Sender<super::proto::ServerEvent>>,
    /// 原始 combined_tx（drop 后与 body_tx 一起释放，彻底断流）。
    combined_tx: Option<std::sync::mpsc::Sender<super::proto::ServerEvent>>,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        // 1. 从 bus 注销其 sender clone（bus 端的 combined_tx）。
        self.bus.unregister(self.sub);
        // 2. drop body_tx + combined_tx 本地 clone：至此 combined_rx 的所有 sender
        //    都释放 → writer 线程的 `iter()` 自然结束。必须在 join 之前完成。
        self.body_tx.take();
        self.combined_tx.take();
        // 3. join writer 线程，确保它退出（不会再有写入）。
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
    }
}

pub fn default_sock_path(cfg: &Config) -> PathBuf {
    cfg.root.join(".ccd.sock")
}

/// 薄封装：bind + accept_one（单次），便于在测试里按需驱动。
pub struct SocketServer {
    listener: UnixListener,
    sock_path: PathBuf,
}

impl SocketServer {
    pub fn bind(sock_path: &Path) -> anyhow::Result<Self> {
        // 残留 socket 文件先清掉（上次 daemon 没干净退出）。
        let _ = std::fs::remove_file(sock_path);
        let listener = UnixListener::bind(sock_path)?;
        // 限制 socket 文件权限为仅 owner 可读写（安全加固）。
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o600));
        Ok(Self { listener, sock_path: sock_path.to_path_buf() })
    }

    /// 阻塞接受一个连接。
    pub fn accept_one(&self) -> anyhow::Result<UnixStream> {
        let (stream, _) = self.listener.accept()?;
        Ok(stream)
    }

    pub fn sock_path(&self) -> &Path {
        &self.sock_path
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock_path);
    }
}

/// 处理单个连接：读一行 ClientRequest，在 mgr 上执行，把结果事件写回流。
/// 当前支持 `SendMessage`/`NewSession`/`ListSessions`/`Resume`/`Shutdown`；其余回 Error。
/// (Task 9a: 新增对 5 种 prompt AgentEvent 的内联处理。)
pub fn handle_connection(
    stream: UnixStream,
    mgr: &Mutex<super::session_manager::DaemonSessionManager>,
    shutdown: &std::sync::atomic::AtomicBool,
    turn_token: &std::sync::Arc<std::sync::Mutex<()>>,
    bus: &std::sync::Arc<super::bus::EventBus>,
) -> anyhow::Result<()> {
    use super::proto::{read_request, write_event, ClientRequest, ServerEvent};
    use std::io::BufWriter;
    use std::sync::mpsc;

    // 读半归 request-reader（本线程），写半归 writer 线程。
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);
    let mut writer = BufWriter::new(stream);

    // combined 通道：所有 ServerEvent（turn 事件 + bus 事件）汇流到此，
    // 由 writer 线程单一写出——写天然串行化。
    let (combined_tx, combined_rx) = mpsc::channel::<ServerEvent>();
    // 注册到 bus：广播事件直接落进 combined_rx，与 turn 事件同流。
    // 保留订阅 id：连接关闭（任意路径）时由 ConnGuard 用它 unregister，
    // 移除 bus 端的 sender clone → combined_rx 的所有 sender 释放 → writer 退出。
    let subscription = bus.register(combined_tx.clone());
    let writer_handle = std::thread::spawn(move || {
        for ev in combined_rx.iter() {
            if write_event(&mut writer, &ev).is_err() {
                break; // 客户端断开或写入失败
            }
        }
    });

    // ConnGuard 保证在函数返回（含 `?` 提前返回）或 panic-unwind 时执行：
    //   bus.unregister(sub) → drop body_tx + combined_tx 本地 clone → join writer。
    // 没有 guard，`?` 提前返回会跳过这几步 → bus 持有的 sender clone 让 writer
    // 线程的 iter() 永久阻塞 → 线程泄漏（quiet daemon 上不可收敛）。
    //
    // body_tx 必须也由 guard 拥有：drop 顺序上 guard 比 body_tx 先 drop（声明在前），
    // 若 guard 在 join 时 body_tx 还活着，combined_rx 永不关闭、writer 永远 join
    // 不上。所以 guard 把所有本地 sender clone 都接管了——loop body 通过
    // `guard.body_tx.as_ref().unwrap()` 借用做 inline send / drain。
    let guard = ConnGuard {
        bus: Arc::clone(bus),
        sub: subscription,
        body_tx: Some(combined_tx.clone()),
        combined_tx: Some(combined_tx),
        writer: Some(writer_handle),
    };
    // body_tx 借用：生命周期与 guard 一致，函数末尾 guard drop 后失效。
    // （as_ref 返回 Option<&Sender>；unwrap 永不 panic——guard 在 drop 前始终保有它。）
    let body_tx: &std::sync::mpsc::Sender<ServerEvent> = guard.body_tx.as_ref().unwrap();

    // 持久连接：循环读 ClientRequest，直到客户端关闭（EOF）。以下 `?` 与
    // `.unwrap()` 任意一条出错 / panic 都会触发 ConnGuard::drop，安全清理。
    while let Some(req) = read_request(&mut reader)? {
        match req {
            ClientRequest::SendMessage { content } => {
                let mut g = mgr.lock().unwrap();
                let id = match g.list().first().cloned() { Some(id) => id, None => g.create() };
                let rx = g.send_message(&id, content)?;
                drop(g); // 释放 mgr 锁，让其它客户端可 NewSession/ListSessions
                let _turn_guard = turn_token.lock().unwrap();
                drain_agent_events(rx, &mut reader, body_tx)?;
            }
            ClientRequest::Resume { id } => {
                let mut g = mgr.lock().unwrap();
                let rx = g.resume(&id)?;
                drop(g);
                let _turn_guard = turn_token.lock().unwrap();
                drain_agent_events(rx, &mut reader, body_tx)?;
            }
            // PromptReply 只应在一个 turn 的 drain 中被内联消费；顶层收到说明协议误用。
            ClientRequest::PromptReply { .. } => {
                let _ = body_tx.send(ServerEvent::Error {
                    message: "unexpected PromptReply (no prompt pending)".into(),
                });
            }
            ClientRequest::NewSession => {
                let id = mgr.lock().unwrap().create();
                let _ = body_tx.send(ServerEvent::SessionCreated { id });
            }
            ClientRequest::ListSessions => {
                let ids = mgr.lock().unwrap().disk_sessions();
                let _ = body_tx.send(ServerEvent::Sessions { ids });
            }
            ClientRequest::Shutdown => {
                shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = body_tx.send(ServerEvent::Notice { text: "shutting down".into() });
            }
            ClientRequest::Status => {
                let _ = body_tx.send(ServerEvent::Notice { text: "ccd running".into() });
            }
        }
    }

    // EOF happy path：guard 在函数返回时 drop，完成 unregister + writer join。
    Ok(())
}

/// drain_loop: 读取原始 AgentEvent，翻译成 ServerEvent 写回客户端，遇到 prompt 时
/// 内联阻塞读取 PromptReply（synchronous 模型，无需新线程）。
fn drain_agent_events(
    rx: std::sync::mpsc::Receiver<crate::agent::AgentEvent>,
    reader: &mut BufReader<UnixStream>,
    combined_tx: &std::sync::mpsc::Sender<super::proto::ServerEvent>,
) -> anyhow::Result<()> {
    use crate::agent::AgentEvent;
    use super::proto::{PromptBody, ServerEvent};

    let mut prompt_id = 0u64;
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(AgentEvent::StreamDelta(text)) => {
                let _ = combined_tx.send(ServerEvent::StreamDelta { text });
            }
            Ok(AgentEvent::Notice(text)) => {
                let _ = combined_tx.send(ServerEvent::Notice { text });
            }
            Ok(AgentEvent::Context { pct }) => {
                let _ = combined_tx.send(ServerEvent::Context { pct });
            }
            Ok(AgentEvent::ToolStarted { name, preview }) => {
                let _ = combined_tx.send(ServerEvent::ToolStarted { name, preview });
            }
            Ok(AgentEvent::ToolFinished { name, is_error, output }) => {
                let _ = combined_tx.send(ServerEvent::ToolFinished { name, is_error, output });
            }
            Ok(AgentEvent::TurnComplete) => {
                let _ = combined_tx.send(ServerEvent::TurnComplete);
                break;
            }
            // ===== Task 9a: 5 种 prompt 事件的内联处理 =====
            Ok(AgentEvent::PermissionRequest { key, preview, reply_tx }) => {
                prompt_id += 1;
                let _ = combined_tx.send(ServerEvent::Prompt {
                    id: prompt_id, body: PromptBody::Permission { key, preview },
                });
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.to_permission_reply());
            }
            Ok(AgentEvent::AskUser { prompt, reply_tx }) => {
                prompt_id += 1;
                let _ = combined_tx.send(ServerEvent::Prompt {
                    id: prompt_id, body: PromptBody::AskUser { prompt },
                });
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.into_text());
            }
            Ok(AgentEvent::Confirm { prompt, reply_tx }) => {
                prompt_id += 1;
                let _ = combined_tx.send(ServerEvent::Prompt {
                    id: prompt_id, body: PromptBody::Confirm { prompt },
                });
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.yes());
            }
            Ok(AgentEvent::PlanApproval { plan, reply_tx }) => {
                prompt_id += 1;
                let _ = combined_tx.send(ServerEvent::Prompt {
                    id: prompt_id, body: PromptBody::PlanApproval { plan },
                });
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.approved());
            }
            Ok(AgentEvent::TrustPrompt { root, reply_tx }) => {
                prompt_id += 1;
                let root_str = root.to_string_lossy().to_string();
                let _ = combined_tx.send(ServerEvent::Prompt {
                    id: prompt_id, body: PromptBody::Trust { root: root_str },
                });
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.to_trust_reply());
            }
            // Sub-agent 进度（agent/review 工具产出）——以 Notice 转发，保留可见性。
            Ok(AgentEvent::SubAgentMilestone(s)) => {
                let _ = combined_tx.send(ServerEvent::Notice { text: format!("↳ {s}") });
            }
            // Chain-of-thought——以 Notice 转发（无独立 wire 变体；ADR 0032 Negative 中记录）。
            Ok(AgentEvent::Reasoning(s)) => {
                let _ = combined_tx.send(ServerEvent::Notice { text: format!("💭 {s}") });
            }
            // 其他 AgentEvent 变体（Test*, L4*）仍丢弃：仅 L4 verify 场景产出，
            // 暂未在 wire 协议中暴露（见 ADR 0032 Negative consequences）。
            Ok(_) => { /* drop unserializable events */ }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = combined_tx.send(ServerEvent::Error {
                    message: "turn timed out (agent unresponsive)".into(),
                });
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = combined_tx.send(ServerEvent::Error {
                    message: "agent disconnected".into(),
                });
                break;
            }
        }
    }
    Ok(())
}

/// 读取一个 `ClientRequest::PromptReply`，校验 id 匹配。
fn read_prompt_reply(
    reader: &mut BufReader<UnixStream>,
    expected_id: u64,
) -> anyhow::Result<super::proto::PromptAnswer> {
    use super::proto::{ClientRequest, read_request};
    let line = read_request(reader)?.ok_or_else(|| anyhow::anyhow!("client closed during prompt"))?;
    match line {
        ClientRequest::PromptReply { id, answer } => {
            if id != expected_id {
                anyhow::bail!("prompt reply id mismatch: expected {expected_id}, got {id}");
            }
            Ok(answer)
        }
        other => anyhow::bail!("expected PromptReply during prompt, got: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::proto::{write_event, ClientRequest, ServerEvent};
    use crate::daemon::session_manager::DaemonSessionManager;
    use crate::provider::stub::StubClient;
    use crate::registry::Registry;
    use std::io::BufRead;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn client_sendmessage_roundtrips_through_socket() {
        let dir = std::env::temp_dir().join(format!("cc_sock_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        ));
        let turn_token = mgr.lock().unwrap().turn_token();
        let shutdown = Arc::new(AtomicBool::new(false));
        let bus = Arc::new(crate::daemon::bus::EventBus::new());

        // 服务端线程：accept 一次并处理。
        let shutdown_c = shutdown.clone();
        let bus_c = Arc::clone(&bus);
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            handle_connection(stream, &mgr, &shutdown_c, &turn_token, &bus_c).unwrap();
        });

        // 客户端：连、发 SendMessage、读到 TurnComplete。给服务端一点时间 bind。
        std::thread::sleep(Duration::from_millis(50));
        let mut conn = UnixStream::connect(&sock).unwrap();
        // 直接写一行 ClientRequest JSON（不要先写 ServerEvent 行——服务端首行即解析请求）
        use std::io::Write;
        let line = serde_json::to_string(&ClientRequest::SendMessage { content: "hi".into() }).unwrap();
        writeln!(conn, "{line}").unwrap();
        conn.flush().unwrap();

        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut events = Vec::new();
        loop {
            let mut buf = String::new();
            if reader.read_line(&mut buf).unwrap() == 0 { break; }
            let ev: ServerEvent = serde_json::from_str(buf.trim()).unwrap();
            let is_done = matches!(ev, ServerEvent::TurnComplete);
            events.push(ev);
            if is_done { break; }
        }
        assert!(events.iter().any(|e| matches!(e, ServerEvent::TurnComplete)));
        // close the client connection so the server's persistent loop sees EOF and exits
        drop(conn);
        drop(reader);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_turn_on_one_connection() {
        // 回归：handle_connection 必须在单个连接上处理多个 SendMessage。
        // 修复前（one-request-per-connection）第二个 turn 会挂死。
        let dir = std::env::temp_dir().join(format!("cc_multi_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        ));
        let turn_token = mgr.lock().unwrap().turn_token();
        let shutdown = Arc::new(AtomicBool::new(false));
        let bus = Arc::new(crate::daemon::bus::EventBus::new());

        // 服务端线程：accept 一次并处理（现在是持久循环）。
        let shutdown_c = shutdown.clone();
        let bus_c = Arc::clone(&bus);
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            handle_connection(stream, &mgr, &shutdown_c, &turn_token, &bus_c).unwrap();
        });

        std::thread::sleep(Duration::from_millis(50));
        let mut conn = UnixStream::connect(&sock).unwrap();
        use std::io::Write;

        // turn 1
        let l1 = serde_json::to_string(&ClientRequest::SendMessage { content: "a".into() }).unwrap();
        writeln!(conn, "{l1}").unwrap(); conn.flush().unwrap();

        // drain turn 1 to TurnComplete
        let mut r = BufReader::new(conn.try_clone().unwrap());
        let mut got_complete = false;
        let mut event_count = 0;
        loop {
            let mut buf = String::new();
            let n = r.read_line(&mut buf).unwrap();
            if n == 0 { break; } // EOF
            event_count += 1;
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(buf.trim()) {
                got_complete = true;
                break;
            }
        }
        assert!(got_complete, "turn 1 must complete (got {} events)", event_count);

        // turn 2 (same connection) — this hangs pre-fix
        let l2 = serde_json::to_string(&ClientRequest::SendMessage { content: "b".into() }).unwrap();
        writeln!(conn, "{l2}").unwrap(); conn.flush().unwrap();

        let mut got2 = false;
        let mut event_count2 = 0;
        loop {
            let mut buf = String::new();
            let n = r.read_line(&mut buf).unwrap();
            if n == 0 { break; } // EOF
            event_count2 += 1;
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(buf.trim()) {
                got2 = true;
                break;
            }
        }
        assert!(got2, "turn 2 must complete on the same connection (REPL multi-turn fix, got {} events)", event_count2);

        drop(conn); // close → server loop sees EOF, exits
        drop(r);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bus_notice_reaches_idle_client() {
        // 客户端连上（订阅）但不发起 turn；daemon 广播一条 BusNotice；
        // 客户端即使在「idle」也必须收到——这是实时推送的核心。
        let dir = std::env::temp_dir().join(format!("cc_bus_idle_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let turn_token = mgr.lock().unwrap().turn_token();
        let shutdown = Arc::new(AtomicBool::new(false));
        let bus = Arc::new(crate::daemon::bus::EventBus::new());

        let mgr_c = Arc::clone(&mgr);
        let shutdown_c = shutdown.clone();
        let turn_token_c = turn_token.clone();
        let bus_c = Arc::clone(&bus);
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            handle_connection(stream, &mgr_c, &shutdown_c, &turn_token_c, &bus_c).unwrap();
        });

        // Give server time to start accepting
        std::thread::sleep(Duration::from_millis(50));

        let conn = UnixStream::connect(&sock).unwrap();
        // 客户端不发任何请求（idle）——仅等待 bus 事件。
        let mut r = BufReader::new(conn.try_clone().unwrap());

        // Give the server time to accept and register the connection with the bus
        std::thread::sleep(Duration::from_millis(500));

        // 广播一条
        bus.broadcast("workgraph", "milestone #1 advanced");
        // 客户端应收到 BusNotice（即使 idle）
        let mut got = false;
        for _ in 0..50 {
            let mut buf = String::new();
            // 非阻塞试探：用 set_read_timeout 让 read_line 短超时轮询
            r.get_mut().set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match r.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(ServerEvent::BusNotice { source, text }) = serde_json::from_str(buf.trim()) {
                        if source == "workgraph" && text == "milestone #1 advanced" {
                            got = true;
                            break;
                        }
                    }
                }
                Err(_) => {} // timeout → keep polling
            }
        }
        assert!(got, "idle client must receive the bus notice in real time");
        // 关闭客户端连接：服务端 read_request 见到 EOF → unregister → writer iter() 结束
        // → handle_connection 返回 → h.join() 完成。这是 unregister liveness 修复的关键路径。
        drop(conn);
        drop(r);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // `write_event` 在服务端 handle_connection 使用；这里仅保持测试模块对其的可见引用。
    #[test]
    fn _write_event_is_part_of_proto_api() {
        let ev = ServerEvent::Notice { text: String::new() };
        let mut buf: Vec<u8> = Vec::new();
        write_event(&mut buf, &ev).unwrap();
        assert!(!buf.is_empty());
    }

    /// 回归（C1 writer-leak）：非 EOF 的 Err 出口也必须触发清理——
    /// 即 bus 注销订阅 + drop combined_tx + join writer。
    ///
    /// 触发方式：客户端发送一个畸形首行（非 JSON），使 `read_request` 返回 Err，
    /// `handle_connection` 经 `?` 提前返回 Err。
    ///
    /// 修复前：cleanup 被跳过 → bus 仍持有该订阅的 sender clone → writer 线程的
    /// `iter()` 永久阻塞 → writer 线程泄漏 + 订阅在 bus 中残留。
    ///
    /// 修复后（ConnGuard）：任意 Err 出口都触发 Drop → unregister → writer 退出。
    /// 我们断言：
    ///   (1) handle_connection 的线程在短时间内 is_finished()（不挂）；
    ///   (2) bus 的订阅集合恢复为 0（最迟在一条广播后）。
    #[test]
    fn error_close_path_unregisters_and_reaps_writer() {
        let dir = std::env::temp_dir().join(format!(
            "cc_err_close_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let turn_token = mgr.lock().unwrap().turn_token();
        let shutdown = Arc::new(AtomicBool::new(false));
        let bus = Arc::new(crate::daemon::bus::EventBus::new());

        let mgr_c = Arc::clone(&mgr);
        let shutdown_c = shutdown.clone();
        let turn_token_c = turn_token.clone();
        let bus_c = Arc::clone(&bus);
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            // 期望返回 Err（read_request 解析 "not-json" 失败）。
            let _ = handle_connection(stream, &mgr_c, &shutdown_c, &turn_token_c, &bus_c);
        });

        // 给 server 时间 accept。
        std::thread::sleep(Duration::from_millis(50));

        // 客户端连上 → 发畸形首行。
        {
            use std::io::Write;
            let mut conn = UnixStream::connect(&sock).unwrap();
            conn.write_all(b"not-json\n").unwrap();
            conn.flush().unwrap();
            // 关闭客户端连接（虽然 server 已经因 Err 返回，但确保 write-half 也释放）。
            let _ = conn.shutdown(std::net::Shutdown::Both);
        }

        // (1) handle_connection 必须在短时间内结束——修复前 writer 永久阻塞会让
        //     这个线程永远 is_finished()==false。用 5s deadline 轮询 is_finished()。
        //     （真出现 pre-fix 退化时，本断言会 fail，不会无限挂死套件。）
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let finished = loop {
            if h.is_finished() { break true; }
            if std::time::Instant::now() >= deadline { break false; }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(finished,
            "handle_connection thread must finish on Err close path \
             (pre-fix: writer leak keeps it alive forever)");

        // (2) bus 订阅集合必须恢复为 0：guard 的 unregister 已移除该订阅。
        //     pre-fix 下这里会是 1（残留）。
        let n_after = bus.subscriber_count();
        assert_eq!(n_after, 0,
            "ConnGuard must unregister on Err path (subscriber count must be 0, got {})", n_after);
        // broadcast 一次兜底——不 panic、不残留。
        bus.broadcast("test", "post-close");
        assert_eq!(bus.subscriber_count(), 0);

        // 回收线程（已确认 finished）。
        let _ = h.join();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
