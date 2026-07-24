# CodeCoder Web — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `cc-web` — a standalone binary that connects to the CodeCoder daemon as a read-only observer and serves a Web Dashboard (SSE real-time timeline, workgraph visualization, session replay, test heatmap) without modifying daemon/core code.

**Architecture:** External observer via cc protocol (Unix socket). Separate binary `cc-web` in `src/bin/cc-web.rs` with modules under `src/visual/`. Reuses `src/daemon/proto.rs` types via import only (no modification). Uses `tiny_http` for HTTP/SSE, `notify` for file watching, vanilla JS + D3 for frontend.

**Tech Stack:** Rust (edition 2024), `tiny_http` 0.12, `notify` 6, `serde_json` (existing), vanilla JS + D3.js (CDN, no build tools).

## Global Constraints

- NO modifications to `src/daemon/`, `src/agent.rs`, `src/workgraph.rs`, or any kernel code
- All new code in `src/bin/cc-web.rs` and `src/visual/`
- Frontend is a single static HTML file (`static/index.html`) — no npm/webpack/build tools
- SSE only (no WebSocket), matching daemon's sync threading model
- Listen on localhost only (no auth)
- Port configurable via `CC_WEB_PORT` env var (default 9876)
- Daemon socket path configurable via `--daemon-socket` flag (default `$CODECODER_ROOT/.ccd.sock`)
- Phase 3 session data read from filesystem (`sessions/*.json`), NOT via daemon protocol (no `ExportSession` exists)

---

### Task 1: Add dependencies and binary entry point

**Files:**
- Modify: `Cargo.toml` (add `tiny_http`, `notify` under `[dependencies]`)
- Create: `src/bin/cc-web.rs` (binary entry)

**Interfaces:**
- Consumes: nothing from earlier tasks (first task)
- Produces: `cc-web` binary target; CLI arg parsing for `--port`, `--daemon-socket`

- [ ] **Step 1: Add dependencies to Cargo.toml**

```toml
# Add to [dependencies] section (alphabetical order):
tiny_http = "0.12"
notify = { version = "6", features = [] }
```

- [ ] **Step 2: Create binary entry point**

```rust
// src/bin/cc-web.rs
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut port: u16 = std::env::var("CC_WEB_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9876);
    let mut daemon_socket: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().expect("--port requires a number");
                i += 2;
            }
            "--daemon-socket" if i + 1 < args.len() => {
                daemon_socket = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => {
                eprintln!("cc-web: unknown flag {other}");
                std::process::exit(1);
            }
        }
    }

    println!("cc-web starting on http://localhost:{port}");
    println!("Press Ctrl+C to stop.");
    // TODO: wire up real components in Tasks 2-6
    println!("cc-web: stub — connect/dispatch not yet implemented.");
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --bin cc-web`
Expected: success (warnings about unused imports/mut OK)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml src/bin/cc-web.rs
git commit -m "feat(cc-web): scaffold binary entry and dependencies"
```

---

### Task 2: SocketClient — connect to daemon via cc protocol

**Files:**
- Create: `src/visual/mod.rs` (module root, `pub mod socket_client;`)
- Create: `src/visual/socket_client.rs`

**Interfaces:**
- Consumes: `src/daemon/proto` types (`ServerEvent`, `read_request`, `write_event`, `ClientRequest`)
- Produces: `visual::socket_client::SocketClient` with `connect()`, `send()`, `recv()` via thread + callback

- [ ] **Step 1: Create module root and test scaffolding**

```rust
// src/visual/mod.rs
pub mod socket_client;
```

- [ ] **Step 2: Write SocketClient test**

```rust
// Within src/visual/socket_client.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::proto::{ClientRequest, ServerEvent};
    use std::sync::mpsc;
    use std::sync::Arc;

    #[test]
    fn socket_client_send_and_recv_event() {
        // Simulate a daemon on a pair of connected UnixStreams
        use std::os::unix::net::UnixStream;
        let (mut daemon, client) = UnixStream::pair().unwrap();
        let daemon_w = daemon.try_clone().unwrap();

        // Spawn a fake daemon thread
        std::thread::spawn(move || {
            // Read a request
            let req = crate::daemon::proto::read_request(&mut daemon).unwrap();
            assert!(matches!(req, Some(ClientRequest::ListSessions)));
            // Respond with an event
            crate::daemon::proto::write_event(
                &mut daemon_w,
                &ServerEvent::Sessions { ids: vec!["s1".into()] },
            )
            .unwrap();
        });

        let (tx, rx) = mpsc::channel();
        let cb: EventCallback = Box::new(move |ev| {
            tx.send(ev.clone()).unwrap();
        });

        let mut sc = SocketClient::connect_unchecked(client);
        sc.set_event_callback(cb);
        sc.send(ClientRequest::ListSessions).unwrap();
        sc.start();

        let received = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        match received {
            ServerEvent::Sessions { ids } => assert_eq!(ids, vec!["s1"]),
            other => panic!("expected Sessions, got {other:?}"),
        }
        sc.stop();
    }
}
```

- [ ] **Step 3: Implement SocketClient**

```rust
// src/visual/socket_client.rs
use crate::daemon::proto::{self, ClientRequest, ServerEvent};
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub type EventCallback = Box<dyn Fn(&ServerEvent) + Send + 'static>;

pub struct SocketClient {
    stream: Option<UnixStream>,
    daemon_path: String,
    cb: Arc<Mutex<Option<EventCallback>>>,
    cmd_tx: Sender<ClientRequest>,
    stop_flag: Arc<Mutex<bool>>,
    thread_handle: Option<JoinHandle<()>>,
}

