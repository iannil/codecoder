// Runtime configuration from CODECODER_* env vars (see README.md env table).
use std::path::PathBuf;

/// Serializes tests (across modules in this lib test binary) that read/mutate the
/// process-global `CODECODER_MAX_TOKENS_CEILING` env var.
pub(crate) static MAX_TOKENS_CEILING_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: Option<String>,
    pub model: String,
    pub api_base: String,
    pub max_tokens: u32,
    /// 自适应截断根治:命中 StopReason::Length 时,单 turn 有效 max_tokens 翻倍上调的封顶值
    /// (迭代 2)。env CODECODER_MAX_TOKENS_CEILING,默认 32768。
    pub max_tokens_ceiling: u32,
    /// no-op 探索兜底(迭代 4):单 turn 内连续多少个「纯探索」迭代后注入一次 steering nudge。
    /// 0 = 禁用。env CODECODER_NOOP_NUDGE_THRESHOLD,默认 3。
    pub noop_nudge_threshold: usize,
    pub temperature: f32,
    pub root: PathBuf,
    pub github_token: Option<String>,
    /// BG 护栏:每次 BG 调用最多推进的 milestone 数(spec 2026-07-22)。
    pub bg_max_auto: usize,
    /// BG 护栏:连续失败熔断阈值。
    pub bg_circuit_k: usize,
    /// BG 护栏:单 milestone turn 的工具迭代上限(< 全局 12)。
    pub bg_milestone_tool_cap: usize,
    /// BG 自恢复:单 milestone needs_fix 后最多自动重试次数(ADR 0026 迭代 1)。
    /// 0 = 禁用自恢复(回退到旧的一失败即停语义)。
    pub bg_max_fix_attempts: usize,
    /// Persistent Capability 跨重启崩溃预算(ADR 0034)。
    pub supervisor_crash_budget: u32,
    /// 工具输出(read_file / run_command)字节上限,超长截断带 marker(ADR 0037)。
    pub max_tool_output: usize,
    /// 命令超时秒数(0 = 无超时)。run_command 工具使用此值,也可被其 timeout_secs 参数覆盖。
    pub command_timeout_secs: u32,
    /// daemon workgraph 推进线程间隔（秒）。默认 30。
    pub wg_tick_secs: u64,
    /// Supervisor 监督线程间隔（秒）。默认 1。
    pub supervisor_tick_secs: u64,
    /// OnDemand capability 自动 reaper 延迟（秒）。默认 5。
    pub ondemand_reaper_secs: u64,
    /// 自动任务发现轮询间隔（秒）。0 = 禁用。env CODECODER_AUTOTASK_INTERVAL_SECS, 默认 300。
    pub auto_task_interval_secs: u64,
    /// 任务源类型。env CODECODER_AUTOTASK_SOURCE, 默认 "github_issues"。
    /// 未来可扩展: "github_issues", "webhook", "linear"。
    pub auto_task_source: String,
}

impl Config {
    pub fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Config {
            api_key: env("CODECODER_API_KEY"),
            model: env("CODECODER_MODEL").unwrap_or_else(|| "gpt-4o".into()),
            api_base: env("CODECODER_API_BASE")
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            max_tokens: env("CODECODER_MAX_TOKENS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(8192),
            max_tokens_ceiling: env("CODECODER_MAX_TOKENS_CEILING")
                .and_then(|v| v.parse().ok())
                .unwrap_or(32768),
            noop_nudge_threshold: env("CODECODER_NOOP_NUDGE_THRESHOLD")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            temperature: env("CODECODER_TEMPERATURE")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.7),
            root: env("CODECODER_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            github_token: env("GITHUB_TOKEN"),
            bg_max_auto: env("CODECODER_BG_MAX_AUTO")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            bg_circuit_k: env("CODECODER_BG_CIRCUIT_K")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            bg_milestone_tool_cap: env("CODECODER_BG_MILESTONE_TOOL_CAP")
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
            bg_max_fix_attempts: env("CODECODER_BG_MAX_FIX_ATTEMPTS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            supervisor_crash_budget: env("CODECODER_SUPERVISOR_CRASH_BUDGET")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            max_tool_output: env("CODECODER_MAX_TOOL_OUTPUT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(256 * 1024),
            command_timeout_secs: env("CODECODER_COMMAND_TIMEOUT_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            wg_tick_secs: env("CODECODER_WG_TICK_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            supervisor_tick_secs: env("CODECODER_SUPERVISOR_TICK_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            ondemand_reaper_secs: env("CODECODER_ONDEMAND_REAPER_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            auto_task_interval_secs: env("CODECODER_AUTOTASK_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            auto_task_source: env("CODECODER_AUTOTASK_SOURCE")
                .unwrap_or_else(|| "github_issues".into()),
        }
    }
}

/// 解析 dotenv 风格文本为 (key, value):跳过空行/`#` 注释/无 `=` 行;在首个 `=` 切分;
/// trim key 与 value;去 value 成对的单/双引号。
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue; };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        let mut val = v.trim();
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = &val[1..val.len() - 1];
        }
        out.push((key.to_string(), val.to_string()));
    }
    out
}

