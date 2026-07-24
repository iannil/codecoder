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