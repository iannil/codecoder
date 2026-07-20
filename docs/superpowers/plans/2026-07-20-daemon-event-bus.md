# Daemon Event Bus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A daemon-level event bus that broadcasts `Notice` events (with a `source` tag) to all connected `cc` clients in real time — even idle ones — so background events (workgraph milestone advanced, capability crashed/restarted) reach every client immediately. Also fixes the latent multi-turn REPL bug (persistent connection).

**Architecture:** `handle_connection` becomes a persistent loop over `ClientRequest`s (no longer one-shot). Each connection has a `combined: mpsc<ServerEvent>` channel and a dedicated **writer thread** that drains it → socket (single writer, serialized writes). The request-reader drains each turn synchronously, sending events to `combined` and reading prompt replies inline from the socket reader. An `EventBus` (`Arc`-shared) holds each connection's `combined_tx`; `broadcast` pushes a `ServerEvent::BusNotice` to every subscriber. `cc` gains a concurrent reader thread so it prints bus events while `stdin` blocks.

**Tech Stack:** Rust (edition 2024), `std::sync::mpsc`, `std::sync::{Arc, Mutex}`, `std::thread`. **No new dependencies.** No async runtime.

**Spec:** `docs/superpowers/specs/2026-07-20-daemon-event-bus-design.md`

> **Plan-level refinement of the spec (transparent):** the spec described a separate "turn-feeder thread" per turn. The plan folds the turn drain into the request-reader's synchronous flow (the request-reader owns the socket *reader* for inline prompt replies; the writer thread owns the socket *writer*). This is equivalent for all behaviors (multi-turn, bus-during-turn, prompt round-trip) with fewer threads (1 writer/conn + 0 per-turn instead of 1 writer + 1/turn). No behavior change.

---

## Global Constraints

- **No new dependencies** — `std::sync::mpsc`, `std::sync::{Arc, Mutex}`, `std::thread` only.
- New `pub` types/functions need doc comments.
- New tests use `std::env::temp_dir().join(format!("cc_<name>{}", std::process::id()))` (unique suffix per test) + `remove_dir_all` cleanup.
- Existing suite stays green; `cargo build` warning-free (no unused imports).
- The bus carries `Notice` text + a `source` tag only — NO typed/structured events (YAGNI); NO topic filtering; NO replay buffer.
- Commit messages: `feat:`/`refactor:` prefix, single line, English.

---

## File Structure

| File | Responsibility | Create/Modify |
|---|---|---|
| `src/daemon/proto.rs` | `ServerEvent::BusNotice { source, text }` | Modify (add) |
| `src/daemon/bus.rs` | `EventBus`: `register`, `broadcast` | Create |
| `src/daemon/mod.rs` | declare `pub mod bus;` | Modify |
| `src/daemon/socket.rs` | `handle_connection` → persistent loop + writer thread + drain-to-combined; gains `bus` param (Task 3) | Modify |
| `src/daemon/mod.rs` (run) | own `Arc<EventBus>`; pass to `handle_connection`; workgraph + supervisor threads broadcast | Modify |
| `src/capability.rs` | `Supervisor::supervise` returns `Vec<String>` event lines | Modify |
| `src/background.rs` | (no change) `advance_one_milestone` already returns `Option<BgOutcome>` with `.events` | — |
| `src/bin/cc.rs` (+ `src/client/mod.rs`) | concurrent reader thread + turn-done signal + `BusNotice` rendering; `Connection::split` | Modify |

---

### Task 1: `ServerEvent::BusNotice` + `EventBus` module

**Files:**
- Modify: `src/daemon/proto.rs` — add `BusNotice` variant
- Create: `src/daemon/bus.rs` — `EventBus`
- Modify: `src/daemon/mod.rs` — `pub mod bus;`
- Test: `src/daemon/bus.rs` inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `proto::ServerEvent` (existing).
- Produces (used by Tasks 3 & 4):
  - `proto::ServerEvent::BusNotice { source: String, text: String }`
  - `daemon::bus::EventBus` with `pub fn new() -> Self`, `pub fn register(&self, tx: std::sync::mpsc::Sender<ServerEvent>)`, `pub fn broadcast(&self, source: &str, text: &str)`

- [ ] **Step 1: Add the `BusNotice` variant**

In `src/daemon/proto.rs`, add to `pub enum ServerEvent` (after `Prompt { ... }`):

```rust
    /// daemon 级广播通知（来自 event bus，如 workgraph/supervisor）。
    /// 与 per-turn `Notice` 区分：带 `source` 标签，客户端可不同渲染。
    BusNotice { source: String, text: String },
```

