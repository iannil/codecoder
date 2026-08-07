# CodeCoder 重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 四项重构：二进制重命名 ccda/ccli/ccweb、CLI help/skill 结构化输出、三层 JSON 配置、workgraph 门禁取消。

**Architecture:** 按依赖顺序串行：Task 1 二进制重命名（无依赖）→ Task 2 CLI 输出 + Task 3 三层配置（都依赖 Task 1，互不依赖）→ Task 4 workgraph 门禁取消（依赖 Task 3）。每个任务独立可测并提交。

**Tech Stack:** Rust, serde, serde_json, fs2

## Global Constraints

- 三个二进制名：`ccda`（daemon）、`ccli`（client）、`ccweb`（web）；全部在 `src/bin/`，`src/lib.rs` 保持公共库
- `-h`/`--help` 与 `--skill <name>` 默认纯文本 markdown，追加 `--json` 输出结构化 JSON
- 三层 JSON 配置：内置默认 → `$HOME/.codecoder/codecoder.json`（Windows `$USERPROFILE`）→ `$PROJECT_ROOT/.codecoder/codecoder.json`，后者覆盖前者
- 保留 env 例外（仅执行路由，不进 JSON）：`CODECODER_ROOT`、`CODECODER_DAEMON`、`CODECODER_BG_TASK`、`CODECODER_BG_WORKGRAPH`、`CODECODER_SCRIPT`
- `Config::from_env()` 废弃 → `Config::load()` 读三层合并；`.ccd.env` 与 `autoload_ccd_env*`/`parse_dotenv`/`DOTENV_ALLOWED_KEYS` 全删
- workgraph 删除 `NeedsFix`/`Verdict`/`GateKind`/`acceptance`/`checks`/`command`/`fix_attempts`/`last_failure`；`NodeStatus` 保留 `Pending`/`InProgress`/`Blocked`/`Done`/`Hypothesis`/`Locked`
- `MissionState` 简化为 `Running`/`Completed`/`EmptyGraph`/`Error(String)`
- 验收由 agent 自报，workgraph 内核只做依赖编排、不做任何客观校验
- 依赖链保留；`engineer*`/`rc*` 技能保留在 `skills/`，指导验收阶段

---

### Task 1: 二进制重命名 ccda / ccli / ccweb

**Files:**
- Move: `src/main.rs` → `src/bin/ccda.rs`
- Move: `src/bin/cc.rs` → `src/bin/ccli.rs`
- Move: `src/bin/cc-web.rs` → `src/bin/ccweb.rs`
- Modify: `Cargo.toml`
- Modify: `src/bin/ccda.rs`（错误消息中的 `ccd:` 前缀可保留；`--help` 文案里的 `codecoder` 保留为产品名）
- Modify: `src/bin/ccli.rs`（内部 `cc` 提示改 `ccli`）
- Modify: `src/bin/ccweb.rs`（内部 `cc-web` 提示改 `ccweb`）

**Interfaces:**
- Consumes: 现有 `src/main.rs`、`src/bin/cc.rs`、`src/bin/cc-web.rs`
- Produces: 三个新二进制入口 `ccda`/`ccli`/`ccweb`，对应 `Cargo.toml` 三个 `[[bin]]`

- [ ] **Step 1: 移动文件**

用 `git mv` 保留历史：
```bash
git mv src/main.rs src/bin/ccda.rs
git mv src/bin/cc.rs src/bin/ccli.rs
git mv src/bin/cc-web.rs src/bin/ccweb.rs
```

- [ ] **Step 2: 更新 Cargo.toml**

把 `Cargo.toml` 中的 `[[bin]]` 段替换为：
```toml
[[bin]]
name = "ccda"
path = "src/bin/ccda.rs"

[[bin]]
name = "ccli"
path = "src/bin/ccli.rs"

[[bin]]
name = "ccweb"
path = "src/bin/ccweb.rs"
```

- [ ] **Step 3: 更新二进制内部提示文案**

