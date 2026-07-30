use crate::daemon::proto::ServerEvent;
use crate::visual::event_router::EventRouter;
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use tiny_http::{Header, Response, Server, StatusCode};

const SSE_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "text/event-stream"),
    ("Cache-Control", "no-cache"),
    ("Connection", "keep-alive"),
    ("Access-Control-Allow-Origin", "*"),
];

const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Write a complete raw HTTP response head for an SSE stream.
///
/// `tiny_http`'s `into_writer()` returns the raw socket and writes nothing on its
/// own — no status line, no headers. For a streaming response we must emit the full
/// head ourselves (status line + headers + terminating blank line), all with CRLF
/// line endings per RFC 7230, before any SSE frame. Omitting it yields an invalid
/// HTTP response that browsers' EventSource rejects.
pub(crate) fn write_sse_head<W: Write>(writer: &mut W) -> std::io::Result<()> {
    write!(writer, "HTTP/1.1 200 OK\r\n")?;
    for (name, val) in SSE_HEADERS {
        write!(writer, "{name}: {val}\r\n")?;
    }
    write!(writer, "\r\n")
}

pub struct HttpServer {
    server: Server,
    router: Arc<EventRouter>,
    static_dir: String,
    root_path: PathBuf,
}

impl HttpServer {
    pub fn new(
        port: u16,
        router: Arc<EventRouter>,
        static_dir: &str,
        root_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let addr = format!("127.0.0.1:{port}");
        let server = Server::http(&addr)
            .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
        Ok(Self {
            server,
            router,
            static_dir: static_dir.to_owned(),
            root_path: root_path.unwrap_or_else(|| PathBuf::from(".")),
        })
    }