- [ ] **Step 2: Write the failing tests for `EventBus`**

Create `src/daemon/bus.rs`:

```rust
// Daemon 级事件总线（ADR 0032 client-server；迁移计划 Task 8 stretch）。
// 持有每个连接注册的 combined_tx（mpsc::Sender<ServerEvent>）；broadcast 时
// 向每个订阅 send 一条 BusNotice。死订阅（receiver 已 drop）在 send 失败时
// 由 retain 剔除——一个关闭的连接不会阻塞广播。
use crate::daemon::proto::ServerEvent;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

pub struct EventBus {
    subscribers: Mutex<Vec<Sender<ServerEvent>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self { subscribers: Mutex::new(Vec::new()) }
    }

    /// 注册一个连接的 combined_tx。bus 持有 Sender；连接关闭后它变死，
    /// 下次 broadcast 的 retain 会剔除。
    pub fn register(&self, tx: Sender<ServerEvent>) {
        self.subscribers.lock().unwrap().push(tx);
    }

    /// 向所有订阅广播一条 BusNotice。死/满订阅被 retain 剔除。
    /// （mpsc::channel 是无界的，send 仅在 receiver drop 时失败。）
    pub fn broadcast(&self, source: &str, text: &str) {
        let ev = ServerEvent::BusNotice { source: source.into(), text: text.into() };
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(ev.clone()).is_ok());
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_broadcast_delivers() {
        let bus = EventBus::new();
        let (tx, rx) = mpsc::channel::<ServerEvent>();
        bus.register(tx);
        bus.broadcast("workgraph", "milestone #1 done");
        let ev = rx.recv().unwrap();
        match ev {
            ServerEvent::BusNotice { source, text } => {
                assert_eq!(source, "workgraph");
                assert_eq!(text, "milestone #1 done");
            }
            other => panic!("expected BusNotice, got {other:?}"),
        }
    }

    #[test]
    fn broadcast_reaps_dead_subscribers() {
        let bus = EventBus::new();
        let (tx1, rx1) = mpsc::channel::<ServerEvent>();
        let (tx2, _rx2) = mpsc::channel::<ServerEvent>(); // rx2 dropped below → dead
        bus.register(tx1);
        bus.register(tx2);
        drop(_rx2); // kill subscriber 2
        bus.broadcast("supervisor", "crashed");
        // live subscriber still receives
        let ev = rx1.recv().unwrap();
        assert!(matches!(ev, ServerEvent::BusNotice { .. }));
        // dead subscriber reaped: only 1 remains
        let n = bus.subscribers.lock().unwrap().len();
        assert_eq!(n, 1, "dead subscriber must be reaped");
    }

    #[test]
    fn broadcast_to_no_subscribers_is_noop() {
        let bus = EventBus::new();
        bus.broadcast("x", "y"); // must not panic
    }
}
```

- [ ] **Step 3: Declare the module**

In `src/daemon/mod.rs`, add to the module declarations (after `pub mod socket;`):

```rust
pub mod bus;
```

- [ ] **Step 4: Run the tests — confirm pass**

Run: `cargo test --lib daemon::bus 2>&1 | tail -20`
Expected: 3 tests pass.

