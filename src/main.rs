// CodeCoder — 入口分发 shim。三条路径（ADR 0016/0026 + 本计划）：
//   1. CODECODER_BG_TASK=<task>  → headless background runner（无 TUI，无 daemon）
//   2. CODECODER_DAEMON=1        → ccd daemon（无 TUI）
//   3. 其它                       → 默认 TUI
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    if std::env::var("CODECODER_DAEMON").is_ok() {
        return codecoder::run_daemon(cfg);
    }
    codecoder::run_tui(cfg)
}
