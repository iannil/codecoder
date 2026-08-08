// Runtime configuration from three-layer JSON config (builtin default → user config → project config).
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Serializes tests (across modules in this lib test binary) that read/mutate the
/// process-global `CODECODER_MAX_TOKENS_CEILING` env var.
pub(crate) static MAX_TOKENS_CEILING_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Serializes tests that read/mutate the CODECODER_SELF_OBSERVE env var.
pub(crate) static SELF_OBSERVE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: Option<String>,
    pub model: String,
    pub api_base: String,
    pub max_tokens: u32,
    /// 自适应截断根治:命中 StopReason::Length 时,单 turn 有效 max_tokens 翻倍上调的封顶值
    /// (迭代 2)。默认 32768。
    pub max_tokens_ceiling: u32,
    /// no-op 探索兜底(迭代 4):单 turn 内连续多少个「纯探索」迭代后注入一次 steering nudge。
    /// 0 = 禁用。默认 3。
    pub noop_nudge_threshold: usize,
    pub temperature: f32,
    pub root: PathBuf,
    pub github_token: Option<String>,
    /// BG 护栏:每次 BG 调用最多推进的 milestone 数(spec 2026-07-22)。
    pub bg_max_auto: usize,
    /// BG 外循环自动轮数上限。超过此数后暂停询问用户。默认 3。
    pub bg_max_auto_cycles: usize,
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
    /// 是否启用 tier-2 compaction (LLM 摘要)。默认 true。
    pub compaction_tier2: bool,
    /// daemon workgraph 推进线程间隔（秒）。默认 30。
    pub wg_tick_secs: u64,
    /// Supervisor 监督线程间隔（秒）。默认 1。
    pub supervisor_tick_secs: u64,
    /// OnDemand capability 自动 reaper 延迟（秒）。默认 5。
    pub ondemand_reaper_secs: u64,
    /// 自动任务发现轮询间隔（秒）。0 = 禁用。默认 300。
    pub auto_task_interval_secs: u64,
    /// 任务源类型。默认 "github_issues"。
    pub auto_task_source: String,
    /// LLM provider 调用最大重试次数(含首次)。0 = 不重试。默认 3。
    pub provider_retry_max: u32,
    /// 重试初始退避毫秒数。每次重试加倍,封顶 30s。默认 1000。
    pub provider_retry_initial_ms: u64,
    /// 可选的主 provider 失败后的 fallback API base。
    pub fallback_api_base: Option<String>,
    /// fallback 模型的名称。
    pub fallback_model: Option<String>,
    /// 告警 webhook URL (Slack-compatible)。
    pub alert_webhook: Option<String>,
    /// 是否仅失败时告警。默认 true。
    pub alert_on_failure_only: bool,
    /// 是否在 daemon 崩溃后自动重启并恢复 session。默认 false。
    pub daemon_auto_restart: bool,
    /// Provider 健康探针：连续失败多少次后触发告警并跳过 workgraph tick。0 = 禁用探针。默认 5。
    pub probe_failure_threshold: u32,
    /// 是否启用 workgraph 自动续期：全部 milestone done 后自动从 AGENTS.md 重新 seed。默认 true。
    pub wg_auto_renew: bool,
    /// sessions/ 目录最大文件数，超限时删除最旧的。0 = 不限制。默认 100。
    pub max_sessions: u32,
    /// bg_ledger.jsonl 最大行数，超限时截断。0 = 不限制。默认 10000。
    pub max_ledger_lines: u32,
    /// 是否启用 LLM 自省（CODECODER_SELF_OBSERVE）。
    /// 启用后每 turn 结束后将上轮 trace 摘要注入下一轮 system prompt。
    pub self_observe: bool,
}

/// 只含覆盖项的 JSON 补丁：每个字段 Option，缺失=None（不覆盖下层）。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfigPatch {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub api_base: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_tokens_ceiling: Option<u32>,
    pub noop_nudge_threshold: Option<usize>,
    pub temperature: Option<f32>,
    pub github_token: Option<String>,
    pub bg_max_auto: Option<usize>,
    pub bg_max_auto_cycles: Option<usize>,
    pub bg_circuit_k: Option<usize>,
    pub bg_milestone_tool_cap: Option<usize>,
    pub bg_max_fix_attempts: Option<usize>,
    pub supervisor_crash_budget: Option<u32>,
    pub max_tool_output: Option<usize>,
    pub command_timeout_secs: Option<u32>,
    pub compaction_tier2: Option<bool>,
    pub wg_tick_secs: Option<u64>,
    pub supervisor_tick_secs: Option<u64>,
    pub ondemand_reaper_secs: Option<u64>,
    pub auto_task_interval_secs: Option<u64>,
    pub auto_task_source: Option<String>,
    pub provider_retry_max: Option<u32>,
    pub provider_retry_initial_ms: Option<u64>,
    pub fallback_api_base: Option<String>,
    pub fallback_model: Option<String>,
    pub alert_webhook: Option<String>,
    pub alert_on_failure_only: Option<bool>,
    pub daemon_auto_restart: Option<bool>,
    pub probe_failure_threshold: Option<u32>,
    pub wg_auto_renew: Option<bool>,
    pub max_sessions: Option<u32>,
    pub max_ledger_lines: Option<u32>,
    pub self_observe: Option<bool>,
}

