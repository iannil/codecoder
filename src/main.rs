// CodeCoder — autonomous AI agent. Entry shim; wiring lives in lib.rs.
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    // Background Agent (ADR 0026): CODECODER_BG_TASK=<task> runs headless, no TUI.
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    codecoder::run(cfg)
}
