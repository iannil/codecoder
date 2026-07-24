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
                .unwrap_or(3),
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

/// 从 `path` 读 dotenv;对每个 key 仅在进程 env 未设置时 set_var(显式 env 优先)。
/// 文件不存在/读失败静默返回 0。返回实际注入的 key 数。
pub fn autoload_ccd_env_from(path: &std::path::Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else { return 0; };
    let mut injected = 0usize;
    for (k, v) in parse_dotenv(&text) {
        if std::env::var(&k).is_err() {
            unsafe { std::env::set_var(&k, &v); }
            injected += 1;
        }
    }
    injected
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
        assert_eq!(c.bg_max_auto, 3);
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
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(".ccd.env");
        std::fs::write(&f, "CODECODER_TEST_DOTENV_ZZ=fromfile\nCODECODER_TEST_DOTENV_YY=file2\n").unwrap();
        unsafe {
            std::env::remove_var("CODECODER_TEST_DOTENV_ZZ");
            std::env::set_var("CODECODER_TEST_DOTENV_YY", "explicit");
        }
        let n = autoload_ccd_env_from(&f);
        assert_eq!(std::env::var("CODECODER_TEST_DOTENV_ZZ").unwrap(), "fromfile"); // 未设置 → 注入
        assert_eq!(std::env::var("CODECODER_TEST_DOTENV_YY").unwrap(), "explicit"); // 已设置 → 不覆盖
        assert_eq!(n, 1, "only the unset key is injected");
        unsafe {
            std::env::remove_var("CODECODER_TEST_DOTENV_ZZ");
            std::env::remove_var("CODECODER_TEST_DOTENV_YY");
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
}