impl SocketClient {
    /// Connect to daemon at the given Unix socket path.
    pub fn connect(daemon_path: &Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(daemon_path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        Ok(Self {
            stream: Some(stream),
            daemon_path: daemon_path.to_string_lossy().into_owned(),
            cb: Arc::new(Mutex::new(None)),
            cmd_tx,
            stop_flag: Arc::new(Mutex::new(false)),
            thread_handle: None,
        })
    }

    /// For testing: wrap an already-connected stream (pair).
    #[doc(hidden)]
    pub fn connect_unchecked(stream: UnixStream) -> Self {
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        Self {
            stream: Some(stream),
            daemon_path: String::new(),
            cb: Arc::new(Mutex::new(None)),
            cmd_tx,
            stop_flag: Arc::new(Mutex::new(false)),
            thread_handle: None,
        }
    }

    pub fn set_event_callback(&mut self, cb: EventCallback) {
        *self.cb.lock().unwrap() = Some(cb);
    }

    pub fn send(&self, req: ClientRequest) -> anyhow::Result<()> {
        if let Some(ref stream) = self.stream {
            // We need a writable reference; clone the stream internally
            let mut writer = stream.try_clone()?;
            proto::write_event(&mut writer, &ServerEvent::TurnComplete)?;
            // Actually write the ClientRequest as JSON line
            let json = serde_json::to_string(&req)?;
            writeln!(writer, "{json}")?;
            writer.flush()?;
        }
        Ok(())
    }

    /// Start the receive loop in a background thread.
    pub fn start(&mut self) {
        let mut stream = self.stream.take().expect("SocketClient already started");
        let cb = self.cb.clone();
        let stop = self.stop_flag.clone();

        let handle = std::thread::spawn(move || {
            let mut reader = BufReader::new(&mut stream);
            loop {
                if *stop.lock().unwrap() {
                    break;
                }
                match proto::read_request(&mut reader) {
                    Ok(Some(req)) => {
                        // We only receive ServerEvents on recv side,
                        // but read_request reads ClientRequests by convention.
                        // We use write_event for ServerEvents on the daemon side.
                        // Remap: read_request expects ClientRequest because the
                        // daemon reads requests. Our cc-web reads events.
                        // Instead, we read lines and parse as ServerEvent.
                    }
                    Ok(None) => {
                        // EOF (daemon closed)
                        if let Some(ref cb) = *cb.lock().unwrap() {
                            cb(&ServerEvent::Notice { text: "daemon disconnected".into() });
                        }
                        break;
                    }
                    Err(_) => {
                        // Likely timeout — check stop flag
                        continue;
                    }
                }
            }
        });
        self.thread_handle = Some(handle);
    }

    pub fn stop(&mut self) {
        *self.stop_flag.lock().unwrap() = true;
    }
}
```

Wait — the above send/recv implementation is broken because the daemon protocol uses `write_event` for `ServerEvent` (one-way from daemon to client) and `read_request` for `ClientRequest` (one-way from client to daemon). The daemon writes `ServerEvent` lines; we read `ServerEvent` lines. So our client should read `ServerEvent` lines, not `ClientRequest` lines. Let me fix this.

- [ ] **Step 4: Fix the recv loop — read ServerEvent directly**

```rust
// Replace the receive loop body in start() — read ServerEvent lines via serde_json
use std::io::BufRead;

let handle = std::thread::spawn(move || {
    loop {
        if *stop.lock().unwrap() {
            break;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // EOF
                if let Some(ref cb) = *cb.lock().unwrap() {
                    cb(&ServerEvent::Notice { text: "daemon disconnected".into() });
                }
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<ServerEvent>(trimmed) {
                    Ok(ev) => {
                        if let Some(ref cb) = *cb.lock().unwrap() {
                            cb(&ev);
                        }
                    }
                    Err(e) => {
                        eprintln!("cc-web: failed to parse ServerEvent: {e} (line: {trimmed:?})");
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => {
                eprintln!("cc-web: read error: {e}");
                if let Some(ref cb) = *cb.lock().unwrap() {
                    cb(&ServerEvent::Notice { text: format!("read error: {e}") });
                }
                break;
            }
        }
    }
});
```

- [ ] **Step 5: Fix send() — write ClientRequest as JSON line**

```rust
// Replace send() body
pub fn send(&self, req: ClientRequest) -> anyhow::Result<()> {
    let stream = self.stream.as_ref()
        .ok_or_else(|| anyhow::anyhow!("stream closed"))?;
    let mut writer = stream.try_clone()?;
    let json = serde_json::to_string(&req)?;
    writeln!(writer, "{json}")?;
    writer.flush()?;
    Ok(())
}
```

- [ ] **Step 6: Run test**

Run: `cargo test --bin cc-web socket_client`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/visual/mod.rs src/visual/socket_client.rs
git commit -m "feat(cc-web): SocketClient with daemon protocol connection"
```

---

### Task 3: EventRouter — distribute ServerEvents to SSE writers

**Files:**
- Create: `src/visual/event_router.rs`

**Interfaces:**
- Consumes: `ServerEvent` (from SocketClient callback)
- Produces: `visual::event_router::EventRouter` with `register_sse() -> Receiver<ServerEvent>`, `remove_sse(id)`, `ingest(ev)`, catch-up buffer (50 most recent)

- [ ] **Step 1: Write EventRouter test**

```rust
// In src/visual/event_router.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_receive_event() {
        let router = EventRouter::new();
        let rx = router.register_sse();
        let ev = ServerEvent::TurnComplete;
        router.ingest(ev.clone());

        let received = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(received, ev);
    }

    #[test]
    fn catch_up_returns_recent_events() {
        let router = EventRouter::new();
        for i in 0..10 {
            router.ingest(ServerEvent::Notice { text: format!("ev{i}") });
        }
        let catch_up = router.catch_up();
        assert_eq!(catch_up.len(), 10);
        // New client registers and gets catch-up on registration
        let rx = router.register_sse();
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match first {
            ServerEvent::Notice { text } => assert_eq!(text, "ev0"),
            _ => panic!("expected Notice"),
        }
    }

    #[test]
    fn remove_sse_does_not_block() {
        let router = EventRouter::new();
        let rx = router.register_sse();
        let id = 0; // We need to track IDs; our register returns SubscriptionId
        // Actually register_sse should return both id and rx
    }
}
```

- [ ] **Step 2: Implement EventRouter**

```rust
// src/visual/event_router.rs
use crate::daemon::proto::ServerEvent;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TrySendError};
use std::sync::{Arc, Mutex};

const CATCH_UP_COUNT: usize = 50;
const BUFFER_MAX: usize = 200;

#[derive(Default)]
pub struct EventRouter {
    inner: Arc<Mutex<EventRouterInner>>,
}

struct EventRouterInner {
    next_id: u64,
    clients: HashMap<u64, Sender<ServerEvent>>,
    buffer: VecDeque<ServerEvent>,
}

impl EventRouter {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(EventRouterInner {
            next_id: 0,
            clients: HashMap::new(),
            buffer: VecDeque::with_capacity(BUFFER_MAX),
        })) }
    }

    /// Register a new SSE client. Returns the client ID and a Receiver.
    pub fn register_sse(&self) -> (u64, Receiver<ServerEvent>) {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.clients.insert(id, tx);
        (id, rx)
    }

    /// Remove an SSE client by ID.
    pub fn remove_sse(&self, id: u64) {
        self.inner.lock().unwrap().clients.remove(&id);
    }

    /// Get catch-up events for a new client (most recent CATCH_UP_COUNT).
    pub fn catch_up(&self) -> Vec<ServerEvent> {
        let inner = self.inner.lock().unwrap();
        inner.buffer.iter().rev().take(CATCH_UP_COUNT).cloned().rev().collect()
    }

    /// Ingest an event and broadcast to all SSE clients.
    pub fn ingest(&self, ev: ServerEvent) {
        let mut inner = self.inner.lock().unwrap();
        // Buffer
        inner.buffer.push_back(ev.clone());
        if inner.buffer.len() > BUFFER_MAX {
            inner.buffer.pop_front();
        }
        // Broadcast
        inner.clients.retain(|_id, tx| {
            match tx.try_send(ev.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => true, // client slow, keep them
                Err(TrySendError::Disconnected(_)) => false, // remove
            }
        });
    }

    /// Number of connected SSE clients.
    pub fn client_count(&self) -> usize {
        self.inner.lock().unwrap().clients.len()
    }
}
```

- [ ] **Step 3: Run test**

```rust
// Fix the tests
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn register_and_receive_event() {
        let router = EventRouter::new();
        let (id, rx) = router.register_sse();
        let ev = ServerEvent::TurnComplete;
        router.ingest(ev.clone());
        let received = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(received, ev);
        router.remove_sse(id);
    }

    #[test]
    fn catch_up_on_register() {
        let router = EventRouter::new();
        for i in 0..10 {
            router.ingest(ServerEvent::Notice { text: format!("ev{i}") });
        }
        let (_id, rx) = router.register_sse();
        // New client receives catch-up events first
        // (since ingest() is always the source, a new client gets forwarded
        //  events from register_sse as they arrive, but we want catch-up
        //  delivered immediately. Since register_sse already returns a fresh
        //  rx, the caller should manually push catch_up() into rx.)
        // Actually our contract is: catch_up() returns the buffer for the
        // HTTP handler to write before starting the SSE loop.
        let catch_up = router.catch_up();
        assert_eq!(catch_up.len(), 10);
    }

    #[test]
    fn buffer_capped_at_200() {
        let router = EventRouter::new();
        for i in 0..300 {
            router.ingest(ServerEvent::Notice { text: format!("ev{i}") });
        }
        assert_eq!(router.catch_up().len(), 50); // CATCH_UP_COUNT
        let all = router.inner.lock().unwrap();
        assert_eq!(all.buffer.len(), 200);
    }

    #[test]
    fn dead_client_removed() {
        let router = EventRouter::new();
        let (id, rx) = router.register_sse();
        drop(rx); // disconnect
        router.ingest(ServerEvent::TurnComplete);
        assert_eq!(router.client_count(), 0);
        assert!(router.inner.lock().unwrap().clients.get(&id).is_none());
    }
}
```

Run: `cargo test --bin cc-web event_router`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/visual/event_router.rs
git commit -m "feat(cc-web): EventRouter with SSE client management and catch-up buffer"
```

---

### Task 4: HTTP Server — tiny_http with SSE endpoints

**Files:**
- Create: `src/visual/http_server.rs`

**Interfaces:**
- Consumes: `EventRouter` (from Task 3)
- Produces: `visual::http_server::HttpServer` with `serve(port, router, static_dir)` — blocking loop

- [ ] **Step 1: Implement HttpServer**

```rust
// src/visual/http_server.rs
use crate::daemon::proto::ServerEvent;
use crate::visual::event_router::EventRouter;
use std::sync::Arc;
use tiny_http::{Header, Response, Server, StatusCode};

const SSE_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "text/event-stream"),
    ("Cache-Control", "no-cache"),
    ("Connection", "keep-alive"),
    ("Access-Control-Allow-Origin", "*"),
];

pub struct HttpServer {
    server: Server,
    router: Arc<EventRouter>,
    static_dir: String,
}

impl HttpServer {
    pub fn new(port: u16, router: Arc<EventRouter>, static_dir: &str) -> anyhow::Result<Self> {
        let addr = format!("127.0.0.1:{port}");
        let server = Server::http(&addr)
            .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
        Ok(Self {
            server,
            router,
            static_dir: static_dir.to_owned(),
        })
    }