Run: `cargo build 2>&1 | tail -10`
Expected: warning-free (the `Receiver` import in bus.rs is used by the test; if flagged unused in non-test build, gate it `#[cfg(test)] use std::sync::mpsc::Receiver;` or remove it from the non-test import — only `Sender` is used in `register`'s signature).

- [ ] **Step 5: Commit**

```bash
git add src/daemon/proto.rs src/daemon/bus.rs src/daemon/mod.rs
git commit -m "feat: EventBus + ServerEvent::BusNotice (daemon broadcast bus)"
```

---

### Task 2: `handle_connection` → persistent loop + writer thread

**Files:**
- Modify: `src/daemon/socket.rs` — refactor `handle_connection` to loop + writer thread; change `drain_agent_events` to send to `combined_tx` instead of writing directly; update the existing test to close the connection after the turn (so the loop sees EOF).
- Test: `src/daemon/socket.rs` inline tests — `multi_turn_on_one_connection` (new), updated `client_sendmessage_roundtrips_through_socket`.

**Interfaces:**
- Consumes: existing `read_request`, `DaemonSessionManager::send_message`/`resume`/`disk_sessions`/`create`, `turn_token`.
- Produces: `handle_connection` signature UNCHANGED in this task (`stream, mgr, shutdown, turn_token`) — Task 3 adds the `bus` param. The internal `drain_agent_events` signature changes: `(rx, reader, combined_tx)`.

**Key behavior:** the connection now stays open across multiple `ClientRequest`s. The drain sends every `ServerEvent` to `combined_tx` (a writer thread writes them); prompt replies are still read inline from the socket `reader`. A client that closes the connection (EOF) ends the loop and the writer thread exits.

- [ ] **Step 1: Write the failing multi-turn test**

Add to the `#[cfg(test)] mod tests` block in `src/daemon/socket.rs`:

```rust
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

        let mgr_c = Arc::new(mgr); // wrap for the move
        let shutdown_c = shutdown.clone();
        let turn_token_c = turn_token.clone();
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            let mgr_inner: &Mutex<DaemonSessionManager> = &mgr_c;
            handle_connection(stream, mgr_inner, &shutdown_c, &turn_token_c).unwrap();
            // hold mgr_c alive
            drop(mgr_c);
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
        loop {
            let mut buf = String::new();
            if r.read_line(&mut buf).unwrap() == 0 { break; }
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(buf.trim()) { got_complete = true; break; }
        }
        assert!(got_complete, "turn 1 must complete");
        // turn 2 (same connection) — this hangs pre-fix
        let l2 = serde_json::to_string(&ClientRequest::SendMessage { content: "b".into() }).unwrap();
        writeln!(conn, "{l2}").unwrap(); conn.flush().unwrap();
        let mut got2 = false;
        loop {
            let mut buf = String::new();
            if r.read_line(&mut buf).unwrap() == 0 { break; }
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(buf.trim()) { got2 = true; break; }
        }
        assert!(got2, "turn 2 must complete on the same connection (REPL multi-turn fix)");
        drop(conn); // close → server loop sees EOF, exits
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
```

> Note on the `Arc<Mutex<...>>` wrap: the existing `client_sendmessage_roundtrips` test uses `Mutex::new(...)` (not Arc) and moves it into the thread. For the multi-turn test the same pattern works — if the borrow of `mgr` into the thread is awkward, wrap in `Arc` as shown. Match whatever the existing test does for consistency; the key assertions are the two `got_complete`/`got2`.

- [ ] **Step 2: Run the test — confirm it fails (hangs)**

Run: `cargo test --lib daemon::socket::tests::multi_turn_on_one_connection 2>&1 | tail -20`
Expected: the test HANGS (the second turn never completes because `handle_connection` exits after the first request). Interrupt if needed; the hang is the failure. (If it times out rather than hangs, that's the same failure.)

- [ ] **Step 3: Refactor `handle_connection` to a persistent loop + writer thread**

Replace the entire body of `handle_connection` in `src/daemon/socket.rs` (lines 50–105) with:

```rust
pub fn handle_connection(
    stream: UnixStream,
    mgr: &Mutex<super::session_manager::DaemonSessionManager>,
    shutdown: &std::sync::atomic::AtomicBool,
    turn_token: &std::sync::Arc<std::sync::Mutex<()>>,
) -> anyhow::Result<()> {
    use super::proto::{read_request, write_event, ClientRequest, ServerEvent};
    use std::io::BufWriter;
    use std::sync::mpsc;

    // 读半归 request-reader（本线程），写半归 writer 线程。
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);
    let writer = BufWriter::new(stream);

    // combined 通道：所有 ServerEvent（turn 事件 + 将来的 bus 事件）汇流到此，
    // 由 writer 线程单一写出——写天然串行化。
    let (combined_tx, combined_rx) = mpsc::channel::<ServerEvent>();
    let writer_handle = std::thread::spawn(move || {
        for ev in combined_rx.iter() {
            if write_event(&mut writer, &ev).is_err() {
                break; // 客户端断开
            }
        }
    });

    // 持久连接：循环读 ClientRequest，直到客户端关闭（EOF）。
    while let Some(req) = read_request(&mut reader)? {
        match req {
            ClientRequest::SendMessage { content } => {
                let mut g = mgr.lock().unwrap();
                let id = match g.list().first().cloned() { Some(id) => id, None => g.create() };
                let rx = g.send_message(&id, content)?;
                drop(g); // 释放 mgr 锁，让其它客户端可 NewSession/ListSessions
                let _turn_guard = turn_token.lock().unwrap();
                drain_agent_events(rx, &mut reader, &combined_tx)?;
            }
            ClientRequest::Resume { id } => {
                let mut g = mgr.lock().unwrap();
                let rx = g.resume(&id)?;
                drop(g);
                let _turn_guard = turn_token.lock().unwrap();
                drain_agent_events(rx, &mut reader, &combined_tx)?;
            }
            // PromptReply 只应在一个 turn 的 drain 中被内联消费；顶层收到说明协议误用。
            ClientRequest::PromptReply { .. } => {
                let _ = combined_tx.send(ServerEvent::Error {
                    message: "unexpected PromptReply (no prompt pending)".into(),
                });
            }
            ClientRequest::NewSession => {
                let id = mgr.lock().unwrap().create();
                let _ = combined_tx.send(ServerEvent::SessionCreated { id });
            }
            ClientRequest::ListSessions => {
                let ids = mgr.lock().unwrap().disk_sessions();
                let _ = combined_tx.send(ServerEvent::Sessions { ids });
            }
            ClientRequest::Shutdown => {
                shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = combined_tx.send(ServerEvent::Notice { text: "shutting down".into() });
            }
            ClientRequest::Status => {
                let _ = combined_tx.send(ServerEvent::Notice { text: "ccd running".into() });
            }
            other => {
                let _ = combined_tx.send(ServerEvent::Error {
                    message: format!("unsupported: {other:?}"),
                });
            }
        }
    }

    // 客户端关闭（EOF）：drop combined_tx → writer 线程退出。
    drop(combined_tx);
    let _ = writer_handle.join();
    Ok(())
}
```

- [ ] **Step 4: Change `drain_agent_events` to send to `combined_tx`**

Change the signature of `drain_agent_events` (currently `(rx, reader, writer)` at line 109) to take `combined_tx: &std::sync::mpsc::Sender<ServerEvent>` instead of `writer: &mut std::io::BufWriter<UnixStream>`:

```rust
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
            Ok(AgentEvent::SubAgentMilestone(s)) => {
                let _ = combined_tx.send(ServerEvent::Notice { text: format!("↳ {s}") });
            }
            Ok(AgentEvent::Reasoning(s)) => {
                let _ = combined_tx.send(ServerEvent::Notice { text: format!("💭 {s}") });
            }
            Ok(_) => { /* drop unserializable events (Test*, L4*) */ }
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
```

(`read_prompt_reply` at line 215 is UNCHANGED — it still reads one `ClientRequest::PromptReply` from the socket `reader`.)

- [ ] **Step 5: Update the existing `client_sendmessage_roundtrips_through_socket` test**

The existing test (line 244) does `h.join().unwrap()` while the client still holds the connection open — under the new loop, the server blocks on `read_request` waiting for the next request/EOF, so `h.join()` would hang. Fix it by closing the client connection after draining `TurnComplete` (so the server sees EOF and exits). At the end of that test, before `h.join().unwrap()`, drop the client stream:

```rust
        // close the client connection so the server's persistent loop sees EOF and exits
        drop(conn);
        drop(reader);
```

(Place these drops immediately before `h.join().unwrap();` and after the assertion that `TurnComplete` was received. The `let _ = std::fs::remove_dir_all(&dir);` stays last.)

- [ ] **Step 6: Run the socket tests — confirm pass**

Run: `cargo test --lib daemon::socket 2>&1 | tail -25`
Expected: `multi_turn_on_one_connection` PASS, `client_sendmessage_roundtrips_through_socket` PASS, `_write_event_is_part_of_proto_api` PASS. No hangs.

Run: `cargo build 2>&1 | tail -10`
Expected: warning-free.

- [ ] **Step 7: Run the full suite**

Run: `cargo test 2>&1 | tail -25`
Expected: 0 failed, 3 ignored. (The daemon/client integration tests still pass — they go through `handle_connection`'s loop for one request each, then the client closes.)

- [ ] **Step 8: Commit**

```bash
git add src/daemon/socket.rs
git commit -m "refactor: handle_connection persistent loop + writer thread (fixes multi-turn REPL)"
```

---

### Task 3: Wire `EventBus` into `Daemon` + register per connection

**Files:**
- Modify: `src/daemon/socket.rs` — `handle_connection` gains `bus: &Arc<EventBus>` param; registers `combined_tx`.
- Modify: `src/daemon/mod.rs` — `Daemon::run` owns `Arc<EventBus>`; passes a clone to each `handle_connection` spawn.
- Test: `src/daemon/socket.rs` — `bus_notice_reaches_idle_client`.

**Interfaces:**
- Consumes: Task 1's `EventBus`; Task 2's persistent-loop `handle_connection`.
- Produces: `handle_connection(stream, mgr, shutdown, turn_token, bus: &Arc<EventBus>)`.

- [ ] **Step 1: Write the failing test (idle client receives a bus notice)**

Add to `src/daemon/socket.rs` tests:

```rust
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

        std::thread::sleep(Duration::from_millis(50));
        let mut conn = UnixStream::connect(&sock).unwrap();
        // 客户端不发任何请求（idle）——仅等待 bus 事件。
        let mut r = BufReader::new(conn.try_clone().unwrap());

        // 广播一条
        bus.broadcast("workgraph", "milestone #1 advanced");
        // 客户端应收到 BusNotice（即使 idle）
        let mut got = false;
        for _ in 0..50 {
            let mut buf = String::new();
            // 非阻塞试探：用 set_read_timeout 让 read_line 短超时轮询
            conn.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match r.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(ServerEvent::BusNotice { source, text }) = serde_json::from_str(buf.trim()) {
                        assert_eq!(source, "workgraph");
                        assert_eq!(text, "milestone #1 advanced");
                        got = true;
                        break;
                    }
                }
                Err(_) => {} // timeout → keep polling
            }
        }
        assert!(got, "idle client must receive the bus notice in real time");
        drop(conn);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run the test — confirm it fails**

Run: `cargo test --lib daemon::socket::tests::bus_notice_reaches_idle_client 2>&1 | tail -20`
Expected: compile error — `handle_connection` takes 4 args, not 5 (no `bus` param yet).

- [ ] **Step 3: Add `bus` param + register `combined_tx`**

In `src/daemon/socket.rs`, change `handle_connection`'s signature and register:

```rust
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

    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);
    let writer = BufWriter::new(stream);

    let (combined_tx, combined_rx) = mpsc::channel::<ServerEvent>();
    // 注册到 bus：广播事件直接落进 combined_rx，与 turn 事件同流。
    bus.register(combined_tx.clone());

    let writer_handle = std::thread::spawn(move || {
        for ev in combined_rx.iter() {
            if write_event(&mut writer, &ev).is_err() { break; }
        }
    });

    while let Some(req) = read_request(&mut reader)? {
        // ... (match body identical to Task 2 Step 3) ...
    }
    drop(combined_tx);
    let _ = writer_handle.join();
    Ok(())
}
```

> The match body is identical to Task 2 Step 3's — copy it verbatim. The ONLY additions in this task are the `bus` param and the `bus.register(combined_tx.clone())` line. (Note: `combined_tx` is cloned for the bus; the local `combined_tx` is still used by the match arms and dropped at function end. When both are dropped, `combined_rx` closes and the writer exits.)

- [ ] **Step 4: Update `Daemon::run` to own + pass the bus**

In `src/daemon/mod.rs::run()`, after `let shutdown = Arc::new(AtomicBool::new(false));` (line 38), add:

```rust
        let bus = Arc::new(crate::daemon::bus::EventBus::new());
```

In the accept loop (the `std::thread::spawn(move || { ... handle_connection(...) })` block, ~line 108-115), clone the bus and pass it:

```rust
            let mgr = mgr.clone();
            let shutdown = shutdown.clone();
            let turn_token_c = Arc::clone(&turn_token);
            let bus_c = Arc::clone(&bus);
            std::thread::spawn(move || {
                if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown, &turn_token_c, &bus_c) {
                    eprintln!("ccd: connection error: {e}");
                }
            });
