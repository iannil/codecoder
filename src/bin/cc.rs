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
        [one] if one == "tree" => send_one(&sock, ClientRequest::TreeShow),
        [one, id] if one == "fork" => {
            let id: u64 = id.parse().map_err(|e| anyhow::anyhow!("fork <id>: {e}"))?;
            send_one(&sock, ClientRequest::TreeNav { id })
        }
        [one] if one == "clone" => send_one(&sock, ClientRequest::TreeClone),
        [one, rest @ ..] if one == "ledger" => {
            // 直读 bg_ledger.jsonl,不经 daemon(BG 独立于 daemon)。
            let root = codecoder::Config::from_env().root;
            let mut n: usize = 10;
            let mut only_failed = false;
            let mut detail = false;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--failed" => only_failed = true,
                    "--detail" => detail = true,
                    "--last" => {
                        n = it.next().and_then(|v| v.parse().ok()).unwrap_or(10);
                    }
                    other => {
                        eprintln!("cc ledger: unknown flag '{other}'");
                        std::process::exit(2);
                    }
                }
            }
            let recs =
                codecoder::bg_ledger::read_recent(&root, if detail { 1 } else { n }, only_failed);
            if recs.is_empty() {
                println!("(no bg_ledger.jsonl yet, or no matching records)");
            } else if detail {
                let r = recs.last().unwrap();
                println!("{}  task={}", codecoder::bg_ledger::format_utc(r.ts), r.task);
                println!("  mission: {:?}", r.mission_state);
                println!("  counts:  {:?}", r.counts);
                for sg in &r.subgoals {
                    println!(
                        "  - milestone #{}: {:?}  {}",
                        sg.milestone_id, sg.verdict, sg.gate_reason
                    );
                }
            } else {
                for r in &recs {
                    println!("{}", codecoder::bg_ledger::summarize_line(r));
                }
            }
            Ok(())
        }
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

        // Tree-sessions slash commands
        if trimmed == "/tree" {
            writer.lock().unwrap().send(&ClientRequest::TreeShow)?;
            let _ = done_rx.recv(); // TreeShow -> Tree + TurnComplete -> reader signals done
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("/fork ") {
            let id: u64 = rest.trim().parse().map_err(|e| anyhow::anyhow!("/fork <id>: {e}"))?;
            writer.lock().unwrap().send(&ClientRequest::TreeNav { id })?;
            let _ = done_rx.recv(); // Navigate -> Notice + TurnComplete
            continue;
        }
        if trimmed == "/clone" {
            writer.lock().unwrap().send(&ClientRequest::TreeClone)?;
            let _ = done_rx.recv();
            continue;
        }

        writer.lock().unwrap().send(&ClientRequest::SendMessage { content: trimmed.to_string() })?;
        // 等 reader 线程通知 turn 结束（期间 reader 可能读 stdin 答 prompt）。
        let _ = done_rx.recv();
    }

    // I1 fix：cc 在 /exit 或 stdin-EOF 时，必须主动关闭写半（SHUT_WR）才能让 daemon
    // 的 read_request 见到 EOF 而返回——否则会形成三方死锁：
    //   - main 阻塞在 reader_handle.join()；
    //   - reader 线程阻塞在 reader.next_event() 等 daemon 关闭写半；
    //   - daemon 阻塞在 read_request 等 cc 关闭写半；
    //   - cc 的写半被 writer Arc（main）+ writer_for_reader Arc（reader 线程）共同持有，
    //     都不释放 → daemon 永远见不到 EOF → reader 永远见不到 EOF → main 永远 join 不上。
    //
    // shutdown_write 走 SHUT_WR（内核级关闭写方向），不依赖 drop 顺序，也不需要 reader
    // 线程先释放它的 Arc clone。daemon 收到 EOF → ConnGuard 清理 → 关闭 daemon 写半
    // → reader 的 next_event 见到 EOF → reader 线程退出 → main 的 join 返回。
    let _ = writer.lock().map_err(|e| anyhow::anyhow!("writer poisoned: {e}"))?.shutdown_write();
    let _ = reader_handle.join();
    Ok(())
}

// 显式引入避免 unused 告警。
#[allow(unused_imports)]
use std::io::Write as _;
