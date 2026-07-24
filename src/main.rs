// CodeCoder — 入口分发 shim。三条路径(ADR 0016/0026/0033 + client-server migration):
//   1. CODECODER_BG_TASK=<task>      → headless background runner,显式单 shot
//   2. CODECODER_BG_WORKGRAPH=1      → headless background runner,workgraph 逐里程碑模式
//   3. 其它                          → ccd daemon(client-server 架构,无 TUI)
fn main() -> anyhow::Result<()> {
    codecoder::config::autoload_ccd_env();
    let cfg = codecoder::Config::from_env();
    match codecoder::bg_mode_from_env() {
        Some(codecoder::BgMode::Explicit(task)) => codecoder::run_background(cfg, task),
        Some(codecoder::BgMode::Workgraph) => codecoder::run_background(cfg, String::new()),
        None => codecoder::run_daemon(cfg),
    }
}
