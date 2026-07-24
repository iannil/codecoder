# 迭代 4：no-op 探索兜底 + footgun 清零 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 turn 循环加「连续 K 次纯探索 → 注入一次 steering nudge」的 no-op 兜底；`.ccd.env` 启动自动加载；headless 未 trusted 但存在 codecoder.json 时 stderr 引导（不放松 trust 门）。

**Architecture:** `AgentLoop` 在 turn 工具迭代循环内统计连续「纯探索」迭代（tool_calls 全 ∈ `EXPLORATION_TOOLS`），达阈值注入一条 `Role::User` nudge（每 turn 幂等）；阈值经 `Config`（env 注入到 `AgentLoop`，与迭代 2 ceiling 同法）。`.ccd.env` 由 `parse_dotenv`（纯）+ `autoload_ccd_env`（读文件、不覆盖已设 env）在 `Config::from_env()` 前调用。allowlist 引导由纯判定 `should_warn_untrusted_allowlist` + build 内 stderr 一次性提示。

**Tech Stack:** Rust（无新依赖）；hermetic 测试（有状态 Provider、临时文件、纯判定函数）。

## Global Constraints

- 不新增 crate 依赖。TDD；全 hermetic 测试。
- A 触发：turn 内连续 K 次「纯探索」迭代 → 注入**一次** nudge（`nudged_this_turn` 每 turn 幂等）。`EXPLORATION_TOOLS = ["read_file","glob","grep","diff"]`；一个迭代「纯探索」= tool_calls **非空且全部** ∈ 该集合。
- A 阈值：`CODECODER_NOOP_NUDGE_THRESHOLD` 默认 **3**，`0` = 禁用。经 `Config.noop_nudge_threshold`；`AgentLoop.noop_nudge_threshold` 在 `build` 内由 `Config::from_env()` 注入 + setter。
- B `.ccd.env`：仅当 env **未设置**时注入（显式 env 优先）；文件缺失静默；启动早期、`Config::from_env()` 之前。
- C：**不改** trust 门 / 权限语义；仅 headless+未 trusted+存在 codecoder.json 时 stderr 引导（进程内一次）。
- D：无代码，仅文档注记。
- 术语精确（CONTEXT.md）：turn / tool call / steering / trust / allowlist。

---

## 关键现状锚点

- `src/agent.rs:18` `const MAX_TOOL_ITERATIONS: usize = 12;`（放 `EXPLORATION_TOOLS` 于此附近）。
- `src/agent.rs:203` `AgentLoop` 字段区；`build`（283-358，已有一次 `Config::from_env()` 读 ceiling — iter2）；`set_tool_cap`（390）/`set_max_tokens_ceiling`（~400）setter 范式。
- `src/agent.rs:807-809` turn 循环起点（`let mut hit_tool_cap`；iter2 的 `let mut effective_max_tokens` 也在此）；`for _ in 0..self.tool_cap {`（~810）。
- `src/agent.rs:941-956` tool dispatch loop（`for (call_id, name, args) in tool_calls` 于 944 **移动** tool_calls；results 于 955 append）。`drain_steer`（750）追加 User 消息的范式。
- `src/config.rs:33` `from_env` 数值字段解析范式；`Config` 结构体。
- `src/main.rs:6` `fn main` 首行 `Config::from_env()`；`src/bin/cc.rs:8` `fn main` 首行 `Config::from_env()`。
- `src/trust.rs`：`has_config_resources`、`decide`、`default_trust`；`src/agent.rs` build 内 trust 解析（304-317，产出 `trust: TrustState`、`let trusted = trust == TrustState::Trusted;`）。

---

## Task 1: Config.noop_nudge_threshold

**Files:**
- Modify: `src/config.rs`（`Config` 字段 + `from_env` + 测试）

**Interfaces:**
- Produces: `Config.noop_nudge_threshold: usize`（env `CODECODER_NOOP_NUDGE_THRESHOLD`，默认 3）。