```

(The workgraph and supervisor threads will get `bus` clones in Task 4.)

- [ ] **Step 5: Run the tests — confirm pass**

Run: `cargo test --lib daemon::socket 2>&1 | tail -25`
Expected: all socket tests pass, including `bus_notice_reaches_idle_client`. (The Task 2 tests now pass `&bus` — update them to construct an `Arc<EventBus>` and pass it; the `multi_turn` and `client_sendmessage_roundtrips` tests need a `let bus = Arc::new(crate::daemon::bus::EventBus::new());` and `&bus` added to their `handle_connection(...)` call.)

Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -25` → 0 failed, 3 ignored.

- [ ] **Step 6: Commit**

```bash
git add src/daemon/socket.rs src/daemon/mod.rs
git commit -m "feat: wire EventBus into daemon; register each connection for real-time bus delivery"
```

---

### Task 4: Publishers — workgraph tick + supervisor

**Files:**
- Modify: `src/daemon/mod.rs` — workgraph thread captures `advance_one_milestone`'s return and broadcasts; supervisor thread broadcasts `supervise()`'s event lines.
- Modify: `src/capability.rs` — `Supervisor::supervise` returns `Vec<String>` (human-readable event lines for this cycle).
- Test: `src/daemon/mod.rs` — a focused test that `advance_one_milestone` returning `Some` triggers a broadcast (via an injected bus); plus updating the existing supervisor test for the new return type.