impl Config {
    /// 默认值（root 取自 env/CWD）。与旧 from_env 默认一致。
    pub fn default() -> Self {
        let root = std::env::var("CODECODER_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        Config {
            api_key: None,
            model: "gpt-4o".into(),
            api_base: "https://api.openai.com/v1".into(),
            max_tokens: 8192,
            max_tokens_ceiling: 32768,
            noop_nudge_threshold: 3,
            temperature: 0.7,
            root,
            github_token: None,
            bg_max_auto: 0,
            bg_max_auto_cycles: 3,
            bg_circuit_k: 2,
            bg_milestone_tool_cap: 15,
            bg_max_fix_attempts: 3,
            supervisor_crash_budget: 3,
            max_tool_output: 256 * 1024,
            command_timeout_secs: 0,
            compaction_tier2: true,
            wg_tick_secs: 30,
            supervisor_tick_secs: 1,
            ondemand_reaper_secs: 5,
            auto_task_interval_secs: 300,
            auto_task_source: "github_issues".into(),
            provider_retry_max: 3,
            provider_retry_initial_ms: 1000,
            fallback_api_base: None,
            fallback_model: None,
            alert_webhook: None,
            alert_on_failure_only: true,
            daemon_auto_restart: false,
            probe_failure_threshold: 5,
            wg_auto_renew: true,
            max_sessions: 100,
            max_ledger_lines: 10000,
            self_observe: false,
        }
    }

    /// 兼容性 shim：旧调用方(CONFIG::from_env()) 继续正常工作。
    pub fn from_env() -> Self {
        Self::load()
    }

    /// 读三层合并：内置默认 → 用户级 → 项目级。越靠后越覆盖。
    pub fn load() -> Self {
        Self::load_with_root(Self::default().root)
    }

    /// 显式 root 的加载（测试用）。
    pub fn load_with_root(root: PathBuf) -> Self {
        let mut cfg = Config::default();
        cfg.root = root.clone();
        if let Some(user) = user_config_path() {
            if let Ok(patch) = read_patch(&user) {
                cfg.apply_patch(patch);
            }
        }
        let proj = project_config_path(&root);
        if let Ok(patch) = read_patch(&proj) {
            cfg.apply_patch(patch);
        }
        cfg
    }

    /// 把补丁中非 None 字段覆盖到 self。
    pub fn apply_patch(&mut self, p: ConfigPatch) {
        macro_rules! set {
            ($field:ident) => { if let Some(v) = p.$field { self.$field = v; } };
        }
        // 标量字段（Config 中为具体类型，patch 中为 Option<T>）。
        set!(model); set!(api_base); set!(max_tokens);
        set!(max_tokens_ceiling); set!(noop_nudge_threshold); set!(temperature);
        set!(bg_max_auto); set!(bg_max_auto_cycles); set!(bg_circuit_k);
        set!(bg_milestone_tool_cap); set!(bg_max_fix_attempts); set!(supervisor_crash_budget);
        set!(max_tool_output); set!(command_timeout_secs); set!(compaction_tier2);
        set!(wg_tick_secs); set!(supervisor_tick_secs); set!(ondemand_reaper_secs);
        set!(auto_task_interval_secs); set!(auto_task_source); set!(provider_retry_max);
        set!(provider_retry_initial_ms);
        set!(alert_on_failure_only); set!(daemon_auto_restart);
        set!(probe_failure_threshold); set!(wg_auto_renew); set!(max_sessions);
        set!(max_ledger_lines); set!(self_observe);
        // Option 字段（Config 与 patch 均为 Option<T>）：非 None 时直接覆盖。
        if let Some(v) = p.api_key { self.api_key = Some(v); }
        if let Some(v) = p.github_token { self.github_token = Some(v); }
        if let Some(v) = p.fallback_api_base { self.fallback_api_base = Some(v); }
        if let Some(v) = p.fallback_model { self.fallback_model = Some(v); }
        if let Some(v) = p.alert_webhook { self.alert_webhook = Some(v); }
    }
}

