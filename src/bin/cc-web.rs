use codecoder::daemon::proto::ServerEvent;
use codecoder::visual::event_router::EventRouter;
use codecoder::visual::file_watcher::FileWatcher;
use codecoder::visual::http_server::HttpServer;
use codecoder::visual::socket_client::SocketClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
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

    // Set up file watcher (Phase 2: full implementation with poll loop)
    let mut file_watcher = match FileWatcher::new(&root, router.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("cc-web: file watcher init failed (non-fatal): {e}");
            FileWatcher::dummy()
        }
    };

    // Spawn a polling thread for file watcher
    thread::spawn(move || {
        loop {
            file_watcher.poll();
            thread::sleep(Duration::from_millis(100));
        }
    });

    // Start HTTP server
    let static_dir = if cfg!(debug_assertions) {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("static");
        p
    } else {
        let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        p.pop(); // remove binary name
        p.push("static");
        p
    };
    let http = HttpServer::new(
        port,
        router.clone(),
        &static_dir.to_string_lossy(),
        Some(root.clone()),
    )
    .unwrap_or_else(|e| {
            eprintln!("cc-web: failed to start HTTP server: {e}");
            std::process::exit(1);
        });

    // Graceful shutdown: SIGINT/SIGTERM → shutdown flag → accept loop unblocks and
    // serve() returns cleanly (mirrors the daemon, ADR 0026/0032). No SIGKILL needed.
    let shutdown = Arc::new(AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown)) {
        eprintln!("cc-web: SIGINT handler not registered: {e}");
    }
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown)) {
        eprintln!("cc-web: SIGTERM handler not registered: {e}");
    }

    println!("cc-web: listening on http://localhost:{port}");
    println!("cc-web: press Ctrl+C to stop");

    // Serve HTTP (blocks until shutdown flag is set).
    Arc::new(http).serve(Arc::clone(&shutdown));

    println!("cc-web: shut down gracefully");
}