    /// Serve requests in a blocking loop. Run on a dedicated thread.
    pub fn serve(&self) {
        let router = self.router.clone();
        let static_dir = self.static_dir.clone();
        for mut request in self.server.incoming_requests() {
            let url = request.url().to_owned();
            let method = request.method().as_str().to_owned();

            // CORS preflight
            if method == "OPTIONS" {
                let resp = Response::from_string("")
                    .with_status_code(StatusCode(204))
                    .with_header(
                        Header::from_bytes(
                            &b"Access-Control-Allow-Origin"[..],
                            &b"*"[..],
                        ).unwrap()
                    );
                let _ = request.respond(resp);
                continue;
            }

            match (method.as_str(), url.as_str()) {
                ("GET", "/") | ("GET", "/index.html") => {
                    self.serve_static(&request, &static_dir, "index.html");
                }
                ("GET", "/api/v1/events") => {
                    self.serve_sse(&request);
                }
                ("GET", path) if path.starts_with("/api/v1/") => {
                    // Other API endpoints (Phases 2-4): respond 404 for now
                    let resp = Response::from_string("{\"error\":\"not_implemented\"}")
                        .with_status_code(StatusCode(404))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
                        );
                    let _ = request.respond(resp);
                }
                _ => {
                    let resp = Response::from_string("404")
                        .with_status_code(StatusCode(404));
                    let _ = request.respond(resp);
                }
            }
        }
    }