/// 用户级配置路径：Unix `$HOME`，Windows `$USERPROFILE`。
pub fn user_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.map(|h| PathBuf::from(h).join(".codecoder").join("codecoder.json"))
}

/// 项目级配置路径：`<root>/.codecoder/codecoder.json`。
pub fn project_config_path(root: &Path) -> PathBuf {
    root.join(".codecoder").join("codecoder.json")
}

/// 从单文件读 JSON 补丁；缺失/非法 → Err（由 load 静默忽略）。
pub fn read_patch(path: &Path) -> anyhow::Result<ConfigPatch> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_values_match_legacy() {
        let _g = MAX_TOKENS_CEILING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let c = Config::load_with_root(PathBuf::from("/tmp/nonexistent_ccd"));
        assert_eq!(c.model, "gpt-4o");
        assert_eq!(c.max_tokens, 8192);
        assert_eq!(c.max_tokens_ceiling, 32768);
        assert_eq!(c.noop_nudge_threshold, 3);
        assert_eq!(c.temperature, 0.7);
        assert_eq!(c.bg_max_auto, 0);
        assert_eq!(c.bg_max_auto_cycles, 3);
        assert_eq!(c.bg_circuit_k, 2);
        assert_eq!(c.bg_milestone_tool_cap, 15);
        assert_eq!(c.bg_max_fix_attempts, 3);
        assert_eq!(c.supervisor_crash_budget, 3);
        assert_eq!(c.max_tool_output, 256 * 1024);
        assert_eq!(c.command_timeout_secs, 0);
        assert!(c.compaction_tier2);
        assert_eq!(c.wg_tick_secs, 30);
        assert_eq!(c.supervisor_tick_secs, 1);
        assert_eq!(c.ondemand_reaper_secs, 5);
        assert_eq!(c.auto_task_interval_secs, 300);
        assert_eq!(c.auto_task_source, "github_issues");
        assert_eq!(c.provider_retry_max, 3);
        assert_eq!(c.provider_retry_initial_ms, 1000);
        assert!(c.alert_on_failure_only);
        assert!(!c.daemon_auto_restart);
        assert_eq!(c.probe_failure_threshold, 5);
        assert!(c.wg_auto_renew);
        assert_eq!(c.max_sessions, 100);
        assert_eq!(c.max_ledger_lines, 10000);
        assert!(!c.self_observe);
        assert!(c.api_key.is_none());
        assert!(c.github_token.is_none());
    }

    #[test]
    fn apply_patch_overrides_present_fields_only() {
        let mut c = Config::load_with_root(PathBuf::from("/tmp/nonexistent_ccd"));
        let patch = ConfigPatch {
            model: Some("deepseek".into()),
            max_tokens: Some(4096),
            ..Default::default()
        };
        c.apply_patch(patch);
        assert_eq!(c.model, "deepseek");
        assert_eq!(c.max_tokens, 4096);
        // 未覆盖字段保持默认
        assert_eq!(c.temperature, 0.7);
        assert_eq!(c.bg_circuit_k, 2);
    }

    #[test]
    fn project_layer_overrides_user_layer() {
        let dir = tempfile::tempdir().unwrap();
        let user = ConfigPatch { model: Some("user-model".into()), max_tokens: Some(2048), ..Default::default() };
        let project = ConfigPatch { model: Some("project-model".into()), ..Default::default() };
        let mut c = Config::load_with_root(dir.path().to_path_buf());
        c.apply_patch(user);
        c.apply_patch(project);
        assert_eq!(c.model, "project-model");   // 项目层覆盖用户层
        assert_eq!(c.max_tokens, 2048);          // 项目层未设 → 保留用户层
    }

    #[test]
    fn read_patch_parses_json_and_defaults_missing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("codecoder.json");
        std::fs::write(&f, r#"{"model":"from-json","max_tokens":512}"#).unwrap();
        let p = read_patch(&f).unwrap();
        assert_eq!(p.model.as_deref(), Some("from-json"));
        assert_eq!(p.max_tokens, Some(512));
        assert_eq!(p.temperature, None, "缺失字段应为 None");
    }

    #[test]
    fn read_patch_missing_file_is_err_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = read_patch(&dir.path().join("nope.json"));
        assert!(p.is_err(), "缺失文件应报错（由 load 静默忽略）");
    }

    #[test]
    fn project_config_path_nested_dotdir() {
        let root = PathBuf::from("/tmp/root");
        assert_eq!(project_config_path(&root), PathBuf::from("/tmp/root/.codecoder/codecoder.json"));
    }
}