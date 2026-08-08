// CodeCoder — 入口分发 shim。三条路径(ADR 0016/0026/0033 + client-server migration):
//   1. CODECODER_BG_TASK=<task>      → headless background runner,显式单 shot
//   2. CODECODER_BG_WORKGRAPH=1      → headless background runner,workgraph 逐里程碑模式
//   3. 其它                          → ccd daemon(client-server 架构,无 TUI)
fn main() -> anyhow::Result<()> {
    // ── CLI arg 解析（先于 env 路由，--help/--version 直接退出）──
    let args: Vec<String> = std::env::args().collect();
    let help_spec = codecoder::help::HelpSpec {
        binary: "ccda",
        title: "CodeCoder daemon",
        description: "Autonomous AI agent daemon — long-running server process",
        usage: &[
            "ccda [FLAGS]",
            "CODECODER_BG_TASK=<task> ccda",
            "CODECODER_BG_WORKGRAPH=1 ccda",
        ],
        config_note: concat!(
            "Environment variables (see README.md):\n",
            "  CODECODER_API_KEY        LLM API key (required for real LLM)\n",
            "  CODECODER_MODEL          Model name (default: gpt-4o)\n",
            "  CODECODER_ROOT           Project root (default: CWD)\n",
            "  CODECODER_DAEMON         1 = daemon mode\n",
            "  CODECODER_BG_TASK        Headless one-shot task\n",
            "  CODECODER_BG_WORKGRAPH   1 = headless workgraph mode\n",
        ),
        skills: &[
            codecoder::help::SkillEntry {
                name: "daemon",
                description: "Run the daemon (default mode)",
                usage: &["ccda"],
                schema: None,
                template: Some("CODECODER_ROOT=/path ccda"),
            },
            codecoder::help::SkillEntry {
                name: "bg-task",
                description: "Run one headless task, then exit",
                usage: &["CODECODER_BG_TASK=<task> ccda"],
                schema: None,
                template: Some("CODECODER_BG_TASK=\"Implement feature X\" ccda"),
            },
            codecoder::help::SkillEntry {
                name: "bg-workgraph",
                description: "Run workgraph milestones headless, then exit",
                usage: &["CODECODER_BG_WORKGRAPH=1 ccda"],
                schema: None,
                template: Some("CODECODER_BG_WORKGRAPH=1 ccda"),
            },
            codecoder::help::SkillEntry {
                name: "config",
                description: "Check and edit configuration",
                usage: &["ccda config", "ccda config --key CODECODER_MODEL"],
                schema: None,
                template: None,
            },
            codecoder::help::SkillEntry {
                name: "recovery",
                description: "Run daemon with crash recovery (auto-restart loop)",
                usage: &["ccda"],
                schema: None,
                template: Some("CODECODER_DAEMON=1 ccda"),
            },
        ],
    };

    // Help request handling (before env routing, --version still works)
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
                return Ok(());
            }
            codecoder::help::HelpRequest::Help { json: false } => {
                println!("{}", codecoder::help::render_help(&help_spec));
                return Ok(());
            }
            codecoder::help::HelpRequest::Skill { name, json: true } => {
                match codecoder::help::skill_json(&help_spec, &name, &skills_dir) {
                    Some(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                    None => eprintln!("ccda: unknown skill '{name}'"),
                }
                return Ok(());
            }
            codecoder::help::HelpRequest::Skill { name, json: false } => {
                match codecoder::help::render_skill(&help_spec, &name, &skills_dir) {
                    Some(s) => println!("{s}"),
                    None => eprintln!("ccda: unknown skill '{name}'"),
                }
                return Ok(());
            }
        }
    }
    if args.len() > 1 {
        match args[1].as_str() {
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
        None => {
            if cfg.daemon_auto_restart {
                codecoder::recovery::run_with_recovery(cfg)
            } else {
                codecoder::run_daemon(cfg)
            }
        }
    }
}