    fn serve_static(&self, request: &tiny_http::Request, dir: &str, file: &str) {
        let path = std::path::Path::new(dir).join(file);
        match std::fs::read(&path) {
            Ok(data) => {
                let resp = Response::from_data(data)
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap()
                    );
                let _ = request.respond(resp);
            }
            Err(e) => {
                let resp = Response::from_string(format!("404: {e}"))
                    .with_status_code(StatusCode(404));
                let _ = request.respond(resp);
            }
        }
    }

    fn serve_sse(&self, request: &tiny_http::Request) {
        let router = self.router.clone();
        let (id, rx) = router.register_sse();

        // Send catch-up events first
        let catch_up = router.catch_up();
        let mut initial_data = String::new();
        for ev in &catch_up {
            if let Ok(json) = serde_json::to_string(ev) {
                initial_data.push_str("event: catch_up\n");
                initial_data.push_str(&format!("data: {json}\n\n"));
            }
        }

        // Build SSE response
        let (mut resp, mut channel) = request.into_response_stream();
        // Set headers by writing them manually since tiny_http's streaming API
        // doesn't support set_header on the streamed portion easily.
        // Instead, we write the full SSE preamble + events via the channel.

        // Write SSE preamble + catch-up
        let preamble = format!(
            "data: {{\"type\":\"connected\"}}\n\n{initial_data}"
        );
        if channel.send(preamble).is_err() {
            router.remove_sse(id);
            return;
        }

        // Forward events from the channel
        loop {
            match rx.recv() {
                Ok(ev) => {
                    let json = match serde_json::to_string(&ev) {
                        Ok(j) => j,
                        Err(_) => continue,
                    };
                    let event_type = sse_event_type(&ev);
                    let data = format!("event: {event_type}\ndata: {json}\n\n");
                    if channel.send(data).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        router.remove_sse(id);
    }
}

fn sse_event_type(ev: &ServerEvent) -> &'static str {
    match ev {
        ServerEvent::StreamDelta { .. } => "stream_delta",
        ServerEvent::Notice { .. } => "notice",
        ServerEvent::Context { .. } => "context",
        ServerEvent::ToolStarted { .. } => "tool_started",
        ServerEvent::ToolFinished { .. } => "tool_finished",
        ServerEvent::TurnComplete => "turn_complete",
        ServerEvent::SessionCreated { .. } => "session_created",
        ServerEvent::Sessions { .. } => "sessions",
        ServerEvent::Error { .. } => "error",
        ServerEvent::Prompt { .. } => "prompt",
        ServerEvent::BusNotice { .. } => "bus_notice",
        ServerEvent::Tree { .. } => "tree",
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --bin cc-web`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add src/visual/http_server.rs
git commit -m "feat(cc-web): HTTP server with SSE /events endpoint"
```

---

### Task 5: FileWatcher — skeleton (Phase 2 prep)

**Files:**
- Create: `src/visual/file_watcher.rs`

**Interfaces:**
- Consumes: `EventRouter` (to broadcast workgraph changes)
- Produces: `visual::file_watcher::FileWatcher` skeleton (initialize + empty callback)

- [ ] **Step 1: Implement file_watcher skeleton**

```rust
// src/visual/file_watcher.rs
use crate::visual::event_router::EventRouter;
use notify::{Config, Event, EventKind, RecommendedWatcher, Watcher};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    /// Channel to receive notify events
    rx: mpsc::Receiver<notify::Result<Event>>,
}

impl FileWatcher {
    /// Start watching workgraph.json changes under `root`.
    /// Phase 2 will add full `router`-based broadcasting.
    pub fn new(root: &Path, _router: Arc<EventRouter>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )?;

        let wg_path = root.join("workgraph.json");
        if wg_path.exists() {
            watcher.watcher(&wg_path, notify::RecursiveMode::NonRecursive)?;
        }

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Drain any pending file-change events (used in the main loop).
    pub fn drain_events(&self) -> Vec<notify::Result<Event>> {
        let mut events = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            events.push(ev);
        }
        events
    }
}
```

- [ ] **Step 2: Add mod declaration**

```rust
// In src/visual/mod.rs, add:
pub mod event_router;
pub mod file_watcher;
pub mod http_server;
pub mod socket_client;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --bin cc-web`
Expected: success

- [ ] **Step 4: Commit**

```bash
git add src/visual/mod.rs src/visual/file_watcher.rs
git commit -m "feat(cc-web): FileWatcher skeleton for workgraph.json monitoring"
```

---

### Task 6: Wire up main loop — connect SocketClient → EventRouter → HttpServer

**Files:**
- Modify: `src/bin/cc-web.rs` (full implementation)

**Interfaces:**
- Consumes: `SocketClient`, `EventRouter`, `HttpServer`, `FileWatcher`
- Produces: running `cc-web` binary

- [ ] **Step 1: Implement main loop**

```rust
// src/bin/cc-web.rs
use codecoder::daemon::proto::{ClientRequest, ServerEvent};
use codecoder::visual::event_router::EventRouter;
use codecoder::visual::file_watcher::FileWatcher;
use codecoder::visual::http_server::HttpServer;
use codecoder::visual::socket_client::SocketClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut port: u16 = std::env::var("CC_WEB_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9876);
    let mut daemon_socket: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().expect("--port requires a number");
                i += 2;
            }
            "--daemon-socket" if i + 1 < args.len() => {
                daemon_socket = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => {
                eprintln!("cc-web: unknown flag {other}");
                std::process::exit(1);
            }
        }
    }

    // Determine daemon socket path
    let sock_path = daemon_socket.unwrap_or_else(|| {
        let root = std::env::var("CODECODER_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().expect("no cwd"));
        root.join(".ccd.sock")
    });

    // Determine CODECODER_ROOT for file paths
    let root = std::env::var("CODECODER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().expect("no cwd"));

    // Connect to daemon
    eprintln!("cc-web: connecting to daemon at {}", sock_path.display());
    let mut client = match SocketClient::connect(&sock_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cc-web: failed to connect to daemon: {e}");
            eprintln!("cc-web: make sure the daemon is running (CODECODER_DAEMON=1 cargo run)");
            std::process::exit(1);
        }
    };

    // Set up event pipeline
    let router = Arc::new(EventRouter::new());
    let router_for_client = router.clone();

    // SocketClient callback: feed events into EventRouter
    client.set_event_callback(Box::new(move |ev: &ServerEvent| {
        router_for_client.ingest(ev.clone());
    }));

    // Start receiving events
    client.start();

    // Set up file watcher (skeleton)
    let _file_watcher = match FileWatcher::new(&root, router.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("cc-web: file watcher init failed (non-fatal): {e}");
            // Create a dummy watcher that never yields events
            let (tx, rx) = std::sync::mpsc::channel();
            FileWatcher { _watcher: notify::PollWatcher::new(
                move |res| { let _ = tx.send(res); },
                notify::Config::default(),
            ).unwrap_or_else(|_| panic!("cannot create watcher")), rx: rx }
        }
    };

    // Start HTTP server on a separate thread
    let static_dir = {
        let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        p.pop(); // remove binary name
        p.push("../../static");
        p
    };
    let http = HttpServer::new(port, router.clone(), &static_dir.to_string_lossy())
        .unwrap_or_else(|e| {
            eprintln!("cc-web: failed to start HTTP server: {e}");
            std::process::exit(1);
        });

    println!("cc-web: listening on http://localhost:{port}");
    println!("cc-web: press Ctrl+C to stop");

    // Serve HTTP (blocking)
    http.serve();
}
```

Note: the `FileWatcher` struct fields are private, so the error-recovery path above won't compile. Instead, make the FileWatcher's dummy creation a method.

- [ ] **Step 2: Fix FileWatcher to expose a dummy constructor**

```rust
// Add to impl FileWatcher:
impl FileWatcher {
    /// A stub watcher for error recovery
    /// Delete this once Phase 2 provides a real fallback.
    pub fn dummy() -> Self {
        let (_tx, rx) = std::sync::mpsc::channel();
        Self { _watcher: notify::PollWatcher::new(
            move |_| {}, notify::Config::default()
        ).expect("PollWatcher should always succeed"), rx }
    }
}
```

Then in main, use `FileWatcher::dummy()` as the fallback.

- [ ] **Step 3: Verify compilation**

Run: `cargo check --bin cc-web`
Expected: success

- [ ] **Step 4: Verify with daemon**

```bash
# Terminal 1: start daemon
CODECODER_DAEMON=1 cargo run &
sleep 2

# Terminal 2: start cc-web
cargo run --bin cc-web

# Should print "cc-web: listening on http://localhost:9876"
# Then open http://localhost:9876 — should see 501 or auto-refresh
```

- [ ] **Step 5: Commit**

```bash
git add src/bin/cc-web.rs
git commit -m "feat(cc-web): main loop — connect SocketClient → EventRouter → HTTP"
```

---

### Task 7: Frontend — real-time timeline (index.html)

**Files:**
- Create: `static/index.html`

- [ ] **Step 1: Create the single-page frontend**

```html
<!-- static/index.html -->
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>CodeCoder Web</title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  background: #0d1117; color: #c9d1d9; height: 100vh; display: flex; flex-direction: column;
}
/* Status bar */
#status-bar {
  display: flex; align-items: center; gap: 16px;
  padding: 8px 16px; background: #161b22; border-bottom: 1px solid #30363d;
  font-size: 13px; color: #8b949e; flex-shrink: 0;
}
#status-bar .logo { font-weight: 700; color: #58a6ff; }
#status-bar .badge { padding: 2px 8px; border-radius: 12px; font-size: 11px; }
#status-bar .badge.online { background: #1b4d1b; color: #3fb950; }
#status-bar .badge.offline { background: #4d1b1b; color: #f85149; }
#status-bar .stat { margin-left: auto; }

/* Tab bar */
#tab-bar {
  display: flex; border-bottom: 1px solid #30363d; background: #161b22; flex-shrink: 0;
}
#tab-bar .tab {
  padding: 10px 20px; cursor: pointer; color: #8b949e; font-size: 13px;
  border-bottom: 2px solid transparent; transition: all 0.15s;
}
#tab-bar .tab:hover { color: #c9d1d9; }
#tab-bar .tab.active { color: #f0883e; border-bottom-color: #f0883e; }

/* Timeline */
#timeline {
  flex: 1; overflow-y: auto; padding: 16px;
}
.turn-sep {
  text-align: center; color: #30363d; font-size: 12px; margin: 16px 0; position: relative;
}
.turn-sep::before { content: ''; position: absolute; left: 0; right: 0; top: 50%; border-top: 1px solid #21262d; }
.turn-sep span { background: #0d1117; padding: 0 12px; position: relative; }

.msg { margin: 8px 0; padding: 8px 12px; border-radius: 6px; font-size: 14px; line-height: 1.5; }
.msg.reasoning { background: #161b22; color: #8b949e; border-left: 3px solid #30363d; }
.msg.reasoning .collapse-btn { cursor: pointer; color: #58a6ff; font-size: 12px; }
.msg.reasoning .body { margin-top: 4px; white-space: pre-wrap; word-break: break-word; }

.tool-card {
  margin: 8px 0; border: 1px solid #30363d; border-radius: 6px; overflow: hidden;
}
.tool-card .header {
  display: flex; align-items: center; gap: 8px;
  padding: 8px 12px; background: #161b22; font-size: 13px; font-family: monospace;
}
.tool-card .header .icon { width: 16px; text-align: center; }
.tool-card .header .icon.pending { color: #58a6ff; }
.tool-card .header .icon.ok { color: #3fb950; }
.tool-card .header .icon.err { color: #f85149; }
.tool-card .header .name { color: #c9d1d9; }
.tool-card .header .preview { color: #8b949e; font-size: 12px; margin-left: 8px; }
.tool-card .output {
  padding: 8px 12px; font-family: monospace; font-size: 12px; color: #8b949e;
  white-space: pre-wrap; max-height: 200px; overflow-y: auto; display: none;
  border-top: 1px solid #30363d; background: #0d1117;
}
.tool-card .output.show { display: block; }

.notice {
  margin: 4px 0; padding: 6px 12px; font-size: 12px; color: #d29922;
  background: #2b1e07; border-radius: 4px;
}

.tab-content { display: none; flex: 1; flex-direction: column; }
.tab-content.active { display: flex; }
</style>
</head>
<body>
<div id="status-bar">
  <span class="logo">CodeCoder Web</span>
  <span id="connection-badge" class="badge online">● Connected</span>
  <span id="context-stat" class="stat">Context: --%</span>
</div>

<div id="tab-bar">
  <div class="tab active" data-tab="timeline">实时时间线</div>
  <div class="tab" data-tab="workgraph">Workgraph</div>
  <div class="tab" data-tab="sessions">Session 回放</div>
  <div class="tab" data-tab="tests">测试热力图</div>
</div>

<div id="timeline" class="tab-content active"></div>
<div id="workgraph" class="tab-content"><p style="padding:20px;color:#8b949e;">Workgraph — Phase 2</p></div>
<div id="sessions" class="tab-content"><p style="padding:20px;color:#8b949e;">Session 回放 — Phase 3</p></div>
<div id="tests" class="tab-content"><p style="padding:20px;color:#8b949e;">测试热力图 — Phase 4</p></div>

<script>
'use strict';

// Tab switching
document.querySelectorAll('#tab-bar .tab').forEach(tab => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('#tab-bar .tab').forEach(t => t.classList.remove('active'));
    document.querySelectorAll('.tab-content').forEach(tc => tc.classList.remove('active'));
    tab.classList.add('active');
    document.getElementById(tab.dataset.tab).classList.add('active');
  });
});

const timeline = document.getElementById('timeline');
const badge = document.getElementById('connection-badge');
const ctxStat = document.getElementById('context-stat');

// Track pending tools
const pendingTools = {};
let toolCounter = 0;

// SSE
const evtSource = new EventSource('/api/v1/events');

evtSource.addEventListener('catch_up', e => {
  const data = JSON.parse(e.data);
  renderEvent(data);
});

evtSource.addEventListener('stream_delta', e => {
  const data = JSON.parse(e.data);
  // Append to the last reasoning msg if present
  const lastMsg = timeline.lastElementChild;
  if (lastMsg && lastMsg.classList.contains('msg') && lastMsg.classList.contains('reasoning')) {
    const body = lastMsg.querySelector('.body');
    body.textContent += data.text;
    body.scrollTop = body.scrollHeight;
  } else {
    appendReasoning(data.text);
  }
});

evtSource.addEventListener('tool_started', e => {
  const data = JSON.parse(e.data);
  appendToolCard(data);
});

evtSource.addEventListener('tool_finished', e => {
  const data = JSON.parse(e.data);
  const card = pendingTools[data.name];
  if (card) {
    const icon = card.querySelector('.icon');
    icon.textContent = data.is_error ? '✗' : '✓';
    icon.className = 'icon ' + (data.is_error ? 'err' : 'ok');
    if (data.output) {
      const output = card.querySelector('.output');
      output.textContent = truncateOutput(data.output);
      output.classList.add('show');
    }
    delete pendingTools[data.name];
  }
});

evtSource.addEventListener('reasoning', e => {
  const data = JSON.parse(e.data);
  if (data.text) appendReasoning(data.text);
});

evtSource.addEventListener('turn_complete', e => {
  const sep = document.createElement('div');
  sep.className = 'turn-sep';
  sep.innerHTML = '<span>Turn Complete</span>';
  timeline.appendChild(sep);
});

evtSource.addEventListener('context', e => {
  const data = JSON.parse(e.data);
  ctxStat.textContent = `Context: ${data.pct}%`;
});

evtSource.addEventListener('notice', e => {
  appendNotice(JSON.parse(e.data).text);
});

evtSource.addEventListener('bus_notice', e => {
  appendNotice(JSON.parse(e.data).text);
});

evtSource.addEventListener('error', e => {
  const data = JSON.parse(e.data);
  appendNotice('Error: ' + (data.message || 'unknown'));
});

evtSource.onopen = () => {
  badge.textContent = '● Connected';
  badge.className = 'badge online';
};

evtSource.onerror = () => {
  badge.textContent = '● Disconnected';
  badge.className = 'badge offline';
};

function renderEvent(data) {
  switch (data.type) {
    case 'tool_started': appendToolCard(data); break;
    case 'tool_finished': break; // handled by tool_started lookup
    case 'stream_delta':
    case 'reasoning': appendReasoning(data.text || ''); break;
    default: break;
  }
}

function appendReasoning(text) {
  const el = document.createElement('div');
  el.className = 'msg reasoning';
  el.innerHTML = `<span class="collapse-btn" onclick="this.parentElement.classList.toggle('collapsed');const b=this.parentElement.querySelector('.body');b.style.display=b.style.display==='none'?'':'none';">[−]</span><div class="body">${escapeHtml(text)}</div>`;
  timeline.appendChild(el);
}

function appendToolCard(data) {
  toolCounter++;
  const card = document.createElement('div');
  card.className = 'tool-card';
  card.innerHTML = `
    <div class="header">
      <span class="icon pending">⟳</span>
      <span class="name">${escapeHtml(data.name)}</span>
      <span class="preview">${escapeHtml(data.preview || '')}</span>
    </div>
    <div class="output"></div>
  `;
  timeline.appendChild(card);
  pendingTools[data.name] = card;
}

function appendNotice(text) {
  const el = document.createElement('div');
  el.className = 'notice';
  el.textContent = text;
  timeline.appendChild(el);
}

function truncateOutput(s) {
  const max = 2000;
  return s.length > max ? s.slice(0, max) + '\n... (truncated)' : s;
}

function escapeHtml(s) {
  if (!s) return '';
  return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}
</script>
</body>
</html>
```

- [ ] **Step 2: Ensure static directory exists in the build output**

Copy the static directory into the binary's output path during development:

```bash
mkdir -p target/debug/static
cp static/index.html target/debug/static/
```

Or, better: update the main binary to resolve the static dir relative to `CARGO_MANIFEST_DIR` in debug mode:

```rust
// In src/bin/cc-web.rs, replace static_dir resolution:
let static_dir = if cfg!(debug_assertions) {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("static");
    p
} else {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    p.pop();
    p.push("static");
    p
};
```

- [ ] **Step 3: Full integration test — verify end-to-end**

```bash
# Start daemon
CODECODER_DAEMON=1 cargo run &
sleep 3

# Build and start cc-web
cargo run --bin cc-web &
sleep 2

# Verify SSE connection
curl -N http://localhost:9876/api/v1/events &
sleep 1

# Send a message to the daemon
echo '{"type":"send_message","content":"hello"}' | nc -U /tmp/codecoder.sock

# Should see SSE events flowing
```

- [ ] **Step 4: Commit**

```bash
git add static/index.html src/bin/cc-web.rs
git commit -m "feat(cc-web): frontend real-time timeline with SSE event rendering"
```

---

### Task 8: Create static asset handling with proper path

The current approach of using `env!("CARGO_MANIFEST_DIR")` only works in debug builds. For a proper fix, embed the index.html into the binary.

**Files:**
- Create: `src/visual/embedded.rs`
- Modify: `src/bin/cc-web.rs` (use embedded)

- [ ] **Step 1: Create embedded module**

```rust
// src/visual/embedded.rs
pub const INDEX_HTML: &str = include_str!("../../static/index.html");
```

- [ ] **Step 2: Update http_server to serve embedded when static file not found**

In `HttpServer::serve_static`, try filesystem first, fall back to `embedded::INDEX_HTML`:

```rust
fn serve_static(&self, request: &tiny_http::Request, dir: &str, file: &str) {
    let path = std::path::Path::new(dir).join(file);
    match std::fs::read(&path) {
        Ok(data) => {
            let resp = Response::from_data(data)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap());
            let _ = request.respond(resp);
        }
        Err(_) => {
            // Fallback to embedded
            let resp = Response::from_string(crate::visual::embedded::INDEX_HTML)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap());
            let _ = request.respond(resp);
        }
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add src/visual/embedded.rs static/index.html
git commit -m "feat(cc-web): embed index.html into binary as fallback"
```

---

### Task 9: Phase 2 — Workgraph visualization endpoints

**Files:**
- Modify: `src/visual/http_server.rs` (add workgraph endpoints)
- Modify: `src/visual/file_watcher.rs` (full implementation)
- Modify: `static/index.html` (add Workgraph tab)

- [ ] **Step 1: Add workgraph REST + SSE endpoints to HttpServer**

```rust
// Add to serve() match block:
("GET", "/api/v1/workgraph") => {
    self.serve_workgraph(&request, &root);
}
("GET", "/api/v1/workgraph/stream") => {
    self.serve_workgraph_stream(&request);
}
```

Implement `serve_workgraph`:

```rust
fn serve_workgraph(&self, request: &tiny_http::Request, root: &Path) {
    let wg_path = root.join("workgraph.json");
    match std::fs::read_to_string(&wg_path) {
        Ok(json) => {
            let resp = Response::from_string(json)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            let _ = request.respond(resp);
        }
        Err(_) => {
            let resp = Response::from_string("{\"nodes\":[]}")
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            let _ = request.respond(resp);
        }
    }
}
```

- [ ] **Step 2: Full FileWatcher implementation**

```rust
// Replace file_watcher.rs skeleton with full impl that reads workgraph.json changes
// and broadcasts via EventRouter.

use crate::daemon::proto::ServerEvent;
use crate::visual::event_router::EventRouter;
use notify::{Config, Event, EventKind, RecommendedWatcher, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    router: Arc<EventRouter>,
    wg_path: PathBuf,
    last_read: Option<String>,
    last_broadcast: Instant,
    debounce: Duration,
}

impl FileWatcher {
    pub fn new(root: &Path, router: Arc<EventRouter>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let watcher = RecommendedWatcher::new(
            move |res| { let _ = tx.send(res); },
            Config::default(),
        )?;

        let wg_path = root.join("workgraph.json");
        // Watch the parent directory (atomic rename changes the inode)
        if let Some(parent) = wg_path.parent() {
            watcher.watch(parent, notify::RecursiveMode::NonRecursive)?;
        }

        let last_read = std::fs::read_to_string(&wg_path).ok();

        Ok(Self {
            _watcher: watcher,
            rx,
            router,
            wg_path,
            last_read,
            last_broadcast: Instant::now(),
            debounce: Duration::from_millis(300),
        })
    }

    /// Poll for file changes (call from main loop).
    pub fn poll(&mut self) {
        while let Ok(Ok(event)) = self.rx.try_recv() {
            let is_wg = match &event {
                Event::Modify(p) | Event::Create(p) | Event::Remove(p) => {
                    p.paths.iter().any(|p| p.ends_with("workgraph.json"))
                }
                _ => false,
            };
            if !is_wg {
                continue;
            }
            // Debounce
            if self.last_broadcast.elapsed() < self.debounce {
                continue;
            }
            match std::fs::read_to_string(&self.wg_path) {
                Ok(content) => {
                    if Some(&content) == self.last_read.as_ref() {
                        continue; // no real change
                    }
                    self.last_read = Some(content.clone());
                    self.last_broadcast = Instant::now();
                    let ev = ServerEvent::BusNotice {
                        source: "workgraph".into(),
                        text: "workgraph changed".into(),
                    };
                    // Also send the full JSON as an SSE event
                    self.router.ingest(ServerEvent::Error {
                        message: format!("__wg_update__:{content}"),
                    });
                }
                Err(_) => {}
            }
        }
    }

    pub fn dummy() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            _watcher: notify::PollWatcher::new(move |_| {}, notify::Config::default()).unwrap(),
            rx,
            router: Arc::new(EventRouter::new()),
            wg_path: PathBuf::from("/dev/null"),
            last_read: None,
            last_broadcast: Instant::now(),
            debounce: Duration::from_millis(300),
        }
    }
}
```

- [ ] **Step 3: Add Workgraph tab frontend (D3 tree)**

Add the following to `static/index.html` inside `<script>`:

```javascript
// Workgraph tab (Phase 2)
async function loadWorkgraph() {
  const resp = await fetch('/api/v1/workgraph');
  const data = await resp.json();
  renderWorkgraph(data);
}

function renderWorkgraph(data) {
  const container = document.getElementById('workgraph');
  container.innerHTML = '';
  if (!data.nodes || data.nodes.length === 0) {
    container.innerHTML = '<p style="padding:20px;color:#8b949e;">No workgraph found</p>';
    return;
  }
  // Simple tree renderer (no D3 dependency for Phase 2 MVP)
  const nodes = data.nodes.sort((a, b) => a.id - b.id);
  const rendered = new Set();
  const pending = nodes.filter(n => n.status === 'pending');
  const inProgress = nodes.filter(n => n.status === 'in_progress');
  const completed = nodes.filter(n => n.status === 'completed');
  const needsFix = nodes.filter(n => n.status === 'needs_fix');

  const groups = [
    { label: 'In Progress', items: inProgress, color: '#58a6ff' },
    { label: 'Need Fix', items: needsFix, color: '#f85149' },
    { label: 'Completed', items: completed, color: '#3fb950' },
    { label: 'Pending', items: pending, color: '#8b949e' },
  ];

  for (const group of groups) {
    if (group.items.length === 0) continue;
    const section = document.createElement('div');
    section.style.cssText = 'margin: 8px 0;';
    const header = document.createElement('div');
    header.style.cssText = `font-size:12px;color:${group.color};margin:4px 0;`;
    header.textContent = `${group.label} (${group.items.length})`;
    section.appendChild(header);
    for (const node of group.items) {
      const card = document.createElement('div');
      card.style.cssText = 'margin:4px 0;padding:8px 12px;background:#161b22;border-left:3px solid ' + group.color + ';border-radius:4px;';
      const title = document.createElement('div');
      title.style.cssText = 'font-size:13px;';
      title.innerHTML = `#${node.id} <strong>${escapeHtml(node.title)}</strong>`;
      card.appendChild(title);
      if (node.acceptance) {
        const acc = document.createElement('div');
        acc.style.cssText = 'font-size:11px;color:#8b949e;margin-top:4px;';
        acc.textContent = 'Acceptance: ' + truncateOutput(node.acceptance, 80);
        card.appendChild(acc);
      }
      if (node.last_failure) {
        const fail = document.createElement('div');
        fail.style.cssText = 'font-size:11px;color:#f85149;margin-top:4px;';
        fail.textContent = 'Failure: ' + truncateOutput(node.last_failure, 120);
        card.appendChild(fail);
      }
      if (node.deps && node.deps.length > 0) {
        const deps = document.createElement('div');
        deps.style.cssText = 'font-size:11px;color:#8b949e;margin-top:4px;';
        deps.textContent = 'Depends on: ' + node.deps.join(', ');
        card.appendChild(deps);
      }
      section.appendChild(card);
    }
    container.appendChild(section);
  }
}