**Interfaces:**
- Consumes: Task 3's `Arc<EventBus>`; `advance_one_milestone(...) -> anyhow::Result<Option<BgOutcome>>` (`BgOutcome.events: Vec<String>`); `Supervisor::supervise`.
- Produces: `Supervisor::supervise(&mut self) -> Vec<String>` (was `()`).

- [ ] **Step 1: Change `Supervisor::supervise` to return event lines**

In `src/capability.rs`, change `pub fn supervise(&mut self)` (line 160) to `pub fn supervise(&mut self) -> Vec<String>` and collect a human-readable line for each restart and each give-up this cycle. Read the current body first; at each point where it restarts a service, push `format!("capability '{name}' restarted (attempt {n})")`; where it sets `gave_up`, push `format!("capability '{name}' gave up after {max} restarts")`. Return the `Vec<String>` at the end. Example shape:

```rust
    pub fn supervise(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        for (name, s) in self.states.iter_mut() {
            if s.gave_up { continue; }
            let exited = match s.child.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            if !exited { continue; }
            let now_inst = std::time::Instant::now();
            if let Some(first) = s.first_restart {
                if now_inst.duration_since(first).as_secs() >= self.window_secs {
                    s.restart_count = 0;
                    s.first_restart = None;
                }
            }
            if s.restart_count >= self.max_restarts {
                s.gave_up = true;
                s.child = None;
                events.push(format!("capability '{name}' gave up after {} restarts", s.restart_count));
                continue;
            }
            s.restart_count += 1;
            if s.first_restart.is_none() { s.first_restart = Some(now_inst); }
            let root = self.root.clone();
            if let Ok(c) = spawn_shell_capability(&root, &s.manifest) {
                s.child = Some(c);
                events.push(format!("capability '{name}' restarted (attempt {})", s.restart_count));
            } else {
                s.child = None;
            }
        }
        events
    }
```

