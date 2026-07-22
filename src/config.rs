// Runtime configuration from CODECODER_* env vars (see README.md env table).
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: Option<String>,
    pub model: String,
    pub api_base: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub root: PathBuf,
    pub github_token: Option<String>,
    /// BG 护栏:每次 BG 调用最多推进的 milestone 数(spec 2026-07-22)。
    pub bg_max_auto: usize,
    /// BG 护栏:连续失败熔断阈值。
    pub bg_circuit_k: usize,
    /// BG 护栏:单 milestone turn 的工具迭代上限(< 全局 12)。
    pub bg_milestone_tool_cap: usize,
    /// Persistent Capability 跨重启崩溃预算(ADR 0034)。
    pub supervisor_crash_budget: u32,
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
                .unwrap_or(4096),
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
            supervisor_crash_budget: env("CODECODER_SUPERVISOR_CRASH_BUDGET")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
        }
    }
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
    fn supervisor_crash_budget_default_and_override() {
        unsafe { std::env::remove_var("CODECODER_SUPERVISOR_CRASH_BUDGET"); }
        assert_eq!(Config::from_env().supervisor_crash_budget, 3);
        unsafe { std::env::set_var("CODECODER_SUPERVISOR_CRASH_BUDGET", "5"); }
        assert_eq!(Config::from_env().supervisor_crash_budget, 5);
        unsafe { std::env::remove_var("CODECODER_SUPERVISOR_CRASH_BUDGET"); }
    }
}