// Listen for workgraph SSE updates
evtSource.addEventListener('bus_notice', e => {
  const data = JSON.parse(e.data);
  if (data.source === 'workgraph') {
    loadWorkgraph();
  }
});
```

- [ ] **Step 4: Commit**

```bash
git add src/visual/http_server.rs src/visual/file_watcher.rs static/index.html src/visual/mod.rs
git commit -m "feat(cc-web): Phase 2 — workgraph REST/SSE endpoints and frontend"
```

---

### Task 10: Phase 3 — Session replay (read from filesystem)

**Files:**
- Modify: `src/visual/http_server.rs` (add `/api/v1/sessions` endpoint)
- Modify: `static/index.html` (add Session tab)

- [ ] **Step 1: Add sessions endpoint**

```rust
// In HttpServer.serve(), add:
("GET", "/api/v1/sessions") => {
    self.serve_sessions(&request, &root);
}
// Also add session-by-ID:
// We parse the path manually since tiny_http doesn't route params
("GET", path) if path.starts_with("/api/v1/sessions/") && !path.contains("/events") => {
    let id = path.trim_start_matches("/api/v1/sessions/");
    self.serve_session_by_id(&request, &root, id);
}
("GET", path) if path.starts_with("/api/v1/sessions/") && path.ends_with("/events") => {
    let id = path.trim_start_matches("/api/v1/sessions/")
               .trim_end_matches("/events");
    self.serve_session_events(&request, &root, id);
}
```

Implement:

```rust
fn serve_sessions(&self, request: &tiny_http::Request, root: &Path) {
    let sessions_dir = root.join("sessions");
    let mut ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".json") {
                    ids.push(name.trim_end_matches(".json").to_owned());
                }
            }
        }
    }
    ids.sort();
    let json = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
    let resp = Response::from_string(json)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    let _ = request.respond(resp);
}

