use crate::daemon::proto::ServerEvent;
use crate::visual::event_router::EventRouter;
use std::io::{BufWriter, Write};
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