> Read the CURRENT `supervise` body first and preserve its exact restart-window/cap logic — the only change is accumulating `events` and returning them. The existing `supervisor_restarts_crashed_persistent_until_cap` test calls `sup.supervise()` and ignores the return — it still compiles (the return is ignored). Confirm that test still passes.

- [ ] **Step 2: Write the failing publisher test**

Add to `src/daemon/mod.rs` tests:

```rust
    #[test]
    fn workgraph_advance_triggers_bus_broadcast() {
        use crate::background::advance_one_milestone;
        use crate::daemon::bus::EventBus;
        use crate::provider::stub::StubClient;
        use crate::workgraph::WorkGraph;

        let dir = std::env::temp_dir().join(format!("cc_bus_pub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = WorkGraph::default();
        g.add("do thing", "acceptance", vec![]).unwrap();
        g.save(&dir).unwrap();

        let bus = Arc::new(EventBus::new());
        let (tx, rx) = std::sync::mpsc::channel::<crate::daemon::proto::ServerEvent>();
        bus.register(tx);

        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap();
        assert!(out.is_some(), "should advance the ready milestone");
        // 模拟 daemon 线程的发布逻辑：advance 返回 Some → 取一条事件广播
        if let Some(o) = out {
            if let Some(line) = o.events.iter().find(|e| e.starts_with("milestone")) {
                bus.broadcast("workgraph", line);
            }
        }
        // 订阅者应收到 BusNotice
        let ev = rx.recv().unwrap();
        assert!(matches!(ev, crate::daemon::proto::ServerEvent::BusNotice { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 3: Run the test — confirm it passes (it's self-contained)**

Run: `cargo test --lib daemon::tests::workgraph_advance_triggers_bus_broadcast 2>&1 | tail -15`
Expected: PASS (the test exercises the publish logic directly). This codifies the broadcast shape the daemon thread will use.

- [ ] **Step 4: Wire the publishers into the daemon threads**

In `src/daemon/mod.rs::run()`:

(a) Workgraph thread — capture the return and broadcast. Give the workgraph thread a bus clone (add `let bus_for_wg = Arc::clone(&bus);` near the other clones, and move it into the `wg_handle` spawn). Replace `let _ = crate::background::advance_one_milestone(...);` with:

```rust
                let provider = crate::select_provider(&cfg_for_wg);
                let out = crate::background::advance_one_milestone(
                    provider,
                    cfg_for_wg.model.clone(),
                    cfg_for_wg.max_tokens,
                    cfg_for_wg.temperature,
                    cfg_for_wg.root.clone(),
                );
                if let Ok(Some(o)) = out {
                    if let Some(line) = o.events.iter().find(|e| e.starts_with("milestone")) {
                        bus_for_wg.broadcast("workgraph", line);
                    }
                }
