// CodeCoder — 入口分发 shim。三条路径(ADR 0016/0026/0033 + client-server migration):
//   1. CODECODER_BG_TASK=<task>      → headless background runner,显式单 shot
//   2. CODECODER_BG_WORKGRAPH=1      → headless background runner,workgraph 逐里程碑模式
//   3. 其它                          → ccd daemon(client-server 架构,无 TUI)
fn main() -> anyhow::Result<()> {
    codecoder::config::autoload_ccd_env();
    // ── CLI arg 解析（先于 env 路由，--help/--version 直接退出）──
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--help" | "-h" => {
                println!("CodeCoder — autonomous AI agent");
                println!();
                println!("USAGE:");
                println!("  {} [FLAGS]          Start daemon (default mode)", args[0]);
                println!("  {} --help            Show this help", args[0]);
                println!("  {} --version         Show version", args[0]);
                println!();
                println!("Modes (set via environment variable, mutually exclusive):");
                println!("  CODECODER_DAEMON=1           Run as daemon (default)");
                println!("  CODECODER_BG_TASK=<task>     Run one headless task, then exit");
                println!("  CODECODER_BG_WORKGRAPH=1     Run workgraph milestones headless, then exit");
                println!();
                println!("Configuration (env vars, see README.md for full table):");
                println!("  CODECODER_API_KEY        LLM API key (required for real LLM)");
                println!("  CODECODER_MODEL          Model name (default: gpt-4o)");
                println!("  CODECODER_ROOT           Project root (default: CWD)");
                println!("  CODECODER_DAEMON         1 = daemon mode");
                println!("  CODECODER_BG_TASK        Headless one-shot task");
                println!("  CODECODER_BG_WORKGRAPH   1 = headless workgraph mode");
                return Ok(());
            }
            "--version" | "-v" => {
                println!("CodeCoder {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {
                // 未知 flag → 仅警告，不阻止启动
                eprintln!("ccd: unknown flag '{}' (try --help)", args[1]);
            }
        }
    }
    let cfg = codecoder::Config::from_env();
    match codecoder::bg_mode_from_env() {
        Some(codecoder::BgMode::Explicit(task)) => codecoder::run_background(cfg, task),
        Some(codecoder::BgMode::Workgraph) => codecoder::run_background(cfg, String::new()),
        None => codecoder::run_daemon(cfg),
    }
}
