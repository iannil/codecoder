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
/// 当前仅支持 `SendMessage`/`NewSession`/`ListSessions`/`Shutdown`；其余回 Error。
pub fn handle_connection(
    stream: UnixStream,
    mgr: &Mutex<super::session_manager::DaemonSessionManager>,
    shutdown: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    use super::proto::{read_request, write_event, ClientRequest, ServerEvent};
    use std::io::BufWriter;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    // 简化：单连接只处理第一个请求（M1 足够；REPL 多请求在 Task 3 由客户端循环驱动）。
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
            write_event(&mut writer, &ServerEvent::Sessions { ids: g.list() })?;
        }
        ClientRequest::SendMessage { content } => {
            // 没指定 session → 自动取第一个（或新建）。
            let id = match g.list().first().cloned() {
                Some(id) => id,
                None => g.create(),
            };
            let rx = g.send_message(&id, content)?;
            drop(g); // 释放 mgr 锁，让 agent 线程推进
            for ev in rx.iter() {
                write_event(&mut writer, &ev)?;
                if matches!(ev, ServerEvent::TurnComplete) {
                    break;
                }
            }
        }
        ClientRequest::Shutdown => {
            shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            write_event(&mut writer, &ServerEvent::Notice { text: "shutting down".into() })?;
        }
        other => {
            write_event(&mut writer, &ServerEvent::Error {
                message: format!("unsupported in M1: {other:?}"),
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::proto::{write_event, ClientRequest, ServerEvent};
    use crate::daemon::session_manager::DaemonSessionManager;
    use crate::provider::stub::StubClient;
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
        let mgr = Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ));
        let shutdown = Arc::new(AtomicBool::new(false));

        // 服务端线程：accept 一次并处理。
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            handle_connection(stream, &mgr, &shutdown_c).unwrap();
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