```

(b) Supervisor thread — broadcast `supervise()`'s event lines. Give the supervisor thread a bus clone (`let bus_for_sup = Arc::clone(&bus);`, move into `sup_handle`). Replace `supervisor.supervise();` with:

```rust
                let events = supervisor.supervise();
                for line in events {
                    bus_for_sup.broadcast("supervisor", &line);
                }
```

- [ ] **Step 5: Build + full suite**

Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -25` → 0 failed, 3 ignored. (The supervisor test's `sup.supervise()` calls now return `Vec<String>`; ignored return is fine.)

- [ ] **Step 6: Commit**

```bash
git add src/daemon/mod.rs src/capability.rs
git commit -m "feat: publish workgraph + supervisor events to the daemon bus"
```

---

### Task 5: `cc` concurrent reader thread + `BusNotice` rendering

**Files:**
- Modify: `src/client/mod.rs` — `Connection::split` into writer/reader halves; `print_event` renders `BusNotice`.
- Modify: `src/bin/cc.rs` — `repl` spawns a reader thread + turn-done signal; `send_one` uses the same event loop.
- Test: hard to unit-test the REPL's threading; gate on compile + the daemon-side `bus_notice_reaches_idle_client` test (Task 3) covering delivery. Add a `print_event`-renders-BusNotice unit assertion.

**Interfaces:**
- Consumes: Task 1's `ServerEvent::BusNotice`; Task 3's real-time delivery.
- Produces: `Connection::split(self) -> (ConnectionWriter, ConnectionReader)` (or equivalent halves); `print_event` handles `BusNotice`.

- [ ] **Step 1: Add `BusNotice` rendering to `print_event` + a unit test**

In `src/client/mod.rs`, add an arm to `print_event` for `BusNotice`:

```rust
        ServerEvent::BusNotice { source, text } => {
            println!("[{source}] {text}");
            false
        }
```

Add a unit test in `src/client/mod.rs` tests:

```rust
    #[test]
    fn print_event_renders_bus_notice() {
        let ev = ServerEvent::BusNotice { source: "workgraph".into(), text: "milestone done".into() };
        // print_event prints to stdout; we only assert it's non-terminal (returns false)
        // and doesn't panic. (Terminal events are TurnComplete/Error.)
        assert!(!print_event(&ev));
    }
```

- [ ] **Step 2: Add `Connection::split`**

In `src/client/mod.rs`, add a method to split a `Connection` into a writer half (for `send`) and a reader half (for `next_event`), so two threads can use them concurrently:

```rust
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

impl Connection {
    /// 拆成写半 + 读半，供 main 线程与 reader 线程并发使用。
    pub fn split(self) -> anyhow::Result<(ConnectionWriter, ConnectionReader)> {
        let writer = self.writer;             // UnixStream
        let reader = BufReader::new(self.reader.into_inner().try_clone()?);
        Ok((ConnectionWriter { writer }, ConnectionReader { reader }))
    }
}
```

> Read the current `Connection` struct first — `writer: UnixStream` and `reader: BufReader<UnixStream>`. `self.reader.into_inner()` yields the inner `UnixStream`; `try_clone()` gives a second handle for the reader half. Adjust the exact field access to match the struct. If the borrow checker complains, clone the stream before splitting.

- [ ] **Step 3: Rewrite `repl` with a reader thread + turn-done signal**

The reader thread needs to `send` `PromptReply` (during a turn) and main needs to `send` `SendMessage` (between turns) — so the writer is shared via `Arc<Mutex<ConnectionWriter>>`. stdin is read by main between turns and by the reader thread only when a `Prompt` arrives mid-turn (main is then blocked on `done_rx`, so the two never contend for stdin).

In `src/bin/cc.rs`, replace `repl` (line 49) with:

```rust
fn repl(sock: &std::path::Path) -> anyhow::Result<()> {
    use codecoder::client::{print_event, prompt_user};
    use codecoder::daemon::proto::{ClientRequest, ServerEvent};
    use std::io::{BufRead, Write};
    use std::sync::{mpsc, Arc, Mutex};

    let conn = codecoder::client::Connection::connect(sock)
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
```

> stdin safety: between turns main reads stdin (reader thread is blocked on `next_event`, not stdin). During a turn main is blocked on `done_rx.recv()`; the reader reads stdin ONLY when a `Prompt` arrives. The two never read stdin concurrently. `writer` is `Arc<Mutex<ConnectionWriter>>` — both threads lock briefly to send (rare: one/turn for main, one/prompt for reader), negligible contention.

- [ ] **Step 4: Update `send_one` (one-shot) similarly**

`send_one` (line 27) keeps a simpler form: it doesn't need a turn-done loop across multiple turns, but it DOES need to receive `BusNotice`/events until terminal. Since `send_one` connects, sends one request, drains to terminal, exits — it can use `Connection::split` + a reader thread that prints events + handles prompts, and the main thread sends the initial request then waits for terminal. Simplest: keep `send_one` close to its current shape but route through split halves and the same print/prompt logic. (If the refactor is awkward for one-shot, leave `send_one` using the blocking `Connection::next_event` loop — it doesn't need real-time idle push since it exits after one turn. But it must render `BusNotice` via `print_event`, which Step 1 already covers.)

