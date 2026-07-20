// cc 客户端连接模块：连 Unix socket，写 ClientRequest 行，读 ServerEvent 行。
use crate::config::Config;
use crate::daemon::proto::{ClientRequest, ServerEvent};
use crate::registry::Registry;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// 一条到 daemon 的连接。`send` 写请求行；`next_event` 读一行事件。
pub struct Connection {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Connection {
    pub fn connect(sock_path: &Path) -> anyhow::Result<Self> {
        let s = UnixStream::connect(sock_path)?;
        let reader = BufReader::new(s.try_clone()?);
        Ok(Self { writer: s, reader })
    }

    pub fn send(&mut self, req: &ClientRequest) -> anyhow::Result<()> {
        // 写一行 ClientRequest（serde_json）+ 换行。注意：`write_event` 是写
        // ServerEvent 的；请求方向是 ClientRequest，故这里手写一行而非复用 write_event。
        use std::io::Write;
        let line = serde_json::to_string(req)?;
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn next_event(&mut self) -> anyhow::Result<Option<ServerEvent>> {
        let mut buf = String::new();
        if self.reader.read_line(&mut buf)? == 0 {
            return Ok(None);
        }
        let ev: ServerEvent = serde_json::from_str(buf.trim())?;
        Ok(Some(ev))
    }
}

/// 默认 socket 路径（与 daemon 一致：`$CODECODER_ROOT/.ccd.sock`）。
pub fn default_sock_path(cfg: &Config) -> std::path::PathBuf {
    crate::daemon::socket::default_sock_path(cfg)
}

/// 把一个 ServerEvent 渲染到 stdout/stderr。返回 true 表示是 turn 终态。
pub fn print_event(ev: &ServerEvent) -> bool {
    use std::io::Write;
    match ev {
        ServerEvent::StreamDelta { text } => {
            print!("{text}");
            let _ = std::io::stdout().flush();
            false
        }
        ServerEvent::Notice { text } => { println!("· {text}"); false }
        ServerEvent::Context { pct } => { eprintln!("[ctx {pct}%]"); false }
        ServerEvent::ToolStarted { name, preview } => { println!("⚙ {name}: {preview}"); false }
        ServerEvent::ToolFinished { name, is_error, output } => {
            if *is_error { eprintln!("  {name} ✗ {output}"); } else { println!("  {name} ✓"); }
            false
        }
        ServerEvent::SessionCreated { id } => { println!("· session {id}"); false }
        ServerEvent::Sessions { ids } => {
            if ids.is_empty() { println!("(no sessions)"); }
            else { for i in ids { println!("{i}"); } }
            false
        }
        ServerEvent::TurnComplete => { println!(); true }
        ServerEvent::Error { message } => { eprintln!("error: {message}"); true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session_manager::DaemonSessionManager;
    use crate::daemon::socket::SocketServer;
    use crate::provider::stub::StubClient;
    use std::sync::{Arc, Mutex, atomic::AtomicBool};

    #[test]
    fn connection_sends_and_receives_turncomplete() {
        let dir = std::env::temp_dir().join(format!("cc_conn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");
        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(Registry::scan(&dir));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let shutdown = Arc::new(AtomicBool::new(false));
        let mgr_c = mgr.clone();
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            crate::daemon::socket::handle_connection(s, &mgr_c, &shutdown_c).unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut conn = Connection::connect(&sock).unwrap();
        conn.send(&ClientRequest::SendMessage { content: "hi".into() }).unwrap();
        let mut done = false;
        while let Some(ev) = conn.next_event().unwrap() {
            if print_event(&ev) { done = true; break; }
        }
        assert!(done);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