`src/bin/ccweb.rs` 中所有 `cc-web:` 前缀字符串改为 `ccweb:`（9 处，见 grep 结果）。`src/bin/ccli.rs` 的 `cc>` REPL 提示与 `cc ledger` 等保留（产品短名），但 `--help` 首行 `cc -- CodeCoder client` 改为 `ccli -- CodeCoder client`。`src/bin/ccda.rs` 的 `ccd:` 错误前缀保留（daemon 短名），`--help` 里 `CODECODER_DAEMON=1 cargo run` 提示改为 `cargo run --bin ccda`。

- [ ] **Step 4: 编译验证**

```bash
cargo build 2>&1 | tail -20
```
Expected: 编译通过，生成 `target/debug/ccda`、`target/debug/ccli`、`target/debug/ccweb`。

- [ ] **Step 5: 跑测试确认无引用断裂**

```bash
cargo test 2>&1 | tail -25
```
Expected: 全部通过（部分 `#[ignore]` 除外）。若有测试脚本引用旧二进制名（`docs/superpowers/scripts/*.sh` 的 `CC_BIN`/`BG_BIN` 默认路径），本任务不改（它们有 env 覆盖）。

- [ ] **Step 6: 提交**

```bash
git add -A
git commit -m "refactor(bin): rename binaries to ccda/ccli/ccweb

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: 共享 CLI help/skill 输出模块

**Files:**
- Create: `src/help.rs`
- Modify: `src/lib.rs`（`pub mod help;`）
- Modify: `src/bin/ccda.rs`
- Modify: `src/bin/ccli.rs`
- Modify: `src/bin/ccweb.rs`

**Interfaces:**
- Consumes: 无（纯新增）
- Produces:
  - `help::SkillEntry { name, description, usage: &'static [&'static str], schema: Option<&'static str>, template: Option<&'static str> }`
  - `help::HelpSpec { binary, title, description, usage, config_note, skills }`
  - `help::HelpRequest { Help { json: bool }, Skill { name: String, json: bool } }`
  - `help::parse_help_request(args: &[String]) -> Option<HelpRequest>`
  - `help::render_help(spec: &HelpSpec) -> String`
  - `help::render_skill(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<String>`
  - `help::help_json(spec: &HelpSpec) -> serde_json::Value`
  - `help::skill_json(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<serde_json::Value>`

- [ ] **Step 1: 写失败测试**

新建 `src/help.rs` 底部测试模块。先写测试文件逻辑（Step 3 实现函数体）：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HelpSpec {
        HelpSpec {
            binary: "ccda",
            title: "CodeCoder daemon",
            description: "Autonomous AI agent daemon",
            usage: &["ccda [FLAGS]", "ccda --skill <name>"],
            config_note: "Config: $HOME/.codecoder/codecoder.json then <root>/.codecoder/codecoder.json",
            skills: &[SkillEntry {
                name: "daemon",
                description: "Run the daemon",
                usage: &["ccda"],
                schema: None,
                template: Some("CODECODER_ROOT=/path ccda"),
            }],
        }
    }

    #[test]
    fn parse_help_request_recognizes_help_and_json() {
        assert!(matches!(parse_help_request(&["--help".into()]), Some(HelpRequest::Help{json:false})));
        assert!(matches!(parse_help_request(&["-h".into(), "--json".into()]), Some(HelpRequest::Help{json:true})));
        assert!(matches!(parse_help_request(&["--skill".into(), "daemon".into()]), Some(HelpRequest::Skill{name, json:false}) if name=="daemon"));
        assert!(matches!(parse_help_request(&["--skill".into(), "daemon".into(), "--json".into()]), Some(HelpRequest::Skill{json:true,..})));
        // skills-dir fallback: unknown skill name returns None from render_skill
        let s = spec();
        assert!(render_skill(&s, "missing", std::path::Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn render_help_contains_binary_and_skills() {
        let text = render_help(&spec());
        assert!(text.contains("ccda"));
        assert!(text.contains("daemon"));
        assert!(text.contains("--skill"));
    }

    #[test]
    fn json_output_is_structurally_valid() {
        let j = help_json(&spec());
        assert_eq!(j["binary"], "ccda");
        assert_eq!(j["skills"][0]["name"], "daemon");
        let sj = skill_json(&spec(), "daemon", std::path::Path::new("/nonexistent")).unwrap();
        assert_eq!(sj["template"], "CODECODER_ROOT=/path ccda");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test --lib help:: 2>&1 | tail -15
```
Expected: 编译错误（`help` 模块不存在或函数未定义）。

- [ ] **Step 3: 实现 help.rs**

写入完整 `src/help.rs`：
```rust
// CLI help/skill 输出（spec 2026-08-07 §2.2）：ccda/ccli/ccweb 共用。
// 默认纯文本 markdown，`--json` 输出结构化 JSON，便于 LLM agent 解析。
use serde_json::Value;
use std::path::Path;

/// 单个技能/模式/子命令的条目。
#[derive(Debug, Clone, Copy)]
pub struct SkillEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static [&'static str],
    /// 参数/输出结构说明（可含 JSON 片段）。
    pub schema: Option<&'static str>,
    /// 可直接复用的模板。
    pub template: Option<&'static str>,
}

/// 单个二进制的帮助规格。
pub struct HelpSpec {
    pub binary: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub usage: &'static [&'static str],
    pub config_note: &'static str,
    pub skills: &'static [SkillEntry],
}

/// 一次帮助请求的解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum HelpRequest {
    Help { json: bool },
    Skill { name: String, json: bool },
}

/// 扫描参数找 `--help`/`-h`、`--skill <name>`/`-s <name>`、`--json`。
/// 返回 None 表示无帮助请求（正常运行）。
pub fn parse_help_request(args: &[String]) -> Option<HelpRequest> {
    let mut json = false;
    let mut skill: Option<String> = None;
    let mut i = 0;
    let mut help = false;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--help" | "-h" => help = true,
            "--skill" | "-s" => {
                if i + 1 < args.len() {
                    skill = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(name) = skill {
        return Some(HelpRequest::Skill { name, json });
    }
    if help {
        return Some(HelpRequest::Help { json });
    }
    None
}

/// 渲染纯文本帮助（markdown）。
pub fn render_help(spec: &HelpSpec) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {} — {}\n\n{}\n\n", spec.title, spec.description, spec.description));
    s.push_str("## USAGE\n\n");
    for u in spec.usage {
        s.push_str(&format!("```\n{u}\n```\n"));
    }
    s.push_str("\n## CONFIGURATION\n\n");
    s.push_str(spec.config_note);
    s.push_str("\n\n## SKILLS\n\n");
    for sk in spec.skills {
        s.push_str(&format!("- **`{}`** — {}\n", sk.name, sk.description));
    }
    s.push_str("\n查看某技能详情：`");
    s.push_str(spec.binary);
    s.push_str(" --skill <name>`；结构化输出追加 `--json`。\n");
    s
}

fn find_skill<'a>(spec: &'a HelpSpec, name: &str) -> Option<&'a SkillEntry> {
    spec.skills.iter().find(|s| s.name == name)
}