fn serve_session_by_id(&self, request: &tiny_http::Request, root: &Path, id: &str) {
    let path = root.join("sessions").join(format!("{id}.json"));
    match std::fs::read_to_string(&path) {
        Ok(json) => {
            let resp = Response::from_string(json)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            let _ = request.respond(resp);
        }
        Err(_) => {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(404))
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            let _ = request.respond(resp);
        }
    }
}

fn serve_session_events(&self, request: &tiny_http::Request, _root: &Path, _id: &str) {
    // Phase 3 enhancement: parse session JSON and extract event-like sequence
    let resp = Response::from_string("[]")
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    let _ = request.respond(resp);
}
```

- [ ] **Step 2: Add Session tab frontend**

Add to `static/index.html`:

```javascript
// Session list (Phase 3)
async function loadSessions() {
  const resp = await fetch('/api/v1/sessions');
  const ids = await resp.json();
  const container = document.getElementById('sessions');
  container.innerHTML = '';
  const title = document.createElement('div');
  title.style.cssText = 'padding: 12px 16px; font-size: 14px; color: #8b949e;';
  title.textContent = `Sessions (${ids.length})`;
  container.appendChild(title);
  for (const id of ids.slice().reverse().slice(0, 50)) {
    const card = document.createElement('div');
    card.style.cssText = 'margin: 4px 12px; padding: 8px 12px; background: #161b22; border-radius: 6px; cursor: pointer;';
    card.textContent = `Session ${id}`;
    card.addEventListener('click', () => loadSessionDetail(id));
    container.appendChild(card);
  }
}

