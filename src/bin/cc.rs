// cc — 薄 CLI 客户端，连 ccd daemon（$CODECODER_ROOT/.ccd.sock）。
// 无 ratatui，纯 stdin/stdout。
use codecoder::client::{default_sock_path, print_event, prompt_user, Connection};
use codecoder::daemon::proto::{ClientRequest, ServerEvent};
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

/// 发单个请求，打印所有事件直到终态，退出（Task 9a: 支持 Prompt）。
fn send_one(sock: &std::path::Path, req: ClientRequest) -> anyhow::Result<()> {
    let mut conn = Connection::connect(sock)
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {}: {e}\n(is `ccd` running? CODECODER_DAEMON=1 cargo run)", sock.display()))?;
    conn.send(&req)?;
    loop {
        match conn.next_event()? {
            None => break,
            Some(ev) => {
                // Task 9a: 交互式处理 Prompt 事件
                if let codecoder::daemon::proto::ServerEvent::Prompt { id, body } = ev {
                    let answer = prompt_user(id, &body);
                    conn.send(&codecoder::daemon::proto::ClientRequest::PromptReply { id, answer })?;
                    continue;
                }
                if print_event(&ev) { break; }
            }
        }
    }
    Ok(())
}

/// REPL：读 stdin 一行 → 发 SendMessage → 流式打印 → 直到 TurnComplete（Task 9a: 支持 Prompt）。
fn repl(sock: &std::path::Path) -> anyhow::Result<()> {
    use codecoder::client::Connection;
    use std::sync::{mpsc, Arc, Mutex};

    let conn = Connection::connect(sock)
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {}: {e}", sock.display()))?;
    let (writer, mut reader) = conn.split()?;
    let writer = Arc::new(Mutex::new(writer));

    // turn-done 信号：reader 线程在 TurnComplete/Error/EOF 时通知 main。
    let (done_tx, done_rx) = mpsc::channel::<()>();

    let writer_for_reader = Arc::clone(&writer);
    let done_tx_clone = done_tx.clone();
    let reader_handle = std::thread::spawn(move || {
        loop {
            match reader.next_event() {
                Ok(None) | Err(_) => {
                    let _ = done_tx_clone.send(());
                    break;
                }
                Ok(Some(ServerEvent::Prompt { id, body })) => {
                    // turn 中 main 阻塞在 done_rx，stdin 空闲——reader 读 stdin 答 prompt。
                    let answer = prompt_user(id, &body);
                    let _ = writer_for_reader.lock().unwrap().send(
                        &ClientRequest::PromptReply { id, answer },
                    );
                }
                Ok(Some(ev)) => {
                    let terminal = print_event(&ev);
                    if terminal {
                        let _ = done_tx_clone.send(());
                    }
                }
            }
        }
    });

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
        writer.lock().unwrap().send(&ClientRequest::SendMessage { content: trimmed.to_string() })?;
        // 等 reader 线程通知 turn 结束（期间 reader 可能读 stdin 答 prompt）。
        let _ = done_rx.recv();
    }
    let _ = reader_handle.join();
    Ok(())
}

// 显式引入避免 unused 告警。
#[allow(unused_imports)]
use std::io::Write as _;
