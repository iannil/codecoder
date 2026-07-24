use crate::daemon::proto::ServerEvent;
use crate::visual::event_router::EventRouter;
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

    /// Serve requests in a blocking loop. Run on a dedicated thread.
    pub fn serve(&self) {
        for request in self.server.incoming_requests() {
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
                    self.serve_static(request, &self.static_dir, "index.html");
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

        let mut writer = BufWriter::new(request.into_writer());

        // Write SSE headers
        for (name, val) in SSE_HEADERS {
            let _ = writeln!(&mut writer, "{name}: {val}");
        }
        let _ = writeln!(&mut writer); // blank line after headers

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

        let mut writer = BufWriter::new(request.into_writer());

        // Write SSE headers manually
        for (name, val) in SSE_HEADERS {
            let _ = writeln!(&mut writer, "{name}: {val}");
        }
        let _ = writeln!(&mut writer); // blank line after headers

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
