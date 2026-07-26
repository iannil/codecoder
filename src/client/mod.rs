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

    /// 拆成写半 + 读半，供 main 线程与 reader 线程并发使用。
    pub fn split(self) -> anyhow::Result<(ConnectionWriter, ConnectionReader)> {
        let writer = self.writer;             // UnixStream
        let reader = BufReader::new(self.reader.into_inner().try_clone()?);
        Ok((ConnectionWriter { writer }, ConnectionReader { reader }))
    }
}

/// 连接的写半（供 main 线程 send）。
pub struct ConnectionWriter {
    writer: UnixStream,
}

impl ConnectionWriter {
    pub fn send(&mut self, req: &ClientRequest) -> anyhow::Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(req)?;
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;
        Ok(())
    }

    /// 关闭写半（SHUT_WR）。让 daemon 的读半见到 EOF，从而触发其 handle_connection
    /// 返回 → 关闭 daemon 的写半 → 本连接的 reader 见到 EOF 退出。
    /// 用于 cc REPL 在 `/exit` 时打破「reader 等 daemon EOF / daemon 等 cc EOF」
    /// 的循环死锁。读取 `&self` 即可调用——UnixStream::shutdown 不需要 `&mut`。
    pub fn shutdown_write(&self) -> std::io::Result<()> {
        self.writer.shutdown(std::net::Shutdown::Write)
    }
}

/// 连接的读半（供 reader 线程 next_event）。
pub struct ConnectionReader {
    reader: BufReader<UnixStream>,
}