- [ ] **Step 1: 写失败测试**（`src/config.rs` `mod tests`）

```rust
#[test]
fn noop_nudge_threshold_default_and_override() {
    unsafe { std::env::remove_var("CODECODER_NOOP_NUDGE_THRESHOLD"); }
    assert_eq!(Config::from_env().noop_nudge_threshold, 3);
    unsafe { std::env::set_var("CODECODER_NOOP_NUDGE_THRESHOLD", "5"); }
    assert_eq!(Config::from_env().noop_nudge_threshold, 5);
    unsafe { std::env::remove_var("CODECODER_NOOP_NUDGE_THRESHOLD"); }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib config::tests::noop_nudge_threshold_default_and_override`
Expected: FAIL — `no field noop_nudge_threshold`。

- [ ] **Step 3: 加字段**（`Config` 结构体，`max_tokens_ceiling` 之后）

```rust
    /// no-op 探索兜底(迭代 4):单 turn 内连续多少个「纯探索」迭代后注入一次 steering nudge。
    /// 0 = 禁用。env CODECODER_NOOP_NUDGE_THRESHOLD,默认 3。
    pub noop_nudge_threshold: usize,
```

- [ ] **Step 4: 加解析**（`from_env`，`max_tokens_ceiling: …` 之后）

```rust
            noop_nudge_threshold: env("CODECODER_NOOP_NUDGE_THRESHOLD")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
```

- [ ] **Step 5: 编译修补其它 Config 字面量**

Run: `cargo build --tests`
Expected: 若 `src/daemon/mod.rs` 测试 Config 字面量报 missing field，补 `noop_nudge_threshold: 3,`。修到通过。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test --lib config::tests`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/config.rs src/daemon/mod.rs
git commit -m "feat(config): add CODECODER_NOOP_NUDGE_THRESHOLD (default 3)"
```

---

## Task 2: AgentLoop no-op 探索兜底

**Files:**
- Modify: `src/agent.rs`（`EXPLORATION_TOOLS` 常量、`AgentLoop` 字段、`build` 注入、setter、turn 循环逻辑、测试）

**Interfaces:**
- Consumes: `Config.noop_nudge_threshold`（Task 1）。
- Produces: `AgentLoop.noop_nudge_threshold: usize`；`pub fn set_noop_nudge_threshold(&mut self, n: usize)`；turn 内连续纯探索达阈值注入 nudge。

- [ ] **Step 1: 写失败测试**（`src/agent.rs` `mod tests`；`ScriptedTools` 有状态 Provider 驱动工具序列）