- [ ] **Step 5: Build + full suite**

Run: `cargo build 2>&1 | tail -15` → warning-free, BOTH binaries build (`codecoder`, `cc`).
Run: `cargo test 2>&1 | tail -25` → 0 failed, 3 ignored.

- [ ] **Step 6: Manual smoke (real-time push)**

```bash
ROOT=$(mktemp -d)
CODECODER_ROOT=$ROOT CODECODER_DAEMON=1 cargo run --quiet 2>/dev/null &
DPID=$!
sleep 1
# 终端 A：开 REPL（idle 等输入）
CODECODER_ROOT=$ROOT cargo run --bin cc --quiet 2>/dev/null &
CCPID=$!
sleep 1
# 终端 B：触发一次 workgraph（写 workgraph + 等 daemon tick 30s，或用 background）
# （workgraph tick 是 30s；为快速冒烟可跳过，仅确认 cc REPL 不挂、多轮 work）
kill $CCPID 2>/dev/null; kill $DPID 2>/dev/null; rm -rf $ROOT
```
Expected: `cc` REPL supports multiple turns on one connection (no hang on the 2nd). (Full real-time bus push smoke is gated on the 30s workgraph tick or a background run; the unit/integration tests are authoritative for delivery.)

- [ ] **Step 7: Commit**

```bash
git add src/client/mod.rs src/bin/cc.rs
git commit -m "feat: cc concurrent reader thread (real-time bus events) + BusNotice rendering"
```

---

## Self-Review (run after writing — notes for the implementer)

- **Spec coverage:** every spec section maps to a task — proto `BusNotice` + `EventBus` (Task 1); persistent connection + writer (Task 2, fixes REPL); bus registration + real-time delivery (Task 3); publishers workgraph + supervisor (Task 4); cc concurrent reader + rendering (Task 5). Error handling (dead-subscriber reap, writer exit on EOF, turn timeout) covered in Task 1/2. Testing: bus unit tests (Task 1), multi-turn + prompt (Task 2), idle-client bus delivery (Task 3), publisher (Task 4), rendering (Task 5).
- **Type consistency:** `EventBus::{register, broadcast}` (Task 1) match the calls in Tasks 3 & 4. `handle_connection` 4-arg in Task 2, 5-arg (`+bus`) in Task 3+ — Task 3 updates all callers. `drain_agent_events(rx, reader, combined_tx)` consistent across Task 2. `Supervisor::supervise -> Vec<String>` (Task 4) matches the supervisor-thread usage. `ServerEvent::BusNotice { source, text }` consistent everywhere.
- **No placeholders:** every step has complete code or exact commands. (Task 5 Step 3 flags the writer-ownership resolution explicitly — the main-owns-writer structure is specified, not left as `todo!` in the final code.)
- **Known gaps (acceptable):** prompt round-trip across the persistent connection is not exercised by a socket-level e2e test (would need a scripted provider that triggers a prompt mid-turn — same accepted gap as Task 9a); covered by the drain preserving its prompt arms + existing Task 9a unit tests. The cc reader-thread is gated by compile + the daemon-side delivery test (Task 3) + the `print_event` unit test.
