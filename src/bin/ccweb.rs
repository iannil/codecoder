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

    let help_spec = codecoder::help::HelpSpec {
        binary: "ccweb",
        title: "CodeCoder web UI",
        description: "Web-based UI that connects to the ccd daemon",
        usage: &[
            "ccweb [FLAGS]",
            "ccweb --port 9876",
            "ccweb --daemon-socket /path/to/.ccd.sock",
        ],
        config_note: concat!(
            "Environment variables:\n",
            "  CC_WEB_PORT            HTTP port (default: 9876)\n",
            "  CODECODER_ROOT         Project root (default: CWD)\n",
        ),
        skills: &[
            codecoder::help::SkillEntry {
                name: "server",
                description: "Start the HTTP server",
                usage: &["ccweb", "ccweb --port 9876"],
                schema: None,
                template: Some("CC_WEB_PORT=9876 ccweb"),
            },
            codecoder::help::SkillEntry {
                name: "config",
                description: "Configure daemon socket path",
                usage: &["ccweb --daemon-socket /path/to/.ccd.sock"],
                schema: None,
                template: Some("ccweb --daemon-socket /tmp/ccd.sock"),
            },
        ],
    };

    // Help request handling (before arg parsing loop)
    if let Some(req) = codecoder::help::parse_help_request(&args[1..]) {
        let skills_dir = {
            let root = std::env::var("CODECODER_ROOT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::current_dir().expect("no cwd"));
            root.join("skills")
        };
        match req {
            codecoder::help::HelpRequest::Help { json: true } => {
                println!("{}", serde_json::to_string_pretty(&codecoder::help::help_json(&help_spec)).unwrap());
                return;
            }
            codecoder::help::HelpRequest::Help { json: false } => {
                println!("{}", codecoder::help::render_help(&help_spec));
                return;
            }
            codecoder::help::HelpRequest::Skills { json: true } => {
                println!("{}", serde_json::to_string_pretty(&codecoder::help::skills_json(&help_spec, &skills_dir)).unwrap());
                return;
            }
            codecoder::help::HelpRequest::Skills { json: false } => {
                println!("{}", codecoder::help::render_skills_list(&help_spec, &skills_dir));
                return;
            }
            codecoder::help::HelpRequest::Skill { name, json: true } => {
                match codecoder::help::skill_json(&help_spec, &name, &skills_dir) {
                    Some(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                    None => eprintln!("ccweb: unknown skill '{name}'"),
                }
                return;
            }
            codecoder::help::HelpRequest::Skill { name, json: false } => {
                match codecoder::help::render_skill(&help_spec, &name, &skills_dir) {
                    Some(s) => println!("{s}"),
                    None => eprintln!("ccweb: unknown skill '{name}'"),
                }
                return;
            }
        }
    }

    // --version/-v 与 ccda 对齐（在 arg 解析循环前拦截）
    for a in &args[1..] {
        if a == "--version" || a == "-v" {
            println!("CodeCoder {}", env!("CARGO_PKG_VERSION"));
            return;
        }
    }

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
                eprintln!("ccweb: unknown flag {other}");
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
    eprintln!("ccweb: connecting to daemon at {}", sock_path.display());
    let mut client = match SocketClient::connect(&sock_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ccweb: failed to connect to daemon: {e}");
            eprintln!("ccweb: make sure the daemon is running (cargo run --bin ccda)");
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
            eprintln!("ccweb: file watcher init failed (non-fatal): {e}");
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
            eprintln!("ccweb: failed to start HTTP server: {e}");
            std::process::exit(1);
        });

    // Graceful shutdown: SIGINT/SIGTERM → shutdown flag → accept loop unblocks and
    // serve() returns cleanly (mirrors the daemon, ADR 0026/0032). No SIGKILL needed.
    let shutdown = Arc::new(AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown)) {
        eprintln!("ccweb: SIGINT handler not registered: {e}");
    }
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown)) {
        eprintln!("ccweb: SIGTERM handler not registered: {e}");
    }

    println!("ccweb: listening on http://localhost:{port}");
    println!("ccweb: press Ctrl+C to stop");

    // Serve HTTP (blocks until shutdown flag is set).
    Arc::new(http).serve(Arc::clone(&shutdown));

    println!("ccweb: shut down gracefully");
}