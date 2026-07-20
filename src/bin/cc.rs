// cc — 薄 CLI 客户端，连 ccd daemon（$CODECODER_ROOT/.ccd.sock）。
// 无 ratatui，纯 stdin/stdout。
use codecoder::client::{default_sock_path, print_event, Connection};
use codecoder::daemon::proto::ClientRequest;
use codecoder::Config;
use std::io::{BufRead, Write};

fn main() -> anyhow::Result<()> {
    let cfg = Config::from_env();
    let sock = default_sock_path(&cfg);
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.as_slice() {
        [] => repl(&sock),
        [one] if one == "sessions" => send_one(&sock, ClientRequest::ListSessions),
        [one] if one == "status" => send_one(&sock, ClientRequest::Status),
        [one] if one == "shutdown" => send_one(&sock, ClientRequest::Shutdown),
        [msg @ ..] => {
            // cc "hello world" — 一次性发送
            let content = msg.join(" ");
            send_one(&sock, ClientRequest::SendMessage { content })
        }
    }
}

/// 发单个请求，打印所有事件直到终态，退出。
fn send_one(sock: &std::path::Path, req: ClientRequest) -> anyhow::Result<()> {
    let mut conn = Connection::connect(sock)
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {}: {e}\n(is `ccd` running? CODECODER_DAEMON=1 cargo run)", sock.display()))?;
    conn.send(&req)?;
    loop {
        match conn.next_event()? {
            None => break,
            Some(ev) => {
                if print_event(&ev) { break; }
            }
        }
    }
    Ok(())
}

/// REPL：读 stdin 一行 → 发 SendMessage → 流式打印 → 直到 TurnComplete。
fn repl(sock: &std::path::Path) -> anyhow::Result<()> {
    let mut conn = Connection::connect(sock)
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {}: {e}", sock.display()))?;
    // 开一个默认 session（若已有则复用第一个）。
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        print!("cc> ");
        std::io::stdout().flush()?;
        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 { break; } // EOF
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if trimmed == "/exit" || trimmed == "/quit" { break; }
        conn.send(&ClientRequest::SendMessage { content: trimmed.to_string() })?;
        loop {
            match conn.next_event()? {
                None => break,
                Some(ev) => {
                    if print_event(&ev) { break; }
                }
            }
        }
    }
    Ok(())
}

// 显式引入避免 unused 告警。
#[allow(unused_imports)]
use std::io::Write as _;