/// 渲染单个技能详情。先查内置技能表；查不到则读仓库 `skills/<name>.md`。
pub fn render_skill(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<String> {
    if let Some(sk) = find_skill(spec, name) {
        return Some(render_skill_entry(name, sk));
    }
    let f = skills_dir.join(format!("{name}.md"));
    std::fs::read_to_string(&f).ok()
}

fn render_skill_entry(name: &str, sk: &SkillEntry) -> String {
    let mut s = format!("# Skill: {name}\n\n{}\n\n## Usage\n\n", sk.description);
    for u in sk.usage {
        s.push_str(&format!("```\n{u}\n```\n"));
    }
    if let Some(sch) = sk.schema {
        s.push_str(&format!("\n## Schema\n\n```json\n{sch}\n```\n"));
    }
    if let Some(t) = sk.template {
        s.push_str(&format!("\n## Template\n\n```\n{t}\n```\n"));
    }
    s
}

fn skill_to_value(name: &str, sk: &SkillEntry) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), Value::String(name.into()));
    m.insert("description".into(), Value::String(sk.description.into()));
    m.insert("usage".into(), Value::Array(sk.usage.iter().map(|u| Value::String((*u).into())).collect()));
    if let Some(sch) = sk.schema {
        m.insert("schema".into(), Value::String(sch.into()));
    }
    if let Some(t) = sk.template {
        m.insert("template".into(), Value::String(t.into()));
    }
    Value::Object(m)
}

