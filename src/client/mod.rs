// cc 客户端连接模块：连 Unix socket，写 ClientRequest 行，读 ServerEvent 行。
use crate::config::Config;
use crate::daemon::proto::{ClientRequest, ServerEvent};
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
        ServerEvent::Prompt { .. } => {
            // 暂时：Prompt 事件由交互式循环处理，print_event 仅用于非交互式输出。
            // 这里应该 unreachable，但为了兼容性先打印一个占位符。
            eprintln!("(prompt event should be handled by interactive loop)");
            false
        }
        ServerEvent::BusNotice { source, text } => { println!("· [{source}] {text}"); false }
    }
}

/// 交互式提示用户回答，返回对应的 PromptAnswer（Task 9a）。
pub fn prompt_user(_id: u64, body: &crate::daemon::proto::PromptBody) -> crate::daemon::proto::PromptAnswer {
    use crate::daemon::proto::{PromptAnswer, PermissionGrant, TrustDecisionWire};
    use std::io::{self, Write};

    match body {
        crate::daemon::proto::PromptBody::Permission { key, preview } => {
            println!("🔐 Permission request: {key}");
            println!("  Preview: {preview}");
            print!("[y]es / [n]o / [s]ession-always / [p]roject-always / [N]ever? ");
            let _ = io::stdout().flush();

            let mut input = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input");
            let choice = input.trim().to_lowercase();

            let grant = match choice.as_str() {
                "y" | "yes" => PermissionGrant::Once,
                "s" | "session" => PermissionGrant::AlwaysThisSession,
                "p" | "project" => PermissionGrant::AlwaysThisProject,
                "n" | "no" => PermissionGrant::Deny,
                "never" | "cancel" | "c" => PermissionGrant::Cancelled,
                _ => PermissionGrant::Deny, // default to deny on invalid input
            };
            PromptAnswer::Permission { grant }
        }
        crate::daemon::proto::PromptBody::AskUser { prompt } => {
            println!("{prompt}");
            print!("> ");
            let _ = io::stdout().flush();

            let mut input = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input");
            PromptAnswer::AskUser { text: input.trim().to_string() }
        }
        crate::daemon::proto::PromptBody::Confirm { prompt } => {
            print!("{prompt} [y/n]: ");
            let _ = io::stdout().flush();

            let mut input = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input");
            let yes = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
            PromptAnswer::Confirm { yes }
        }
        crate::daemon::proto::PromptBody::PlanApproval { plan } => {
            println!("Plan:\n{plan}");
            print!("Approve? [y/n]: ");
            let _ = io::stdout().flush();

            let mut input = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input");
            let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
            PromptAnswer::PlanApproval { approved }
        }
        crate::daemon::proto::PromptBody::Trust { root } => {
            println!("Trust this project's disk self? {root}");
            print!("[a]lways / [o]nce / [n]ever: ");
            let _ = io::stdout().flush();

            let mut input = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input");
            let decision = match input.trim().to_lowercase().as_str() {
                "a" | "always" => TrustDecisionWire::Always,
                "o" | "once" => TrustDecisionWire::Once,
                "n" | "never" => TrustDecisionWire::Never,
                _ => TrustDecisionWire::Once, // default to once on invalid input
            };
            PromptAnswer::Trust { decision }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session_manager::DaemonSessionManager;
    use crate::daemon::socket::SocketServer;
    use crate::provider::stub::StubClient;
    use crate::registry::Registry;
    use std::sync::{Arc, Mutex, atomic::AtomicBool};

    #[test]
    fn connection_sends_and_receives_turncomplete() {
        let dir = std::env::temp_dir().join(format!("cc_conn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");
        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let shutdown = Arc::new(AtomicBool::new(false));
        let mgr_c = mgr.clone();
        let shutdown_c = shutdown.clone();
        let turn_token = mgr.lock().unwrap().turn_token();
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            crate::daemon::socket::handle_connection(s, &mgr_c, &shutdown_c, &turn_token).unwrap();
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