/// 允许从 `.ccd.env`(仓库本地、可能不可信)注入的键——只含无副作用的调参项。
/// 刻意排除:密钥/端点(API_KEY/API_BASE/GITHUB_TOKEN)、trust 门(DEFAULT_TRUST)、
/// 路径/根(ROOT)、执行模式(BG_TASK/BG_WORKGRAPH)、以及一切 loader/shell 敏感变量
/// (LD_*/DYLD_*/PATH/BASH_ENV/IFS/GIT_*)——这些必须来自用户真实 shell,绝不从仓库文件 source。
const DOTENV_ALLOWED_KEYS: &[&str] = &[
    "CODECODER_MODEL",
    "CODECODER_MAX_TOKENS",
    "CODECODER_MAX_TOKENS_CEILING",
    "CODECODER_TEMPERATURE",
    "CODECODER_MAX_TOOL_OUTPUT",
    "CODECODER_BG_MAX_AUTO",
    "CODECODER_BG_CIRCUIT_K",
    "CODECODER_BG_MILESTONE_TOOL_CAP",
    "CODECODER_BG_MAX_FIX_ATTEMPTS",
    "CODECODER_NOOP_NUDGE_THRESHOLD",
    "CODECODER_SUPERVISOR_CRASH_BUDGET",
    "CODECODER_COMMAND_TIMEOUT_SECS",
    "CODECODER_WG_TICK_SECS",
    "CODECODER_SUPERVISOR_TICK_SECS",
    "CODECODER_ONDEMAND_REAPER_SECS",
    "CODECODER_AUTOTASK_INTERVAL_SECS",
    "CODECODER_AUTOTASK_SOURCE",
];

/// 从 `path` 读 dotenv;仅对 (a) 在 `DOTENV_ALLOWED_KEYS` 白名单内且 (b) 进程 env 未设置的
/// key 执行 set_var(显式 env 优先)。安全边界在此:仓库本地文件可能不可信,故密钥/端点/
/// trust 门/loader/shell 变量一律拒绝注入(记入 rejected 并 eprintln 告警,让恶意文件可见)。
/// 文件不存在/读失败静默返回 0。返回实际注入的 key 数。
pub fn autoload_ccd_env_from(path: &std::path::Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else { return 0; };
    let mut injected: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for (k, v) in parse_dotenv(&text) {
        if !DOTENV_ALLOWED_KEYS.contains(&k.as_str()) {
            rejected.push(k);
            continue;
        }
        if std::env::var_os(&k).is_none() {
            unsafe { std::env::set_var(&k, &v); }
            injected.push(k);
        }
    }
    if !injected.is_empty() {
        eprintln!(
            "ccd: loaded {} key(s) from {}: {}",
            injected.len(),
            path.display(),
            injected.join(", ")
        );
    }
    if !rejected.is_empty() {
        eprintln!(
            "ccd: ignored {} non-allowlisted key(s) in {} (secrets/endpoints/trust/loader vars are never sourced from a repo file): {}",
            rejected.len(),
            path.display(),
            rejected.join(", ")
        );
    }
    injected.len()
}