```rust
use std::sync::Mutex as StdMutex2;

/// 按脚本逐次产出工具调用或结束文本:Some(name)→ToolCall,None→纯文本(结束 turn)。
struct ScriptedTools {
    script: Vec<Option<&'static str>>,
    calls: StdMutex2<usize>,
}
impl Provider for ScriptedTools {
    fn name(&self) -> &str { "scripted-tools" }
    fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
        use crate::message::{Message, MessageItem, Role};
        let mut c = self.calls.lock().unwrap();
        let i = *c; *c += 1;
        let step = self.script.get(i).copied().flatten();
        let msg = match step {
            Some(name) => Message {
                id: 0, role: Role::Assistant,
                items: vec![MessageItem::ToolCall {
                    id: format!("t{i}"), name: name.to_string(),
                    args: serde_json::json!({"pattern": "*"}),
                }],
            },
            None => Message {
                id: 0, role: Role::Assistant,
                items: vec![MessageItem::Text { text: "done".into() }],
            },
        };
        Ok(msg.into())
    }
}

fn count_noop_notices(rx: std::sync::mpsc::Receiver<AgentEvent>) -> usize {
    rx.into_iter().filter(|e| matches!(e, AgentEvent::Notice(m) if m.contains("no-op backstop"))).count()
}

fn run_scripted(script: Vec<Option<&'static str>>, threshold: usize) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(ScriptedTools { script, calls: StdMutex2::new(0) });
    let mut agent = AgentLoop::new(p as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
    agent.set_noop_nudge_threshold(threshold);
    let (tx, rx) = std::sync::mpsc::channel();
    agent.run_one_turn("go".into(), &tx);
    drop(tx);
    count_noop_notices(rx)
}

#[test]
fn noop_backstop_nudges_after_threshold_explore_steps() {
    // glob×3 → 达阈值 3 → 恰一次 nudge;随后 text 结束。
    let n = run_scripted(vec![Some("glob"), Some("glob"), Some("glob"), None], 3);
    assert_eq!(n, 1, "expected exactly one no-op nudge, got {n}");
}

#[test]
fn noop_backstop_no_nudge_under_threshold() {
    let n = run_scripted(vec![Some("glob"), Some("glob"), None], 3);
    assert_eq!(n, 0);
}

#[test]
fn noop_backstop_disabled_when_threshold_zero() {
    let n = run_scripted(vec![Some("glob"), Some("glob"), Some("glob"), Some("glob"), None], 0);
    assert_eq!(n, 0);
}

#[test]
fn noop_backstop_resets_on_non_exploration_tool() {
    // glob,glob,milestone(重置),glob,glob → 连续从不达 3 → 不 nudge。
    let n = run_scripted(vec![Some("glob"), Some("glob"), Some("milestone"), Some("glob"), Some("glob"), None], 3);
    assert_eq!(n, 0);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib agent::tests::noop_backstop_nudges_after_threshold_explore_steps`
Expected: FAIL — `no method set_noop_nudge_threshold` / 无 nudge。

- [ ] **Step 3: 加 EXPLORATION_TOOLS 常量**（`src/agent.rs`，`MAX_TOOL_ITERATIONS`（18）附近）

```rust
/// 纯探索工具(迭代 4 no-op 兜底):只读/查、不推进交付物。turn 内连续多轮全是这些
/// → 注入一次 steering nudge。write_file/edit_file/run_command/commit/reason/milestone/
/// memory/plan 等都不在此集,算「动了」。
const EXPLORATION_TOOLS: &[&str] = &["read_file", "glob", "grep", "diff"];
```

- [ ] **Step 4: 加字段 + build 注入 + setter**（`src/agent.rs`）

字段（`AgentLoop`，`max_tokens_ceiling` 之后）：
```rust
    /// no-op 探索兜底阈值(迭代 4)。build 内由 Config::from_env() 注入;0 = 禁用。
    noop_nudge_threshold: usize,
```
build 初始化（`Self { … }`，`max_tokens_ceiling: …` 之后）：
```rust
            noop_nudge_threshold: crate::config::Config::from_env().noop_nudge_threshold,
```
setter（紧邻 `set_max_tokens_ceiling`）：
```rust
    /// 覆盖 no-op 兜底阈值(测试/特殊场景)。
    pub fn set_noop_nudge_threshold(&mut self, n: usize) {
        self.noop_nudge_threshold = n;
    }
```

- [ ] **Step 5: turn 循环内计数 + nudge**（`src/agent.rs`）

在 turn 循环起点（`let mut hit_tool_cap = true;` 附近，与 `effective_max_tokens` 同处）加：
```rust
        // no-op 探索兜底(迭代 4):统计连续「纯探索」迭代,达阈值注入一次 nudge。
        let mut consecutive_explore_iters = 0usize;
        let mut nudged_this_turn = false;
```

在 dispatch loop **之前**（`let mut results = Vec::new();` 那行之前，tool_calls 尚未被移动时）加：
```rust
            // 分类本迭代是否「纯探索」(tool_calls 非空且全部 ∈ EXPLORATION_TOOLS)。tool_calls
            // 随后在 dispatch 循环被移动,故先算。
            let all_exploration = !tool_calls.is_empty()
                && tool_calls.iter().all(|(_, name, _)| EXPLORATION_TOOLS.contains(&name.as_str()));
```

