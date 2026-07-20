// CodeCoder — 入口分发 shim。两条路径（ADR 0016/0026 + client-server migration）：
//   1. CODECODER_BG_TASK=<task>  → headless background runner（无 daemon，无 TUI）
//   2. 其它                       → ccd daemon（client-server 架构，无 TUI）
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    codecoder::run_daemon(cfg)
}