/// 解析项目根(CODECODER_ROOT 或 CWD),自动加载 `<root>/.ccd.env`。入口在 Config::from_env() 之前调用。
pub fn autoload_ccd_env() -> usize {
    let root = std::env::var("CODECODER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    autoload_ccd_env_from(&root.join(".ccd.env"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_env_defaults_and_overrides() {
        unsafe {
            std::env::remove_var("CODECODER_BG_MAX_AUTO");
            std::env::remove_var("CODECODER_BG_CIRCUIT_K");
            std::env::remove_var("CODECODER_BG_MILESTONE_TOOL_CAP");
        }
        let c = Config::from_env();
        assert_eq!(c.bg_max_auto, 10);
        assert_eq!(c.bg_circuit_k, 2);
        assert_eq!(c.bg_milestone_tool_cap, 8);

        unsafe {
            std::env::set_var("CODECODER_BG_MAX_AUTO", "5");
            std::env::set_var("CODECODER_BG_CIRCUIT_K", "4");
            std::env::set_var("CODECODER_BG_MILESTONE_TOOL_CAP", "6");
        }
        let c2 = Config::from_env();
        assert_eq!(c2.bg_max_auto, 5);
        assert_eq!(c2.bg_circuit_k, 4);
        assert_eq!(c2.bg_milestone_tool_cap, 6);
        unsafe {
            std::env::remove_var("CODECODER_BG_MAX_AUTO");
            std::env::remove_var("CODECODER_BG_CIRCUIT_K");
            std::env::remove_var("CODECODER_BG_MILESTONE_TOOL_CAP");
        }
    }

    #[test]
    fn bg_max_fix_attempts_default_and_override() {
        unsafe { std::env::remove_var("CODECODER_BG_MAX_FIX_ATTEMPTS"); }
        assert_eq!(Config::from_env().bg_max_fix_attempts, 3);
        unsafe { std::env::set_var("CODECODER_BG_MAX_FIX_ATTEMPTS", "5"); }
        assert_eq!(Config::from_env().bg_max_fix_attempts, 5);
        unsafe { std::env::remove_var("CODECODER_BG_MAX_FIX_ATTEMPTS"); }
    }

    #[test]
    fn max_tokens_default_is_8192() {
        unsafe { std::env::remove_var("CODECODER_MAX_TOKENS"); }
        assert_eq!(Config::from_env().max_tokens, 8192);
    }

    #[test]
    fn max_tokens_ceiling_default_and_override() {
        let _g = MAX_TOKENS_CEILING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("CODECODER_MAX_TOKENS_CEILING"); }
        assert_eq!(Config::from_env().max_tokens_ceiling, 32768);
        unsafe { std::env::set_var("CODECODER_MAX_TOKENS_CEILING", "16384"); }
        assert_eq!(Config::from_env().max_tokens_ceiling, 16384);
        unsafe { std::env::remove_var("CODECODER_MAX_TOKENS_CEILING"); }
    }

    #[test]
    fn noop_nudge_threshold_default_and_override() {
        unsafe { std::env::remove_var("CODECODER_NOOP_NUDGE_THRESHOLD"); }
        assert_eq!(Config::from_env().noop_nudge_threshold, 3);
        unsafe { std::env::set_var("CODECODER_NOOP_NUDGE_THRESHOLD", "5"); }
        assert_eq!(Config::from_env().noop_nudge_threshold, 5);
        unsafe { std::env::remove_var("CODECODER_NOOP_NUDGE_THRESHOLD"); }
    }

    #[test]
    fn parse_dotenv_handles_comments_blank_quotes() {
        let text = "# comment\n\nFOO=bar\nBAZ = \"qux\"\nNOEQ\nA=b=c\n";
        let pairs = parse_dotenv(text);
        assert_eq!(pairs, vec![
            ("FOO".to_string(), "bar".to_string()),
            ("BAZ".to_string(), "qux".to_string()),   // trim + 去成对引号
            ("A".to_string(), "b=c".to_string()),      // 只在首个 = 切分
        ]);
    }

    #[test]
    fn autoload_ccd_env_from_injects_unset_not_override() {
        // 使用真实的白名单键。CODECODER_MAX_TOKENS_CEILING 由专用锁串行化(其他测试也读它),
        // 顺带覆盖 CODECODER_MODEL(注入)。注意:CODECODER_MODEL 无专用锁,与并行读写它的
        // 测试之间存在竞态窗口——此处 save/restore 尽力兜底,但不是完全无竞态。
        let _g = MAX_TOKENS_CEILING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prior_model = std::env::var_os("CODECODER_MODEL");
        let prior_ceiling = std::env::var_os("CODECODER_MAX_TOKENS_CEILING");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(".ccd.env");
        std::fs::write(
            &f,
            "CODECODER_MODEL=fromfile\nCODECODER_MAX_TOKENS_CEILING=file2\n",
        )
        .unwrap();
        unsafe {
            std::env::remove_var("CODECODER_MODEL"); // 未设置 → 应被注入
            std::env::set_var("CODECODER_MAX_TOKENS_CEILING", "explicit"); // 已设置 → 不覆盖
        }
        let n = autoload_ccd_env_from(&f);
        assert_eq!(std::env::var("CODECODER_MODEL").unwrap(), "fromfile"); // 未设置 → 注入
        assert_eq!(std::env::var("CODECODER_MAX_TOKENS_CEILING").unwrap(), "explicit"); // 已设置 → 不覆盖
        assert_eq!(n, 1, "only the unset key is injected");
        // 恢复先前值,避免污染其它测试。
        unsafe {
            match prior_model {
                Some(v) => std::env::set_var("CODECODER_MODEL", v),
                None => std::env::remove_var("CODECODER_MODEL"),
            }
            match prior_ceiling {
                Some(v) => std::env::set_var("CODECODER_MAX_TOKENS_CEILING", v),
                None => std::env::remove_var("CODECODER_MAX_TOKENS_CEILING"),
            }
        }
    }

    #[test]
    fn autoload_ccd_env_from_rejects_non_allowlisted_keys() {
        // 恶意 .ccd.env:loader 注入、凭证、trust 门、PATH 均须被拒;只有白名单键被注入。
        let _g = MAX_TOKENS_CEILING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prior_model = std::env::var_os("CODECODER_MODEL");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(".ccd.env");
        std::fs::write(
            &f,
            "LD_PRELOAD=/evil.so\nCODECODER_API_KEY=stolen\nCODECODER_DEFAULT_TRUST=always\nPATH=/evil\nCODECODER_MODEL=safe-model\n",
        )
        .unwrap();
        // 确保这些危险键在测试前都未设置(注入才有意义;PATH 通常已由 shell 设置,
        // 但即便如此它也在白名单之外,断言的是"不被本文件的值污染")。
        let prior_ld = std::env::var_os("LD_PRELOAD");
        let prior_apikey = std::env::var_os("CODECODER_API_KEY");
        let prior_trust = std::env::var_os("CODECODER_DEFAULT_TRUST");
        unsafe {
            std::env::remove_var("LD_PRELOAD");
            std::env::remove_var("CODECODER_API_KEY");
            std::env::remove_var("CODECODER_DEFAULT_TRUST");
            std::env::remove_var("CODECODER_MODEL");
        }
        let n = autoload_ccd_env_from(&f);
        // 四个危险键:注入前均未设置(PATH 除外——但白名单外,值绝不来自文件),故仍未设置。
        assert!(std::env::var_os("LD_PRELOAD").is_none(), "LD_PRELOAD must not be injected");
        assert!(std::env::var_os("CODECODER_API_KEY").is_none(), "API_KEY must not be injected");
        assert!(std::env::var_os("CODECODER_DEFAULT_TRUST").is_none(), "DEFAULT_TRUST must not be injected");
        assert_ne!(std::env::var("PATH").ok().as_deref(), Some("/evil"), "PATH must not be sourced from file");
        // 白名单键被注入。
        assert_eq!(std::env::var("CODECODER_MODEL").unwrap(), "safe-model");
        assert_eq!(n, 1, "only the single allowlisted key is injected");
        // 清理/恢复。
        unsafe {
            match prior_model {
                Some(v) => std::env::set_var("CODECODER_MODEL", v),
                None => std::env::remove_var("CODECODER_MODEL"),
            }
            match prior_ld {
                Some(v) => std::env::set_var("LD_PRELOAD", v),
                None => std::env::remove_var("LD_PRELOAD"),
            }
            match prior_apikey {
                Some(v) => std::env::set_var("CODECODER_API_KEY", v),
                None => std::env::remove_var("CODECODER_API_KEY"),
            }
            match prior_trust {
                Some(v) => std::env::set_var("CODECODER_DEFAULT_TRUST", v),
                None => std::env::remove_var("CODECODER_DEFAULT_TRUST"),
            }
        }
    }

    #[test]
    fn autoload_ccd_env_from_missing_file_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(autoload_ccd_env_from(&dir.path().join(".ccd.env")), 0);
    }

    #[test]
    fn supervisor_crash_budget_default_and_override() {
        unsafe { std::env::remove_var("CODECODER_SUPERVISOR_CRASH_BUDGET"); }
        assert_eq!(Config::from_env().supervisor_crash_budget, 3);
        unsafe { std::env::set_var("CODECODER_SUPERVISOR_CRASH_BUDGET", "5"); }
        assert_eq!(Config::from_env().supervisor_crash_budget, 5);
        unsafe { std::env::remove_var("CODECODER_SUPERVISOR_CRASH_BUDGET"); }
    }

    #[test]
    fn config_interval_defaults() {
        unsafe {
            std::env::remove_var("CODECODER_WG_TICK_SECS");
            std::env::remove_var("CODECODER_SUPERVISOR_TICK_SECS");
            std::env::remove_var("CODECODER_ONDEMAND_REAPER_SECS");
        }
        let cfg = Config::from_env();
        assert_eq!(cfg.wg_tick_secs, 30);
        assert_eq!(cfg.supervisor_tick_secs, 1);
        assert_eq!(cfg.ondemand_reaper_secs, 5);
    }

    #[test]
    fn config_interval_overrides() {
        unsafe {
            std::env::set_var("CODECODER_WG_TICK_SECS", "15");
            std::env::set_var("CODECODER_SUPERVISOR_TICK_SECS", "3");
            std::env::set_var("CODECODER_ONDEMAND_REAPER_SECS", "10");
        }
        let cfg = Config::from_env();
        assert_eq!(cfg.wg_tick_secs, 15);
        assert_eq!(cfg.supervisor_tick_secs, 3);
        assert_eq!(cfg.ondemand_reaper_secs, 10);
        unsafe {
            std::env::remove_var("CODECODER_WG_TICK_SECS");
            std::env::remove_var("CODECODER_SUPERVISOR_TICK_SECS");
            std::env::remove_var("CODECODER_ONDEMAND_REAPER_SECS");
        }
    }

    #[test]
    fn autotask_config_defaults() {
        unsafe {
            std::env::remove_var("CODECODER_AUTOTASK_INTERVAL_SECS");
            std::env::remove_var("CODECODER_AUTOTASK_SOURCE");
        }
        let cfg = Config::from_env();
        assert_eq!(cfg.auto_task_interval_secs, 300);
        assert_eq!(cfg.auto_task_source, "github_issues");
    }

    #[test]
    fn autotask_config_overrides() {
        unsafe {
            std::env::set_var("CODECODER_AUTOTASK_INTERVAL_SECS", "60");
            std::env::set_var("CODECODER_AUTOTASK_SOURCE", "webhook");
        }
        let cfg = Config::from_env();
        assert_eq!(cfg.auto_task_interval_secs, 60);
        assert_eq!(cfg.auto_task_source, "webhook");
        unsafe {
            std::env::remove_var("CODECODER_AUTOTASK_INTERVAL_SECS");
            std::env::remove_var("CODECODER_AUTOTASK_SOURCE");
        }
    }
}