impl ConnectionReader {
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

/// 渲染会话树为字符串：按 parent 链缩进，active 路径前缀 ►，leaf 前缀 ●，废弃分支无标记。
/// 纯函数（不打印），便于单测。
pub fn print_tree(nodes: &[crate::daemon::proto::TreeNode]) -> String {
    use std::collections::HashMap;
    let by_id: HashMap<u64, &crate::daemon::proto::TreeNode> =
        nodes.iter().map(|n| (n.id, n)).collect();
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut roots: Vec<u64> = Vec::new();
    for n in nodes {
        match n.parent {
            None => roots.push(n.id),
            Some(p) => children.entry(p).or_default().push(n.id),
        }
    }
    // 稳定排序：按 id 升序，保证渲染确定性
    for v in children.values_mut() { v.sort(); }
    roots.sort();

    let mut out = String::new();
    fn rec(
        id: u64,
        depth: usize,
        by_id: &HashMap<u64, &crate::daemon::proto::TreeNode>,
        children: &HashMap<u64, Vec<u64>>,
        out: &mut String,
    ) {
        let n = by_id.get(&id).copied().unwrap();
        let prefix = if n.is_leaf { "●" }
            else if n.on_active_path { "►" }
            else { " " };
        let indent = "  ".repeat(depth);
        // Append causal marker when present
        let causal = match (n.causal_node, n.status.as_deref()) {
            (Some(cn), Some("ruled_out")) => format!(" (✗H#{cn} ruled_out)"),
            (Some(cn), Some(st)) => format!(" (→H#{cn} {st})"),
            (Some(cn), None) => format!(" (→H#{cn})"),
            _ => String::new(),
        };
        out.push_str(&format!("{indent}{prefix} [{}] {}: {}{causal}\n", n.id, n.role, n.preview));
        if let Some(kids) = children.get(&id) {
            for c in kids { rec(*c, depth + 1, by_id, children, out); }
        }
    }
    for r in roots { rec(r, 0, &by_id, &children, &mut out); }
    out
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
        ServerEvent::Status(s) => {
            println!("daemon status:");
            println!("  uptime: {}s", s.uptime_secs);
            println!("  sessions: {} ({})", s.active_sessions, s.session_ids.join(", "));
            for svc in &s.supervisor_services {
                let status = if svc.gave_up { "FAILED" } else { "running" };
                println!("  service: {} ({}) at {}", svc.name, status, svc.address);
            }
            for t in &s.threads {
                let last = t.last_tick.map(|ts| format!("tick={}", ts)).unwrap_or_else(|| "none".into());
                println!("  thread: {} ({} ticks, last: {}, event: {})", t.name, t.tick_count, last, t.last_event);
            }
            false
        }
        ServerEvent::Tree { nodes } => {
            print!("{}", print_tree(nodes));
            let _ = std::io::stdout().flush();
            false
        }
        ServerEvent::Services(payload) => {
            let services = &payload.services;
            if services.is_empty() {
                println!("(no persistent services)");
            } else {
                println!("persistent services:");
                for svc in services {
                    let status = if svc.gave_up { "FAILED" } else { "running" };
                    println!("  {}  {}  {}", status, svc.name,
                        if svc.address.is_empty() { "(no address)" } else { &svc.address });
                }
            }
            false
        }
        ServerEvent::WorkgraphStatus(s) => {
            if s.total == 0 {
                println!("workgraph: (empty — seed workgraph.json first)");
            } else {
                println!("workgraph: {} milestones", s.total);
                println!("  pending:   {}", s.pending);
                println!("  done:      {}", s.done);
                println!("  needs_fix: {}", s.needs_fix);
                println!("  blocked:   {}", s.blocked);
                if s.paused {
                    println!("  auto-advance: PAUSED");
                }
                if let Some(ref t) = s.last_advanced {
                    println!("  last:      {}", t);
                }
            }
            false
        }
        ServerEvent::AutotaskStatus(s) => {
            println!("autotask:");
            println!("  running:    {}", s.running);
            println!("  last_event: {}", s.last_event);
            println!("  ticks:      {}", s.tick_count);
            false
        }
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
        let bus = Arc::new(crate::daemon::bus::EventBus::new());
        let bus_c = Arc::clone(&bus);
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            crate::daemon::socket::handle_connection(s, &mgr_c, &shutdown_c, &turn_token, &bus_c).unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut conn = Connection::connect(&sock).unwrap();
        conn.send(&ClientRequest::SendMessage { content: "hi".into() }).unwrap();
        let mut done = false;
        while let Some(ev) = conn.next_event().unwrap() {
            if print_event(&ev) { done = true; break; }
        }
        assert!(done);
        // 关闭客户端连接 → 服务端 handle_connection 的 read_request 见 EOF
        // → 注销 bus → writer 退出 → handle_connection 返回，h.join() 才不会挂。
        drop(conn);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn print_event_renders_bus_notice() {
        let ev = ServerEvent::BusNotice { source: "workgraph".into(), text: "milestone done".into() };
        // print_event prints to stdout; we only assert it's non-terminal (returns false)
        // and doesn't panic. (Terminal events are TurnComplete/Error.)
        assert!(!print_event(&ev));
    }

    #[test]
    fn print_tree_marks_active_path_leaf_and_abandoned() {
        use crate::daemon::proto::TreeNode;
        // root(0) → A(1) → leaf(3)   [active]
        //        → B(2)               [abandoned]
        let nodes = vec![
            TreeNode { id: 0, parent: None,      role: "user".into(),      preview: "root".into(),      is_leaf: false, on_active_path: true, causal_node: None, status: None },
            TreeNode { id: 1, parent: Some(0),   role: "assistant".into(), preview: "A".into(),         is_leaf: false, on_active_path: true, causal_node: None, status: None },
            TreeNode { id: 2, parent: Some(0),   role: "assistant".into(), preview: "B".into(),         is_leaf: false, on_active_path: false, causal_node: None, status: None },
            TreeNode { id: 3, parent: Some(1),   role: "user".into(),      preview: "leaf".into(),      is_leaf: true,  on_active_path: true, causal_node: None, status: None },
        ];
        let rendered = print_tree(&nodes);
        // active path nodes (0,1,3) get ► or ●; abandoned (2) gets a space prefix
        assert!(rendered.contains("► [0]"));
        assert!(rendered.contains("► [1]"));
        assert!(rendered.contains("● [3]"));
        assert!(rendered.contains("  [2]")); // abandoned: leading space, no marker
        // indentation depth: leaf at depth 2 (4 spaces: 2 per depth)
        assert!(rendered.contains("    ● [3]"));
    }

    #[test]
    fn print_tree_renders_causal_markers() {
        use crate::daemon::proto::TreeNode;
        // Test causal link markers
        let nodes = vec![
            TreeNode { id: 0, parent: None,      role: "user".into(),      preview: "root".into(),      is_leaf: false, on_active_path: true, causal_node: None, status: None },
            TreeNode { id: 1, parent: Some(0),   role: "assistant".into(), preview: "A".into(),         is_leaf: false, on_active_path: true, causal_node: Some(5), status: Some("hypothesis".into()) },
            TreeNode { id: 2, parent: Some(0),   role: "assistant".into(), preview: "B".into(),         is_leaf: false, on_active_path: false, causal_node: Some(7), status: Some("ruled_out".into()) },
        ];
        let rendered = print_tree(&nodes);
        // hypothesis status should show →H#5 (hypothesis)
        assert!(rendered.contains("→H#5"), "hypothesis marker should show");
        // ruled_out status should show ✗H#7 ruled_out
        assert!(rendered.contains("✗H#7 ruled_out"), "ruled_out marker should show");
    }
}