在 dispatch loop **之后**、`if cancelled { … }` 之前（results 已 append）加：
```rust
            // no-op 兜底:更新连续纯探索计数;达阈值且本 turn 未 nudge 过 → 注入一次 steering。
            if all_exploration {
                consecutive_explore_iters += 1;
            } else {
                consecutive_explore_iters = 0;
            }
            if self.noop_nudge_threshold > 0
                && consecutive_explore_iters >= self.noop_nudge_threshold
                && !nudged_this_turn
            {
                let n = self.noop_nudge_threshold;
                self.append(Role::User, vec![MessageItem::Text {
                    text: format!(
                        "You have only explored (read/glob/grep/diff) for {n} tool steps without \
                         making a change. Make a concrete edit or run a command now, or explicitly \
                         state that you are blocked and why."
                    ),
                }]);
                let _ = event_tx.send(AgentEvent::Notice(format!(
                    "no-op backstop: nudged to act after {n} exploration-only steps"
                )));
                nudged_this_turn = true;
            }
```

- [ ] **Step 6: 跑测试确认通过 + 无回归**

Run: `cargo test --lib agent::tests`
Expected: PASS — 4 个新测试绿；既有 agent 测试不回退。

- [ ] **Step 7: 全仓测试**

Run: `cargo test`
Expected: PASS。

- [ ] **Step 8: 提交**

```bash
git add src/agent.rs
git commit -m "feat(agent): in-turn no-op exploration backstop (steering nudge)"
```

---

## Task 3: .ccd.env 自动加载

**Files:**
- Modify: `src/config.rs`（`parse_dotenv`、`autoload_ccd_env_from`、`autoload_ccd_env`、测试）
- Modify: `src/main.rs`、`src/bin/cc.rs`（入口调用）
- Modify: `src/lib.rs`（re-export `autoload_ccd_env` 若需要）

**Interfaces:**
- Produces: `pub fn parse_dotenv(text: &str) -> Vec<(String, String)>`；`pub fn autoload_ccd_env_from(path: &std::path::Path) -> usize`；`pub fn autoload_ccd_env() -> usize`（解析 root 内部化）。

- [ ] **Step 1: 写失败测试**（`src/config.rs` `mod tests`）

```rust
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib config::tests::parse_dotenv_handles_comments_blank_quotes`
Expected: FAIL — `cannot find function parse_dotenv`。

- [ ] **Step 3: 实现三个函数**（`src/config.rs`，`Config` impl 外，文件顶层）

```rust
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
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    autoload_ccd_env_from(&root.join(".ccd.env"))
}
```

- [ ] **Step 4: 入口调用**（`src/main.rs` 与 `src/bin/cc.rs` 的 `fn main` 首行，`Config::from_env()` 之前）

`src/main.rs`：
```rust
fn main() -> anyhow::Result<()> {
    codecoder::config::autoload_ccd_env();
    let cfg = codecoder::Config::from_env();
    // …余不变
```
`src/bin/cc.rs`：
```rust
fn main() -> anyhow::Result<()> {
    codecoder::config::autoload_ccd_env();
    let cfg = Config::from_env();
    // …余不变
```
（若 `codecoder::config` 模块未 pub 导出，在 `src/lib.rs` 确认 `pub mod config;` 或加 `pub use config::autoload_ccd_env;` 后用短路径。以能编译为准。）

- [ ] **Step 5: 跑测试确认通过 + 编译入口**

Run: `cargo test --lib config::tests && cargo build`
Expected: PASS + 编译通过。

- [ ] **Step 6: 提交**

```bash
git add src/config.rs src/main.rs src/bin/cc.rs src/lib.rs
git commit -m "feat(config): autoload .ccd.env at startup (no-override semantics)"
```