/// 完整帮助的 JSON 结构。
pub fn help_json(spec: &HelpSpec) -> Value {
    let skills: Vec<Value> = spec.skills.iter().map(|sk| skill_to_value(sk.name, sk)).collect();
    serde_json::json!({
        "binary": spec.binary,
        "title": spec.title,
        "description": spec.description,
        "usage": spec.usage,
        "config": spec.config_note,
        "skills": skills,
    })
}

/// 单个技能的 JSON。查不到内置技能则读仓库 `skills/<name>.md` 原文。
pub fn skill_json(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<Value> {
    if let Some(sk) = find_skill(spec, name) {
        return Some(skill_to_value(name, sk));
    }
    let f = skills_dir.join(format!("{name}.md"));
    let content = std::fs::read_to_string(&f).ok()?;
    Some(serde_json::json!({ "name": name, "source": "skills/", "content": content }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HelpSpec {
        HelpSpec {
            binary: "ccda",
            title: "CodeCoder daemon",
            description: "Autonomous AI agent daemon",
            usage: &["ccda [FLAGS]", "ccda --skill <name>"],
            config_note: "Config: $HOME/.codecoder/codecoder.json then <root>/.codecoder/codecoder.json",
            skills: &[SkillEntry {
                name: "daemon",
                description: "Run the daemon",
                usage: &["ccda"],
                schema: None,
                template: Some("CODECODER_ROOT=/path ccda"),
            }],
        }
    }

    #[test]
    fn parse_help_request_recognizes_help_and_json() {
        assert!(matches!(parse_help_request(&["--help".into()]), Some(HelpRequest::Help{json:false})));
        assert!(matches!(parse_help_request(&["-h".into(), "--json".into()]), Some(HelpRequest::Help{json:true})));
        assert!(matches!(parse_help_request(&["--skill".into(), "daemon".into()]), Some(HelpRequest::Skill{name, json:false}) if name=="daemon"));
        assert!(matches!(parse_help_request(&["--skill".into(), "daemon".into(), "--json".into()]), Some(HelpRequest::Skill{json:true,..})));
        assert!(parse_help_request(&["--port".into(), "9876".into()]).is_none());
    }

    #[test]
    fn render_skill_missing_returns_none() {
        let s = spec();
        assert!(render_skill(&s, "missing", Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn render_help_contains_binary_and_skills() {
        let text = render_help(&spec());
        assert!(text.contains("ccda"));
        assert!(text.contains("daemon"));
        assert!(text.contains("--skill"));
    }

    #[test]
    fn json_output_is_structurally_valid() {
        let j = help_json(&spec());
        assert_eq!(j["binary"], "ccda");
        assert_eq!(j["skills"][0]["name"], "daemon");
        let sj = skill_json(&spec(), "daemon", Path::new("/nonexistent")).unwrap();
        assert_eq!(sj["template"], "CODECODER_ROOT=/path ccda");
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test --lib help:: 2>&1 | tail -15
```
Expected: 4 个测试全部 PASS。

- [ ] **Step 5: 在 lib.rs 注册模块**

`src/lib.rs` 的 `pub mod config;` 附近加 `pub mod help;`。

- [ ] **Step 6: 提交**

```bash
git add src/help.rs src/lib.rs
git commit -m "feat(cli): shared help/skill rendering module (text + json)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 三层 JSON 配置

**Files:**
- Rewrite: `src/config.rs`
- Modify: `src/bin/ccda.rs`（`Config::from_env()` → `Config::load()`，删 `autoload_ccd_env()`）
- Modify: `src/bin/ccli.rs`（同上）
- Modify: `src/background.rs:156,595`（`Config::from_env()` → `Config::load()`）

**Interfaces:**
- Consumes: 现有 `Config` 全部字段
- Produces:
  - `Config::load() -> Config`（读三层合并，root 取自 env/CWD）
  - `Config::load_with_root(root: PathBuf) -> Config`（测试与显式 root 用）
  - `Config::default() -> Config`
  - `ConfigPatch`（全 `Option<T>`，`#[serde(default)]`，`Deserialize`）
  - `Config::apply_patch(&mut self, patch: ConfigPatch)`
  - `config::user_config_path() -> Option<PathBuf>`
  - `config::read_patch(path: &Path) -> anyhow::Result<ConfigPatch>`
  - `config::project_config_path(root: &Path) -> PathBuf`
- 删除：`from_env()`、`autoload_ccd_env()`、`autoload_ccd_env_from()`、`parse_dotenv()`、`DOTENV_ALLOWED_KEYS`

- [ ] **Step 1: 写失败测试**

`src/config.rs` 测试模块替换为以下（在 Step 3 实现后通过）：
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_values_match_legacy() {
        let _ = MAX_TOKENS_CEILING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let c = Config::load_with_root(PathBuf::from("/tmp/nonexistent_ccd"));
        assert_eq!(c.model, "gpt-4o");
        assert_eq!(c.max_tokens, 8192);
        assert_eq!(c.max_tokens_ceiling, 32768);
        assert_eq!(c.noop_nudge_threshold, 3);
        assert_eq!(c.temperature, 0.7);
        assert_eq!(c.bg_max_auto, 0);
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
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test --lib config:: 2>&1 | tail -15
```
Expected: 编译失败（`ConfigPatch`/`load_with_root`/`apply_patch` 未定义）。

- [ ] **Step 3: 重写 config.rs 主体**

保留 `Config` 结构体字段不变（含 `root: PathBuf`），但：
1. 给 `Config` 加 `#[derive(Serialize, Deserialize)]`
2. `root` 字段加 `#[serde(skip)]`（不序列化进 JSON，由 env/CWD 决定）
3. 全部字段加 `#[serde(default)]`（层级缺失字段继承下层）
4. 删除原有 `from_env()`；新增以下实现：

```rust
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
        set!(api_key); set!(model); set!(api_base); set!(max_tokens);
        set!(max_tokens_ceiling); set!(noop_nudge_threshold); set!(temperature);
        set!(github_token); set!(bg_max_auto); set!(bg_circuit_k);
        set!(bg_milestone_tool_cap); set!(bg_max_fix_attempts); set!(supervisor_crash_budget);
        set!(max_tool_output); set!(command_timeout_secs); set!(compaction_tier2);
        set!(wg_tick_secs); set!(supervisor_tick_secs); set!(ondemand_reaper_secs);
        set!(auto_task_interval_secs); set!(auto_task_source); set!(provider_retry_max);
        set!(provider_retry_initial_ms); set!(fallback_api_base); set!(fallback_model);
        set!(alert_webhook); set!(alert_on_failure_only); set!(daemon_auto_restart);
        set!(probe_failure_threshold); set!(wg_auto_renew); set!(max_sessions);
        set!(max_ledger_lines); set!(self_observe);
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
```

同时删除整个 `parse_dotenv`、`autoload_ccd_env_from`、`autoload_ccd_env`、`DOTENV_ALLOWED_KEYS` 及旧测试。保留 `MAX_TOKENS_CEILING_ENV_LOCK`/`SELF_OBSERVE_ENV_LOCK`（若其它模块仍引用；否则删除）。

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test --lib config:: 2>&1 | tail -15
```
Expected: 6 个新测试全部 PASS。

- [ ] **Step 5: 更新调用方**

`src/bin/ccda.rs`、`src/bin/ccli.rs`、`src/background.rs`（2 处）把 `Config::from_env()` 替换为 `Config::load()`；删除 `src/bin/ccda.rs` 与 `src/bin/ccli.rs` 中的 `codecoder::config::autoload_ccd_env();` 调用。

- [ ] **Step 6: 全量编译 + 测试**

```bash
cargo build 2>&1 | tail -10 && cargo test 2>&1 | tail -20
```
Expected: 编译通过，测试通过。

- [ ] **Step 7: 提交**

```bash
git add src/config.rs src/bin/ccda.rs src/bin/ccli.rs src/background.rs
git commit -m "feat(config): three-layer JSON config, drop .ccd.env and env config

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: workgraph 门禁取消

**Files:**
- Delete: `src/bg_gate.rs`
- Modify: `src/workgraph.rs`（删 gate 字段/状态/函数，升 schema_version=2，加迁移）
- Modify: `src/background.rs`（简化推进循环，删 gate/review/retry）
- Modify: `src/bg_ledger.rs`（简化 MissionState 与计数）
- Modify: `src/lib.rs`（删 `pub mod bg_gate`，更新测试）
- Modify: `src/tool/dev.rs`（milestone 工具去掉 acceptance/command/needs_fix/verdict）
- Modify: `src/tool/generate_milestones.rs`（只产 title，不再产 acceptance）
- Modify: `src/daemon/session_manager.rs`（milestone_reset：NeedsFix → 非 Done 重置）
- Modify: `src/daemon/mod.rs`（如引用 gate 相关则清理）
- Modify: `src/daemon/proto.rs`、`src/daemon/socket.rs`（删 `MilestoneReset` 请求，若存在）
- Modify: `src/bin/ccli.rs`（删 `milestone-reset` 子命令）

**Interfaces:**
- Consumes: Task 3 的 `Config::load()`（`bg_*` 字段迁移）
- Produces:
  - `MissionState`（在 `bg_ledger.rs` 定义）：`Running`/`Completed`/`EmptyGraph`/`Error(String)`
  - `SubgoalOutcome { milestone_id, touched_files, tool_cap_hit }`（去掉 verdict/gate_reason/gate_kind）
  - `WorkGraph::add(&mut self, title: &str, deps: Vec<u64>) -> anyhow::Result<u64>`
  - `advance_one_milestone(...) -> anyhow::Result<Option<BgOutcome>>`（签名不变，内部不再跑 gate）
  - 删除 `next_retryable`、`retry_one_milestone`、`run_milestone_and_gate`、`build_repair_prompt`、`evaluate` 等

- [ ] **Step 1: 写失败测试（workgraph migration）**

在 `src/workgraph.rs` 测试模块加：
```rust
#[test]
fn migrate_v1_needs_fix_to_pending_and_drops_gate_fields() {
    let raw = r#"{"schema_version":1,"nodes":[
        {"id":1,"title":"a","acceptance":"cargo test","deps":[],"status":"needs_fix","touched":[],
         "verdict":"needs_fix","fix_attempts":2,"last_failure":"x","command":"cargo test","checks":[]},
        {"id":2,"title":"b","acceptance":"","deps":[1],"status":"pending","touched":[]}
    ]}"#;
    let g = WorkGraph::load(raw).unwrap();
    assert_eq!(g.get(1).unwrap().status, NodeStatus::Pending, "needs_fix → pending");
    assert_eq!(g.get(2).unwrap().status, NodeStatus::Pending);
    // 不 panic：gate 字段已丢弃，struct 无这些字段
}
```
运行确认失败（`NodeStatus::NeedsFix` 仍在枚举中，迁移未实现 → 该测试会因 `needs_fix` 字符串无法反序列化而失败，或 schema_version 仍为 1）。

- [ ] **Step 2: 改写 workgraph.rs**

1. `WG_SCHEMA_VERSION` = 2
2. 删除 `CheckType`、`CheckSpec` 枚举/结构
3. `NodeStatus` 删除 `NeedsFix` 变体
4. `Milestone` 删除字段：`acceptance`、`verdict`、`fix_attempts`、`last_failure`、`command`、`checks`；保留 `id`/`title`/`deps`/`status`/`touched`
5. `add(&mut self, title: &str, deps: Vec<u64>)`（去掉 acceptance 参数）
6. 删除 `next_retryable`
7. `render()` 去掉 verdict/command 显示；`render_for_prompt()` 去掉 needs_fix 分支
8. `migrate()` 增加 from=1 分支：
```rust
fn migrate(from: u32, mut json: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    match from {
        0 => Ok(json),
        1 => {
            // 1->2: needs_fix → pending，丢弃 gate 字段
            if let Some(nodes) = json.get_mut("nodes").and_then(|v| v.as_array_mut()) {
                for n in nodes {
                    if let Some(o) = n.as_object_mut() {
                        if o.get("status").and_then(|s| s.as_str()) == Some("needs_fix") {
                            o.insert("status".into(), serde_json::json!("pending"));
                        }
                        for key in ["acceptance","verdict","fix_attempts","last_failure","command","checks"] {
                            o.remove(key);
                        }
                    }
                }
            }
            Ok(json)
        }
        other => anyhow::bail!("no workgraph migration registered from schema_version {other}"),
    }
}
```
9. `migrate_todos` 更新为新结构

- [ ] **Step 3: 运行 workgraph 测试**

```bash
cargo test --lib workgraph:: 2>&1 | tail -20
```
Expected: 新迁移测试通过；依赖 `NeedsFix`/`acceptance`/`command` 的旧测试需同步改写（Step 4）。

- [ ] **Step 4: 改写 workgraph 旧测试**

删除/改写引用 `NeedsFix`、`acceptance`、`command`、`next_retryable`、`fix_attempts`、`CheckType`、`CheckSpec` 的测试（约 15 个）。`add(...)` 调用去掉第二个参数（acceptance）。`with_status`/`Milestone` 字面量构造去掉已删字段。

```bash
cargo test --lib workgraph:: 2>&1 | tail -20
```
Expected: 全部 PASS。

- [ ] **Step 5: 改写 bg_ledger.rs（MissionState 简化）**

在 `bg_ledger.rs` 定义新的 `MissionState`（替代 `bg_gate::MissionState`，`bg_gate.rs` 将删除）：
```rust
/// BG 任务终态（去掉 gate 专用状态）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MissionState {
    Running,
    Completed,
    EmptyGraph,
    Error(String),
}
```
`mission_exit_code`：
```rust
pub fn mission_exit_code(state: &MissionState) -> i32 {
    match state {
        MissionState::Completed | MissionState::Running => 0,
        MissionState::EmptyGraph => 5,
        MissionState::Error(_) => 4,
    }
}
```
`blocked_at_of` 删除（不再有 id）。`read_recent` 的 `only_failed` 过滤改为 `!matches!(r.mission_state, MissionState::Completed)`。`summarize_line` 的 match 简化。`SubgoalOutcome` 改为 `{ milestone_id, touched_files, tool_cap_hit }`（在 `background.rs` 定义，去掉 `SubgoalVerdict`/`gate_reason`/`gate_kind`）。`counts_of`：`passed = subgoals.len()`，`failed = 0`。更新 `bg_ledger.rs` 测试。

- [ ] **Step 6: 改写 background.rs 推进循环**

1. 删除 `use crate::bg_gate::MissionState;` → `use crate::bg_ledger::MissionState;`
2. 删除 `SubgoalVerdict` 枚举；`SubgoalOutcome` 只留 `milestone_id`/`touched_files`/`tool_cap_hit`
3. `run_background_cfg` 签名去掉 `circuit_k`、`max_fix_attempts` 参数（`run_background` 同步去掉传参）
4. 重写 workgraph 分支循环：每轮 `advance_one_milestone`，成功即累加 `advanced`，`advanced >= max_auto` 或无可推进则 `Completed`；空图 `EmptyGraph`；读图失败 `Error`；turn panic → `Error`
5. `advance_one_milestone`：只跑一个 turn（prompt 不再要求 `VERDICT:` 结尾，改为"完成后标记完成"），turn 后直接 `set_status(Done)` + `update_bg_checkpoint`，不再跑 gate
6. 删除 `retry_one_milestone`、`run_milestone_and_gate`、`build_repair_prompt`、`resolve_bg_task` 中 acceptance 引用
7. `advance_one_milestone` 的 prompt 改为：
```rust
let t = format!(
    "workgraph milestone #{}: {}\n\n\
     Complete this milestone. When done, it will be marked complete automatically;\
     no verdict line is required.",
    n.id, n.title,
);
```
8. 更新 `background.rs` 全部测试（`run_background_cfg` 调用去掉两个参数；`MissionState::BlockedAt/CircuitBreaker/StuckNeedsFix` 断言改为 `Completed`/`Error`）

- [ ] **Step 7: 更新 lib.rs**

删除 `pub mod bg_gate;`。更新 `lib.rs` 测试 `run_background_ledger_append_and_exit_code`：`o.mission_state = MissionState::BlockedAt(9)` → `MissionState::Error("x".into())`，退出码断言 4 改为 4。

- [ ] **Step 8: 更新 milestone 工具（tool/dev.rs）**

- `description`：去掉 `needs_fix`、`acceptance`、`command`、verdict 描述，改为 `action = list | add | start | done | next | remove`，`add` 只取 `title`+`deps`，`done` 只标记完成
- `schema`：去掉 `acceptance`/`command`/`verdict` 属性；`action` enum 去掉 `needs_fix`
- `apply`：`add` 分支去掉 acceptance/command 解析与 `bg_gate::gate_command` 提示；`next` 分支去掉 acceptance 显示；删除 `needs_fix` 分支；`done` 分支去掉 verdict gate，直接 `set_status(i, Done)`；删除 `n.verdict` 写入
- 更新 `tool/dev.rs` 测试（`milestone_add_*`、`milestone_non_pass_verdict_lands_needs_fix` 等删除/改写）

- [ ] **Step 9: 更新 generate_milestones 工具**

- prompt 改为只生成 milestone `title`（不再要求 acceptance）
- `parse_milestones` 改为返回 `Vec<String>`（仅标题）
- `g.add(title, vec![])` 调用改为新签名
- 更新测试

- [ ] **Step 10: 更新 daemon 层**

- `src/daemon/session_manager.rs` `milestone_reset`：`NeedsFix` 判断改为"非 Done 即重置为 Pending"（或删除该能力，见 Step 11）
- `src/daemon/proto.rs`、`src/daemon/socket.rs`：若含 `MilestoneReset` 请求类型与分发，删除
- `src/daemon/mod.rs`：确认无 `bg_gate`/`NeedsFix` 引用；`advance_one_milestone` 调用不变
- `src/bin/ccli.rs`：删除 `milestone-reset` 子命令与 `ClientRequest::MilestoneReset` 分支

- [ ] **Step 11: 全量编译 + 测试 + 清理**

```bash
cargo build 2>&1 | tail -20
cargo test 2>&1 | tail -30
```
Expected: 编译通过，所有测试通过。删除 `src/bg_gate.rs`（git rm）。`grep -rn 'bg_gate\|NeedsFix\|next_retryable\|fix_attempts\|CircleBreaker\|StuckNeedsFix' src/` 应无残留（除注释/文档）。

- [ ] **Step 12: 提交**

```bash
git add -A
git commit -m "refactor(workgraph): remove per-milestone gates, acceptance by milestone nodes

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 文档同步

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `README.md`
- Modify: `CONTEXT.md`
- Modify: `CLAUDE.md`
- Modify: `docs/adr/`（0026/0028/0033/0034 相关段落）
- Modify: `skills/driver-codecoder.md`

**Interfaces:**
- Consumes: 前 4 个任务的全部产物

- [ ] **Step 1: 更新二进制名**

`README.md`/`CLAUDE.md`/`skills/driver-codecoder.md` 中 `cargo run`、`cargo run --bin cc`、`cc-web`、`target/release/{codecoder,cc}` 全部改为 `ccda`/`ccli`/`ccweb`。

- [ ] **Step 2: 更新配置说明**

`README.md` 的 env 表与 `.ccd.env` 说明段删除，改为三层 JSON 配置说明（含 `codecoder.json` 模板、层级、`root` 与执行路由 env 例外）。`ARCHITECTURE.md` 的 `config.rs` 行更新。`CONTEXT.md` 移除 `.ccd.env` 术语。

- [ ] **Step 3: 更新 workgraph 门禁说明**

`README.md`/`ARCHITECTURE.md`/`skills/driver-codecoder.md` 中关于 `bg_gate.rs`、命令门、review 门、`needs_fix`、`VERDICT` 行的描述改为"验收由独立里程碑节点承载，agent 自报完成"。`CLAUDE.md` 项目状态段同步。

- [ ] **Step 4: 更新 ADR**

`docs/adr/` 中引用 `.ccd.env`、`bg_gate`、`NeedsFix`、`MissionState::BlockedAt/CircuitBreaker/StuckNeedsFix` 的 ADR（0026/0028/0033/0034）补修订说明或新立 ADR 记录本次变更。

- [ ] **Step 5: 提交**

```bash
git add ARCHITECTURE.md README.md CONTEXT.md CLAUDE.md docs/adr/ skills/driver-codecoder.md
git commit -m "docs: sync bin names, 3-layer config, workgraph gate removal

Co-Authored-By: Claude <noreply@anthropic.com>"
```