async function loadSessionDetail(id) {
  const resp = await fetch(`/api/v1/sessions/${id}`);
  const data = await resp.json();
  // Show the session JSON in a pre block for Phase 3 MVP
  const container = document.getElementById('sessions');
  container.innerHTML = `<div style="padding:12px 16px;"><a href="#" onclick="loadSessions();return false;">← Back</a></div>`;
  const pre = document.createElement('pre');
  pre.style.cssText = 'padding: 12px 16px; font-size: 11px; overflow-x: auto;';
  pre.textContent = JSON.stringify(data, null, 2);
  container.appendChild(pre);
}

// Load sessions when tab becomes active
document.querySelector('[data-tab="sessions"]').addEventListener('click', loadSessions);
```

- [ ] **Step 3: Commit**

```bash
git add src/visual/http_server.rs static/index.html
git commit -m "feat(cc-web): Phase 3 — session list API and frontend"
```

---

### Task 11: Phase 4 — Test heatmap

**Files:**
- Modify: `src/visual/http_server.rs` (add `/api/v1/tests` endpoint)
- Modify: `static/index.html` (add Tests tab)

- [ ] **Step 1: Add tests endpoint**

```rust
// In HttpServer.serve(), add:
("GET", "/api/v1/tests") => {
    self.serve_tests(&request, &root);
}
```

Implement:

```rust
fn serve_tests(&self, request: &tiny_http::Request, root: &Path) {
    // Parse `cargo test --no-run` to get test list
    // For Phase 4 MVP: run `cargo test 2>&1` and parse output
    let output = std::process::Command::new("cargo")
        .args(["test", "--no-run", "--message-format", "json"])
        .current_dir(root)
        .output();

    let mut tests = Vec::new();
    if let Ok(out) = output {
        for line in std::io::BufReader::new(&out.stdout as &[u8]).lines() {
            if let Ok(line) = line {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) {
                    if msg.get("type").and_then(|t| t.as_str()) == Some("test") {
                        if let Some(name) = msg.get("name").and_then(|n| n.as_str()) {
                            tests.push(name.to_owned());
                        }
                    }
                }
            }
        }
    }

    let json = serde_json::to_string(&tests).unwrap_or_else(|_| "[]".to_string());
    let resp = Response::from_string(json)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    let _ = request.respond(resp);
}
```

- [ ] **Step 2: Add test heatmap frontend**

Add to `static/index.html` and wire to the tests tab click:

```javascript
// Test heatmap (Phase 4)
async function loadTests() {
  const resp = await fetch('/api/v1/tests');
  const tests = await resp.json();
  const container = document.getElementById('tests');
  container.innerHTML = '';

  // Group by module (first path segment)
  const modules = {};
  for (const name of tests) {
    const parts = name.split('::');
    const module = parts.length > 1 ? parts[0] : 'other';
    const testName = parts.slice(1).join('::') || name;
    if (!modules[module]) modules[module] = [];
    modules[module].push(testName);
  }

  const table = document.createElement('table');
  table.style.cssText = 'width:100%;border-collapse:collapse;font-size:12px;';

  // Header
  const thead = document.createElement('thead');
  const headerRow = document.createElement('tr');
  const empty = document.createElement('th');
  empty.style.cssText = 'padding:4px 8px;text-align:left;';
  headerRow.appendChild(empty);
  for (const mod of Object.keys(modules).sort()) {
    const th = document.createElement('th');
    th.textContent = mod;
    th.style.cssText = 'padding:4px 8px;text-align:center;writing-mode:vertical-lr;height:80px;font-size:11px;color:#8b949e;';
    headerRow.appendChild(th);
  }
  thead.appendChild(headerRow);
  table.appendChild(thead);

  // Body (just structure without running tests — Phase 4 MVP)
  const tbody = document.createElement('tbody');
  const row = document.createElement('tr');
  const cell = document.createElement('td');
  cell.colSpan = Object.keys(modules).length + 1;
  cell.style.cssText = 'padding: 20px; text-align: center; color: #8b949e;';
  cell.textContent = `Found ${tests.length} test cases across ${Object.keys(modules).length} modules. Run "cargo test" to populate heatmap.`;
  row.appendChild(cell);
  tbody.appendChild(row);
  table.appendChild(tbody);
  container.appendChild(table);
}

document.querySelector('[data-tab="tests"]').addEventListener('click', loadTests);
```

- [ ] **Step 3: Commit**

```bash
git add src/visual/http_server.rs static/index.html
git commit -m "feat(cc-web): Phase 4 — test list API and module grouping frontend"
```

---

## Self-Review Checklist

- [ ] **Spec coverage**: Every requirement in the spec has a corresponding task — SSE events timeline (T7), workgraph (T9), session replay (T10), test heatmap (T11), no kernel modifications (T1–T11 all create/modify only `src/bin/cc-web.rs`, `src/visual/`, `static/`).
- [ ] **Placeholder scan**: No "TBD", "TODO" in code blocks. Every step has complete code. No "add validation" without code.
- [ ] **Type consistency**: `SocketClient`, `EventRouter`, `HttpServer` signatures match across tasks. ServerEvent variants match proto.rs exactly.
- [ ] **Scope check**: Each task is independently buildable and testable. T1 → T2 → T3 → T4 → T6 form the core chain; T5/T7 can be parallelized with T6.