---

## Task 4: allowlist 未加载引导（不放松门）

**Files:**
- Modify: `src/trust.rs`（`should_warn_untrusted_allowlist` + 测试）
- Modify: `src/agent.rs`（build 内 stderr 一次性引导）

**Interfaces:**
- Produces: `pub fn should_warn_untrusted_allowlist(root: &std::path::Path, trusted: bool, headless: bool) -> bool`。

- [ ] **Step 1: 写失败测试**（`src/trust.rs` `mod tests`）

```rust
#[test]
fn should_warn_untrusted_allowlist_truth_table() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // 无 codecoder.json → 任何情况都不提示。
    assert!(!should_warn_untrusted_allowlist(root, false, true));
    // 有 codecoder.json:
    std::fs::write(root.join("codecoder.json"), "{}").unwrap();
    assert!(should_warn_untrusted_allowlist(root, false, true));   // headless + 未 trusted + 有文件 → 提示
    assert!(!should_warn_untrusted_allowlist(root, true, true));    // 已 trusted → 不提示
    assert!(!should_warn_untrusted_allowlist(root, false, false));  // 交互 → 不提示
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib trust::tests::should_warn_untrusted_allowlist_truth_table`
Expected: FAIL — `cannot find function should_warn_untrusted_allowlist`。

- [ ] **Step 3: 实现纯判定**（`src/trust.rs`）

```rust
/// headless 且未 trusted 且磁盘存在 codecoder.json(有预授权 allowlist)→ 应 stderr 引导:
/// allowlist 未加载会导致预授权 Ask 工具被静默自动拒绝。仅提示,不放松 trust 门。
pub fn should_warn_untrusted_allowlist(root: &std::path::Path, trusted: bool, headless: bool) -> bool {
    headless && !trusted && root.join("codecoder.json").exists()
}
```

- [ ] **Step 4: build 内一次性引导**（`src/agent.rs` build，trust 解析后 `let trusted = trust == TrustState::Trusted;` 之后）

```rust
        if crate::trust::should_warn_untrusted_allowlist(&root, trusted, headless) {
            use std::sync::Once;
            static WARN_ONCE: Once = Once::new();
            WARN_ONCE.call_once(|| {
                eprintln!(
                    "ccd: codecoder.json found but project is untrusted → allowlist not loaded; \
                     every pre-authorized Ask tool will be auto-denied. Set \
                     CODECODER_DEFAULT_TRUST=always or add ~/.codecoder/trust.json to load it."
                );
            });
        }
```

- [ ] **Step 5: 跑测试确认通过 + 无回归**

Run: `cargo test --lib trust::tests && cargo test --lib agent::tests`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/trust.rs src/agent.rs
git commit -m "feat(trust): stderr guidance when untrusted project has codecoder.json allowlist"
```

---

## Task 5: 文档 + ADR 0029 修订 + D 注记

**Files:**
- Modify: `docs/adr/0029-turn-steering-and-follow-up.md`（追加 no-op 自发 nudge 修订）
- Modify: `README.md`（env 表加 `CODECODER_NOOP_NUDGE_THRESHOLD`；`.ccd.env` 说明）
- Modify: `ARCHITECTURE.md`（no-op steering + `.ccd.env` 自动加载）
- Modify: `CLAUDE.md`（D：并发写纪律注记；计数同步）
- Modify: `docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`（§4.2 标已治；§6.1/6.7/6.8 标已引导/已治/已文档化）

- [ ] **Step 1: 核对代码事实**

Run: `grep -n "noop_nudge_threshold\|EXPLORATION_TOOLS\|autoload_ccd_env\|should_warn_untrusted" src/agent.rs src/config.rs src/trust.rs | head`
Expected: 确认符号就位（供文档准确引用）。

- [ ] **Step 2: ADR 0029 追加修订段**

```markdown
## 修订（2026-07-24，迭代 4：no-op 探索兜底）