    /// Serve requests in a blocking loop until `shutdown` is set. Run on a
    /// dedicated thread (or the main thread — it blocks until shutdown).
    ///
    /// Each request is dispatched onto its own thread. This is essential because
    /// the SSE handlers (`/api/v1/events`, `/api/v1/workgraph/stream`) block for
    /// the lifetime of the connection; handling requests inline would let a single
    /// open SSE stream monopolize the server and starve every other request
    /// (index, REST endpoints, the second SSE stream) — freezing the whole UI.
    ///
    /// Graceful shutdown mirrors the daemon (ADR 0026/0032): a signal handler sets
    /// the shared `shutdown` flag, a monitor thread observes it and calls
    /// `Server::unblock()`, which makes `incoming_requests()` return so the accept
    /// loop exits cleanly instead of requiring SIGKILL.
    pub fn serve(self: Arc<Self>, shutdown: Arc<AtomicBool>) {
        // Monitor thread: when the shutdown flag is set, unblock the accept loop.
        let this = Arc::clone(&self);
        let shutdown_for_monitor = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            while !shutdown_for_monitor.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }
            this.server.unblock();
        });

        for request in self.server.incoming_requests() {
            // `unblock()` ends the iterator; guard against a racing spurious wake.
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let this = Arc::clone(&self);
            std::thread::spawn(move || this.handle(request));
        }
    }

    fn handle(&self, request: tiny_http::Request) {
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
            return;
        }

        match (method.as_str(), url.as_str()) {
                ("GET", "/") | ("GET", "/index.html") => {
                    self.serve_static(request, &self.static_dir, "index.html");
                }
                ("GET", "/trace.html") | ("GET", "/trace") => {
                    self.serve_static(request, &self.static_dir, "trace.html");
                }
                ("GET", "/api/v1/events") => {
                    self.serve_sse(request);
                }
                ("GET", "/api/v1/workgraph") => {
                    self.serve_workgraph(request);
                }
                ("GET", "/api/v1/workgraph/stream") => {
                    self.serve_workgraph_stream(request);
                }
                ("GET", "/api/v1/trace/stream") => {
                    self.serve_trace_stream(request);
                }
                ("GET", "/api/v1/trace/touches") => {
                    self.serve_trace_touches(request);
                }
                ("GET", "/api/v1/sessions") => {
                    self.serve_sessions(request, &self.root_path);
                }
                ("GET", "/api/v1/tests") => {
                    self.serve_tests(request, &self.root_path);
                }
                ("GET", path) if path.starts_with("/api/v1/sessions/") && !path.contains("/events") => {
                    let id = path.trim_start_matches("/api/v1/sessions/");
                    self.serve_session_by_id(request, &self.root_path, id);
                }
                ("GET", path) if path.starts_with("/api/v1/sessions/") && path.ends_with("/events") => {
                    let id = path.trim_start_matches("/api/v1/sessions/")
                               .trim_end_matches("/events");
                    self.serve_session_events(request, &self.root_path, id);
                }
                ("GET", "/api/v1/trace/agents") => {
                    self.serve_trace_agents(request);
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

    fn serve_workgraph(&self, request: tiny_http::Request) {
        let wg_path = self.root_path.join("workgraph.json");
        match std::fs::read_to_string(&wg_path) {
            Ok(json) => {
                let resp = Response::from_string(json)
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
                    );
                let _ = request.respond(resp);
            }
            Err(_) => {
                let resp = Response::from_string("{\"nodes\":[]}")
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
                    );
                let _ = request.respond(resp);
            }
        }
    }

    fn serve_workgraph_stream(&self, request: tiny_http::Request) {
        let router = self.router.clone();
        let (id, rx) = router.register_sse();

        // into_writer() hands back the raw socket; we must write the HTTP head.
        let mut writer = BufWriter::new(request.into_writer());
        if write_sse_head(&mut writer).is_err() {
            router.remove_sse(id);
            return;
        }

        // Read and emit current workgraph
        let wg_path = self.root_path.join("workgraph.json");
        let initial = match std::fs::read_to_string(&wg_path) {
            Ok(json) => json,
            Err(_) => "{\"nodes\":[]}".to_string(),
        };
        let _ = write!(&mut writer, "event: wg_update\n");
        let _ = write!(&mut writer, "data: {initial}\n\n");
        let _ = writer.flush();

        // Forward events with keepalive
        loop {
            match rx.recv_timeout(SSE_KEEPALIVE_INTERVAL) {
                Ok(ev) => {
                    let json = match serde_json::to_string(&ev) {
                        Ok(j) => j,
                        Err(_) => continue,
                    };
                    let ev_type = sse_event_type(&ev);
                    let _ = write!(&mut writer, "event: {ev_type}\n");
                    let _ = write!(&mut writer, "data: {json}\n\n");
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = write!(&mut writer, ": keepalive\n\n");
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Err(_) = writer.flush() {
                break;
            }
        }

        router.remove_sse(id);
    }

    fn serve_sessions(&self, request: tiny_http::Request, root: &Path) {
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

    fn serve_session_by_id(&self, request: tiny_http::Request, root: &Path, id: &str) {
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

    fn serve_session_events(&self, request: tiny_http::Request, _root: &Path, _id: &str) {
        // Phase 3 enhancement: parse session JSON and extract event-like sequence
        let resp = Response::from_string("[]")
            .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
        let _ = request.respond(resp);
    }

    fn serve_tests(&self, request: tiny_http::Request, root: &Path) {
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

    fn serve_static(&self, request: tiny_http::Request, dir: &str, file: &str) {
        let path = std::path::Path::new(dir).join(file);
        match std::fs::read(&path) {
            Ok(data) => {
                let resp = Response::from_data(data)
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap()
                    );
                let _ = request.respond(resp);
            }
            Err(_) => {
                // Fallback to embedded
                let resp = Response::from_string(crate::visual::embedded::INDEX_HTML)
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap()
                    );
                let _ = request.respond(resp);
            }
        }
    }

    fn serve_sse(&self, request: tiny_http::Request) {
        let router = self.router.clone();
        let (id, rx) = router.register_sse();

        // into_writer() hands back the raw socket; we must write the HTTP head.
        let mut writer = BufWriter::new(request.into_writer());
        if write_sse_head(&mut writer).is_err() {
            router.remove_sse(id);
            return;
        }

        // Catch-up events
        let catch_up = router.catch_up();
        for ev in &catch_up {
            if let Ok(json) = serde_json::to_string(ev) {
                let _ = write!(&mut writer, "event: catch_up\n");
                let _ = write!(&mut writer, "data: {json}\n\n");
            }
        }
        let _ = writer.flush();

        // Forward events from the channel with keepalive
        loop {
            match rx.recv_timeout(SSE_KEEPALIVE_INTERVAL) {
                Ok(ev) => {
                    let json = match serde_json::to_string(&ev) {
                        Ok(j) => j,
                        Err(_) => continue,
                    };
                    let ev_type = sse_event_type(&ev);
                    let _ = write!(&mut writer, "event: {ev_type}\n");
                    let _ = write!(&mut writer, "data: {json}\n\n");
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Send keepalive comment to keep connection alive
                    let _ = write!(&mut writer, ": keepalive\n\n");
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Err(_) = writer.flush() {
                break;
            }
        }

        router.remove_sse(id);
    }

    fn serve_trace_stream(&self, request: tiny_http::Request) {
        use std::sync::mpsc;

        let stream = crate::visual::trace_stream::TraceStream::new(&self.root_path);
        let rx = match stream.follow() {
            Ok(rx) => rx,
            Err(e) => {
                let resp = Response::from_string(format!("{{\"error\":\"failed to start trace stream: {e}\"}}"))
                    .with_status_code(StatusCode(500));
                let _ = request.respond(resp);
                return;
            }
        };

        let mut writer = std::io::BufWriter::new(request.into_writer());
        if write_sse_head(&mut writer).is_err() {
            return;
        }

        // Keepalive interval: 15s
        loop {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(line) => {
                    let _ = write!(&mut writer, "data: {line}\n\n");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = write!(&mut writer, ": keepalive\n\n");
                }
                Err(_) => break,
            }
            if let Err(_) = writer.flush() {
                break;
            }
        }
    }

    fn serve_trace_touches(&self, request: tiny_http::Request) {
        // Return file touch heatmap. For now, return an empty placeholder
        // since the actual SseObserver heatmap state lives in AgentLoop's ObserverSet.
        let body = "{}";
        let resp = Response::from_string(body)
            .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
        let _ = request.respond(resp);
    }

    fn serve_trace_agents(&self, request: tiny_http::Request) {
        use crate::trace::agent_graph::AgentGraph;
        use crate::trace::reader::TraceReader;

        let reader = TraceReader::from_root(&self.root_path);
        let graph = match AgentGraph::from_reader(&reader) {
            Ok(g) => g,
            Err(_) => AgentGraph::new(),
        };

        let rendered = graph.render_tree();
        let json = serde_json::json!({
            "nodes": graph.nodes,
            "edges": graph.edges,
            "tree": rendered,
        });
        let body = serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".to_string());
        let resp = Response::from_string(body)
            .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
        let _ = request.respond(resp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpStream;

    /// An SSE endpoint opened via `into_writer()` must emit a complete raw HTTP
    /// response head — status line + headers + blank line — before any SSE frame.
    /// `tiny_http`'s `into_writer()` hands back the raw socket and writes NOTHING
    /// on its own, so omitting the head produces an invalid response the browser's
    /// EventSource rejects (the "web won't start" symptom).
    #[test]
    fn sse_stream_emits_valid_http_response_head() {
        let router = Arc::new(EventRouter::new());
        // Bind an ephemeral port on a throwaway root (workgraph.json absent → fallback).
        let server = Arc::new(HttpServer::new(0, router, "static", Some(PathBuf::from("."))).unwrap());
        let addr = server.server.server_addr().to_ip().unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || server.serve(shutdown));

        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            stream,
            "GET /api/v1/workgraph/stream HTTP/1.1\r\nHost: localhost\r\n\r\n"
        )
        .unwrap();

        let mut buf = [0u8; 256];
        let n = stream.read(&mut buf).unwrap();
        let head = String::from_utf8_lossy(&buf[..n]);

        assert!(
            head.starts_with("HTTP/1.1 200"),
            "SSE response must begin with an HTTP status line, got:\n{head:?}"
        );
        assert!(
            head.to_ascii_lowercase().contains("content-type: text/event-stream"),
            "SSE response must declare the event-stream content type, got:\n{head:?}"
        );
    }

    /// A long-lived SSE stream must not block other requests. The server handles
    /// each request on its own thread, so an open `/api/v1/events` connection must
    /// not starve the index route (the frozen-UI symptom).
    #[test]
    fn open_sse_stream_does_not_block_other_requests() {
        let router = Arc::new(EventRouter::new());
        let server = Arc::new(HttpServer::new(0, router, "static", Some(PathBuf::from("."))).unwrap());
        let addr = server.server.server_addr().to_ip().unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || server.serve(shutdown));

        // Give the accept loop a moment to come up, then hold an SSE stream open.
        std::thread::sleep(Duration::from_millis(100));
        let mut sse = TcpStream::connect(addr).unwrap();
        write!(sse, "GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        // Read its head so the handler has entered its blocking forward loop.
        sse.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut b = [0u8; 64];
        let _ = sse.read(&mut b).unwrap();

        // A separate request must still be served promptly.
        let mut other = TcpStream::connect(addr).unwrap();
        other.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(other, "GET /api/v1/workgraph HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut buf = [0u8; 64];
        let n = other.read(&mut buf).unwrap();
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "index/REST must respond while an SSE stream is open, got:\n{head:?}"
        );
    }

    /// Setting the shutdown flag must unblock the accept loop so `serve()` returns
    /// — the basis for graceful shutdown on SIGINT/SIGTERM (no SIGKILL needed).
    #[test]
    fn shutdown_flag_stops_serve_loop() {
        let router = Arc::new(EventRouter::new());
        let server = Arc::new(HttpServer::new(0, router, "static", Some(PathBuf::from("."))).unwrap());
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || server.serve(shutdown_c));

        std::thread::sleep(Duration::from_millis(150));
        shutdown.store(true, Ordering::SeqCst);

        // The monitor polls every 100ms; give it a generous window to unblock.
        let deadline = Duration::from_secs(3);
        let start = std::time::Instant::now();
        while !handle.is_finished() && start.elapsed() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            handle.is_finished(),
            "serve() must return after the shutdown flag is set"
        );
        handle.join().unwrap();
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
        ServerEvent::Status(_) => "status",
        ServerEvent::Services(_) => "services",
        ServerEvent::WorkgraphStatus(_) => "workgraph_status",
        ServerEvent::AutotaskStatus(_) => "autotask_status",
        ServerEvent::HealthStatus(_) => "health_status",
        ServerEvent::TracePoint { .. } => "trace_point",
        ServerEvent::TraceSpan { .. } => "trace_span",
    }
}
