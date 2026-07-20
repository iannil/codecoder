// Unix socket listener：bind、accept、按行读写 JSON 帧。socket 路径默认
// `$CODECODER_ROOT/.ccd.sock`。
use crate::config::Config;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
) -> anyhow::Result<()> {
    use super::proto::{read_request, write_event, ClientRequest, ServerEvent};
    use std::io::BufWriter;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    // 简化：单连接只处理第一个请求（M1 足够；REPL 多请求在客户端循环驱动）。
    let Some(req) = read_request(&mut reader)? else {
        return Ok(());
    };
    let mut g = mgr.lock().unwrap();
    match req {
        ClientRequest::NewSession => {
            let id = g.create();
            write_event(&mut writer, &ServerEvent::SessionCreated { id })?;
        }
        ClientRequest::ListSessions => {
            write_event(&mut writer, &ServerEvent::Sessions { ids: g.disk_sessions() })?;
        }
        ClientRequest::Resume { id } => {
            let rx = g.resume(&id)?;
            drop(g); // release the manager lock BEFORE draining so other clients can proceed
            // 持有 turn_token 全程——workgraph tick 线程的 try_lock 探测到此即跳过推进。
            let _turn_guard = turn_token.lock().unwrap();
            drain_agent_events(rx, &mut reader, &mut writer)?;
        }
        ClientRequest::SendMessage { content } => {
            // 没指定 session → 自动取第一个（或新建）。
            let id = match g.list().first().cloned() {
                Some(id) => id,
                None => g.create(),
            };
            let rx = g.send_message(&id, content)?;
            drop(g); // release the manager lock BEFORE draining so other clients can proceed
            // 持有 turn_token 全程——workgraph tick 线程的 try_lock 探测到此即跳过推进。
            let _turn_guard = turn_token.lock().unwrap();
            drain_agent_events(rx, &mut reader, &mut writer)?;
        }
        ClientRequest::Shutdown => {
            shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            write_event(&mut writer, &ServerEvent::Notice { text: "shutting down".into() })?;
        }
        other => {
            write_event(&mut writer, &ServerEvent::Error {
                message: format!("unsupported: {other:?}"),
            })?;
        }
    }
    Ok(())
}

/// drain_loop: 读取原始 AgentEvent，翻译成 ServerEvent 写回客户端，遇到 prompt 时
/// 内联阻塞读取 PromptReply（synchronous 模型，无需新线程）。
fn drain_agent_events(
    rx: std::sync::mpsc::Receiver<crate::agent::AgentEvent>,
    reader: &mut BufReader<UnixStream>,
    writer: &mut std::io::BufWriter<UnixStream>,
) -> anyhow::Result<()> {
    use crate::agent::AgentEvent;
    use super::proto::{PromptBody, ServerEvent, write_event};

    let mut prompt_id = 0u64;
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(AgentEvent::StreamDelta(text)) => {
                write_event(writer, &ServerEvent::StreamDelta { text })?;
            }
            Ok(AgentEvent::Notice(text)) => {
                write_event(writer, &ServerEvent::Notice { text })?;
            }
            Ok(AgentEvent::Context { pct }) => {
                write_event(writer, &ServerEvent::Context { pct })?;
            }
            Ok(AgentEvent::ToolStarted { name, preview }) => {
                write_event(writer, &ServerEvent::ToolStarted { name, preview })?;
            }
            Ok(AgentEvent::ToolFinished { name, is_error, output }) => {
                write_event(writer, &ServerEvent::ToolFinished { name, is_error, output })?;
            }
            Ok(AgentEvent::TurnComplete) => {
                write_event(writer, &ServerEvent::TurnComplete)?;
                break;
            }
            // ===== Task 9a: 5 种 prompt 事件的内联处理 =====
            Ok(AgentEvent::PermissionRequest { key, preview, reply_tx }) => {
                prompt_id += 1;
                write_event(writer, &ServerEvent::Prompt {
                    id: prompt_id,
                    body: PromptBody::Permission { key, preview },
                })?;
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.to_permission_reply());
            }
            Ok(AgentEvent::AskUser { prompt, reply_tx }) => {
                prompt_id += 1;
                write_event(writer, &ServerEvent::Prompt {
                    id: prompt_id,
                    body: PromptBody::AskUser { prompt },
                })?;
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.into_text());
            }
            Ok(AgentEvent::Confirm { prompt, reply_tx }) => {
                prompt_id += 1;
                write_event(writer, &ServerEvent::Prompt {
                    id: prompt_id,
                    body: PromptBody::Confirm { prompt },
                })?;
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.yes());
            }
            Ok(AgentEvent::PlanApproval { plan, reply_tx }) => {
                prompt_id += 1;
                write_event(writer, &ServerEvent::Prompt {
                    id: prompt_id,
                    body: PromptBody::PlanApproval { plan },
                })?;
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.approved());
            }
            Ok(AgentEvent::TrustPrompt { root, reply_tx }) => {
                prompt_id += 1;
                let root_str = root.to_string_lossy().to_string();
                write_event(writer, &ServerEvent::Prompt {
                    id: prompt_id,
                    body: PromptBody::Trust { root: root_str },
                })?;
                let ans = read_prompt_reply(reader, prompt_id)?;
                let _ = reply_tx.send(ans.to_trust_reply());
            }
            // Sub-agent 进度（agent/review 工具产出）——以 Notice 转发，保留可见性。
            Ok(AgentEvent::SubAgentMilestone(s)) => {
                write_event(writer, &ServerEvent::Notice { text: format!("↳ {s}") })?;
            }
            // Chain-of-thought——以 Notice 转发（无独立 wire 变体；ADR 0032 Negative 中记录）。
            Ok(AgentEvent::Reasoning(s)) => {
                write_event(writer, &ServerEvent::Notice { text: format!("💭 {s}") })?;
            }
            // 其他 AgentEvent 变体（Test*, L4*）仍丢弃：仅 L4 verify 场景产出，
            // 暂未在 wire 协议中暴露（见 ADR 0032 Negative consequences）。
            Ok(_) => { /* drop unserializable events */ }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                write_event(writer, &ServerEvent::Error {
                    message: "turn timed out (agent unresponsive)".into(),
                })?;
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                write_event(writer, &ServerEvent::Error {
                    message: "agent disconnected".into(),
                })?;
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

        // 服务端线程：accept 一次并处理。
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            handle_connection(stream, &mgr, &shutdown_c, &turn_token).unwrap();
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
        h.join().unwrap();
        assert!(events.iter().any(|e| matches!(e, ServerEvent::TurnComplete)));
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
}