steering 除用户注入外,新增 **agent 自发 nudge**:turn 内连续 `CODECODER_NOOP_NUDGE_THRESHOLD`(默认 3,0=禁用)个「纯探索」迭代(tool_calls 全 ∈ read_file/glob/grep/diff)后,内核追加一条 User steering 消息推动动手或声明阻塞,并发 Notice。每 turn 至多一次。与迭代 1 自恢复叠加(turn 内先 nudge,仍失败再由 gate→自恢复)。
```

- [ ] **Step 3: README / ARCHITECTURE**

- README env 表新增：`| \`CODECODER_NOOP_NUDGE_THRESHOLD\` | \`3\` | 单 turn 连续多少「纯探索」步后注入一次 steering nudge（0=禁用，迭代 4）。|`；并在启动/配置说明处加一句「`ccd`/`cc` 启动时自动加载项目根的 `.ccd.env`（KEY=VALUE，不覆盖已设 env）」。
- ARCHITECTURE：turn 循环描述处补「no-op 兜底：连续纯探索达阈值注入 steering nudge」；启动描述补「`.ccd.env` 自动加载」。

- [ ] **Step 4: CLAUDE.md（D 注记）**

在 Background/daemon 段补一句编排纪律：「切忌向同一常驻 daemon **并发**发消息（共享 session 历史 + 异步写 → 版本竞争）；并发工作用独立 root/daemon 或串行化（ADR 0035 已护 workgraph 并发写）。」并同步测试计数（若引用）。

- [ ] **Step 5: 评估报告**

§4.2 末补 `（迭代 4 已治：no-op 探索兜底 nudge）`；§6 footgun：6.1 allowlist 标 `（迭代 4 已引导）`、6.7 `.ccd.env` 标 `（迭代 4 已自动加载）`、6.8 并发写标 `（迭代 4 已文档化编排纪律；ADR 0035 已护 workgraph）`。

- [ ] **Step 6: 全仓测试 + 数字核对**

Run: `cargo test 2>&1 | tail -3`
Expected: PASS。文档引用计数按实更新。

- [ ] **Step 7: 提交**

```bash
git add docs/adr/0029-turn-steering-and-follow-up.md README.md ARCHITECTURE.md CLAUDE.md docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md
git commit -m "docs: no-op backstop + .ccd.env autoload + footgun notes (ADR 0029, README, ARCHITECTURE)"
```

---

## Self-Review

- **Spec coverage**：A（config 阈值=Task 1；EXPLORATION_TOOLS + 循环兜底 + 字段/setter=Task 2）；B（`.ccd.env`=Task 3）；C（allowlist 引导=Task 4）；D（文档=Task 5）；文档/ADR=Task 5。测试覆盖 spec §4：A 触发/欠阈值/禁用/重置四态；B parse_dotenv 格式 + 注入/不覆盖/缺失；C 真值表。
- **Placeholder scan**：无 TBD；每改代码步骤给出完整代码。Task 3 Step 4 括注「以能编译为准」是对模块可见性的对齐提示（`pub mod config` 已存在），非需求占位。
- **Type consistency**：`Config.noop_nudge_threshold: usize` / `AgentLoop.noop_nudge_threshold` / `set_noop_nudge_threshold(usize)` / `EXPLORATION_TOOLS: &[&str]` / `parse_dotenv(&str)->Vec<(String,String)>` / `autoload_ccd_env_from(&Path)->usize` / `autoload_ccd_env()->usize` / `should_warn_untrusted_allowlist(&Path,bool,bool)->bool` 跨 Task 一致。
- **已知取舍**：A 分类用保守 4 工具白名单(启发式)；nudge 追加 User 消息复用既有 steering 语义(与 drain_steer 一致)；阈值 build 内 Config::from_env() 注入(与 iter2 ceiling 同法，接受一次额外 env 读)。
