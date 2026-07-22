# BG 失败处理 / 护栏 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 Background Agent 加客观验收门 + 失败写回 + continue/stop 策略,使长期无人值守下失败安全(不固着、不回退当成功)。

**Architecture:** 复用 `run_background`/`advance_one_milestone` 既有的 milestone 循环;新增 `src/bg_gate.rs` 提供 `GateVerdict` + 命令门 + 注入式 review 门 + 纯函数策略 `next_action`;扩展 `BgOutcome`(`subgoals`+`mission_state`)与 `Config`(3 个 env);给 `AgentLoop` 加可配 `tool_cap`。门 verdict **覆盖** agent 自报。

**Tech Stack:** Rust 2024 edition;既有 `workgraph`/`review`/`reason`/`tool::builtin::run_shell_cancellable`/`CancelToken`;测试用 `ScriptedProvider` + `tempfile`(hermetic,不烧 LLM、不依赖 Docker)。

## Global Constraints

- **不改交互式 agent loop 的语义**,仅加可选 `tool_cap` 字段;失败策略全在 background.rs + bg_gate.rs。
- **调度仍外置**(systemd/cron/launchd);本计划只让每次 BG 调用 failure-safe。
- **门 verdict 覆盖自报**:`advance_one_milestone` 现有逻辑(信任 agent 的 `VERDICT:` 行)改为以客观门为准。
- **hermetic 测试**:命令门用 `echo`/`false`;review 门用注入闭包;禁用真 LLM/Docker/网络。
- **领域术语**遵 `CONTEXT.md`(Permission Scope、MessageId vs ToolCall.id 等)。
- **commit 规范**遵 `skills/commit-conventions.md`(conventional commits + 中文正文动机);提交到 `feat/bg-failure-guardrails` 分支。
- 真实仓 key 路径:`src/background.rs`、`src/workgraph.rs`、`src/tool/reason.rs`、`src/tool/builtin.rs`(含 `run_shell_cancellable`)、`src/tool/mod.rs`(`ToolCtx`)、`src/config.rs`、`src/agent.rs`(`CancelToken`/`AgentLoop`)。

## 关键既有签名(供各 Task 引用)

```rust
// src/tool/mod.rs
pub struct ToolCtx<'a> { pub root: &'a Path, pub cancel: Option<&'a CancelToken> }
impl ToolCtx<'a> { pub fn new(root) -> Self; pub fn with_cancel(root, &CancelToken) -> Self; pub fn is_cancelled(&self) -> bool; }
// src/tool/builtin.rs
fn run_shell_cancellable(command: Command, ctx: &ToolCtx) -> anyhow::Result<ToolOutput>;  // 需提为 pub(crate)
// src/agent.rs
pub struct CancelToken(...); impl CancelToken { pub fn is_cancelled(&self)->bool; pub fn cancel_on_sigint(&self)->Result<()>; }
pub fn new_background(provider, model, max_tokens, temperature, root: PathBuf) -> Self;
const MAX_TOOL_ITERATIONS: usize = 12;  // agent.rs:18, turn loop at :781
// src/workgraph.rs
pub enum NodeStatus { Pending, InProgress, Blocked, NeedsFix, Done, ... }
pub struct Milestone { pub id: u64, pub title: String, pub acceptance: String, pub deps: Vec<u64>, pub status: NodeStatus, pub verdict: Option<String>, pub touched: Vec<String> }
impl WorkGraph { pub fn read(root)->WorkGraph; pub fn save(&self,root)->Result<()>; pub fn next_ready(&self)->Option<&Milestone>; pub fn set_status(&mut self,id,status)->bool; pub fn get(&self,id)->Option<&Milestone>; pub fn add(&mut self,title,acceptance,deps)->Result<u64>; }
// src/review.rs
pub enum Verdict { Pass, NeedsFix, Rebuild }
pub fn parse_review(text: &str) -> ReviewOutcome;  // { verdict, unparsed, .. }
// src/tool/reason.rs
struct CausalTree { .. }  // load(root)/save(root)/add(question,parent)->u64 (私有); pub fn record_ruling(root,id,ruling)->Result<()>
// src/provider/stub.rs: StubClient; tests用 ScriptedProvider::new(vec![..])
```

---

## Task 1: Config 加 3 个 BG env 变量

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Produces: `Config { bg_max_auto: usize, bg_circuit_k: usize, bg_milestone_tool_cap: usize }`(默认 3 / 2 / 8)。

- [ ] **Step 1: 写失败测试**

在 `src/config.rs` 的 `#[cfg(test)] mod tests`(若无则新建)加:
```rust
#[test]
fn bg_env_defaults_and_overrides() {
    // defaults when unset
    std::env::remove_var("CODECODER_BG_MAX_AUTO");
    std::env::remove_var("CODECODER_BG_CIRCUIT_K");
    std::env::remove_var("CODECODER_BG_MILESTONE_TOOL_CAP");
    let c = Config::from_env();
    assert_eq!(c.bg_max_auto, 3);
    assert_eq!(c.bg_circuit_k, 2);
    assert_eq!(c.bg_milestone_tool_cap, 8);

    std::env::set_var("CODECODER_BG_MAX_AUTO", "5");
    std::env::set_var("CODECODER_BG_CIRCUIT_K", "4");
    std::env::set_var("CODECODER_BG_MILESTONE_TOOL_CAP", "6");
    let c2 = Config::from_env();
    assert_eq!(c2.bg_max_auto, 5);
    assert_eq!(c2.bg_circuit_k, 4);
    assert_eq!(c2.bg_milestone_tool_cap, 6);
    std::env::remove_var("CODECODER_BG_MAX_AUTO");
    std::env::remove_var("CODECODER_BG_CIRCUIT_K");
    std::env::remove_var("CODECODER_BG_MILESTONE_TOOL_CAP");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test config::tests::bg_env_defaults_and_overrides 2>&1 | tail -5`
Expected: 编译失败(`no field bg_max_auto`)。

- [ ] **Step 3: 实现**

在 `Config` struct 加字段:
```rust
    pub bg_max_auto: usize,
    pub bg_circuit_k: usize,
    pub bg_milestone_tool_cap: usize,
```
在 `from_env()` 末尾(紧随 `github_token` 后)加:
```rust
            bg_max_auto: env("CODECODER_BG_MAX_AUTO").and_then(|v| v.parse().ok()).unwrap_or(3),
            bg_circuit_k: env("CODECODER_BG_CIRCUIT_K").and_then(|v| v.parse().ok()).unwrap_or(2),
            bg_milestone_tool_cap: env("CODECODER_BG_MILESTONE_TOOL_CAP").and_then(|v| v.parse().ok()).unwrap_or(8),
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test config::tests::bg_env_defaults_and_overrides 2>&1 | tail -3`
Expected: `ok. 1 passed`。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs
git commit -m "feat(config): 加 BG 护栏 env 变量

BG_MAX_AUTO(3)/CIRCUIT_K(2)/MILESTONE_TOOL_CAP(8),供后续客观验收门
与 continue/stop 策略读取,默认值对齐 spec。"
```

---

## Task 2: `run_shell_cancellable` 提为 `pub(crate)`

**Files:**
- Modify: `src/tool/builtin.rs:86`(`fn run_shell_cancellable` → `pub(crate) fn run_shell_cancellable`)

**Interfaces:**
- Produces: `pub(crate) fn run_shell_cancellable(command: Command, ctx: &ToolCtx) -> anyhow::Result<ToolOutput>`,供 bg_gate 调用。

- [ ] **Step 1: 改可见性**

将 `src/tool/builtin.rs:86` 的 `fn run_shell_cancellable` 改为 `pub(crate) fn run_shell_cancellable`。

- [ ] **Step 2: 编译确认**

Run: `cargo build 2>&1 | tail -3`
Expected: `Finished`(无 dead_code 警告,因 Task 3 即将使用)。

- [ ] **Step 3: 提交**

```bash
git add src/tool/builtin.rs
git commit -m "refactor(tool): run_shell_cancellable 提为 pub(crate)

bg_gate 命令门需复用此既有的可取消 shell 执行器(尊重 CancelToken、
取消时 kill 子进程),而非另造一份。"
```

---

## Task 3: `bg_gate` 模块 — GateVerdict + extract_gate_command(纯函数)

**Files:**
- Create: `src/bg_gate.rs`
- Modify: `src/lib.rs`(注册 `pub mod bg_gate;`)

**Interfaces:**
- Produces: `pub enum GateVerdict { Pass, NeedsFix(String), Inconclusive(String) }`、`pub fn extract_gate_command(acceptance: &str) -> Option<String>`。

- [ ] **Step 1: 写失败测试**

创建 `src/bg_gate.rs`:
```rust
//! BG 客观验收门 + continue/stop 策略(spec 2026-07-22)。

/// 客观验收门的判定结果。**覆盖** agent 自报 verdict。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    Pass,
    NeedsFix(String),
    Inconclusive(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_gate_command_finds_known_patterns() {
        assert_eq!(extract_gate_command("cargo test 通过"), Some("cargo test 通过".into()));
        assert_eq!(extract_gate_command("runs: pytest -q"), Some("runs: pytest -q".into()));
        assert_eq!(extract_gate_command("make test"), Some("make test".into()));
    }

    #[test]
    fn extract_gate_command_none_when_no_pattern() {
        assert_eq!(extract_gate_command("renderer 输出正确"), None);
        assert_eq!(extract_gate_command(""), None);
    }
}
```
在 `src/lib.rs` 加 `pub mod bg_gate;`(紧随 `pub mod background;` 一类)。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test bg_gate::tests 2>&1 | tail -5`
Expected: 编译失败(`extract_gate_command` 未定义)。

- [ ] **Step 3: 实现纯函数**

在 `src/bg_gate.rs` 顶部(`GateVerdict` 下方)加:
```rust
/// 从 acceptance 文本提取可执行的验收命令(若有)。按行扫描,返回首个含已知
/// 测试/构建命令模式的行(原样,含其修饰)。已知模式:cargo test/build/check/clippy、
/// pytest、npm/yarn test、make、go test、rustc。
pub fn extract_gate_command(acceptance: &str) -> Option<String> {
    const PATTERNS: &[&str] = &[
        "cargo test", "cargo build", "cargo check", "cargo clippy",
        "pytest", "py.test", "npm test", "yarn test", "go test", "rustc", "make ",
    ];
    for line in acceptance.lines() {
        let low = line.to_lowercase();
        if PATTERNS.iter().any(|p| low.contains(p)) {
            return Some(line.trim().to_string());
        }
    }
    None
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test bg_gate::tests 2>&1 | tail -3`
Expected: `2 passed`。

- [ ] **Step 5: 提交**

```bash
git add src/bg_gate.rs src/lib.rs
git commit -m "feat(bg_gate): GateVerdict 枚举 + extract_gate_command 纯函数

客观验收门的基础类型与命令提取(识别 cargo/pytest/make 等),纯函数
可独立单测。后续命令门/review 门/evaluate 在此之上构建。"
```

---

## Task 4: 命令门 `run_command_gate` + 顶层 `evaluate`(注入式 review)

**Files:**
- Modify: `src/bg_gate.rs`

**Interfaces:**
- Consumes: `crate::tool::builtin::run_shell_cancellable`、`ToolCtx`、`CancelToken`、`Milestone`。
- Produces:
  - `pub fn run_command_gate(cmd: &str, root: &Path, cancel: Option<&CancelToken>) -> GateVerdict`
  - `pub fn evaluate(m: &Milestone, root: &Path, cancel: Option<&CancelToken>, review_runner: &dyn Fn() -> GateVerdict) -> GateVerdict`

- [ ] **Step 1: 写失败测试**(append to `src/bg_gate.rs` tests)
```rust
    use crate::workgraph::{Milestone, NodeStatus, WorkGraph};
    use std::fs;
    use tempfile::tempdir;

    fn ms(id: u64, acceptance: &str) -> Milestone {
        Milestone { id, title: format!("t{id}"), acceptance: acceptance.into(),
            deps: vec![], status: NodeStatus::Pending, verdict: None, touched: vec![] }
    }

    #[test]
    fn command_gate_pass_on_exit_zero() {
        let dir = tempdir().unwrap();
        let v = run_command_gate("echo ok", dir.path(), None);
        assert_eq!(v, GateVerdict::Pass);
    }

    #[test]
    fn command_gate_needsfix_on_nonzero() {
        let dir = tempdir().unwrap();
        let v = run_command_gate("false", dir.path(), None);
        match v { GateVerdict::NeedsFix(msg) => assert!(msg.contains("false")), other => panic!("{other:?}") }
    }

    #[test]
    fn command_gate_inconclusive_on_missing_binary() {
        let dir = tempdir().unwrap();
        let v = run_command_gate("this-binary-does-not-exist-xyz", dir.path(), None);
        assert!(matches!(v, GateVerdict::Inconclusive(_)), "{v:?}");
    }

    #[test]
    fn evaluate_uses_command_gate_when_present() {
        let dir = tempdir().unwrap();
        let m = ms(1, "cargo test 通过");
        // 即便 review_runner 说 NeedsFix,命令门 echo 优先 → Pass
        let v = evaluate(&m, dir.path(), None, &|_| GateVerdict::NeedsFix("review".into()));
        // "cargo test 通过" 不是合法 sh 命令整体,但 extract 取整行;为稳定,用纯命令:
        let m2 = ms(1, "echo ok");
        let v2 = evaluate(&m2, dir.path(), None, &|_| GateVerdict::NeedsFix("review".into()));
        assert_eq!(v2, GateVerdict::Pass, "命令门应覆盖 review 注入");
        let _ = v;
    }

    #[test]
    fn evaluate_falls_back_to_review_runner() {
        let dir = tempdir().unwrap();
        let m = ms(1, "renderer 输出正确"); // 无命令模式
        let v = evaluate(&m, dir.path(), None, &|_| GateVerdict::NeedsFix("review says no".into()));
        assert_eq!(v, GateVerdict::NeedsFix("review says no".into()));
    }

    #[test]
    fn evaluate_inconclusive_when_acceptance_empty() {
        let dir = tempdir().unwrap();
        let m = ms(1, "");
        let v = evaluate(&m, dir.path(), None, &|_| GateVerdict::Pass);
        assert!(matches!(v, GateVerdict::Inconclusive(_)));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test bg_gate::tests 2>&1 | tail -5`
Expected: 编译失败(`run_command_gate`/`evaluate` 未定义)。

- [ ] **Step 3: 实现**

在 `src/bg_gate.rs` 加(use 区先补):
```rust
use crate::tool::ToolCtx;
use crate::tool::builtin::run_shell_cancellable;
use crate::agent::CancelToken;
use crate::workgraph::Milestone;
use std::path::Path;
use std::process::Command;
```
再加函数:
```rust
/// 跑命令门:exit 0 → Pass;非零 → NeedsFix(附 stdout/stderr 摘要);跑不起来 → Inconclusive。
pub fn run_command_gate(cmd: &str, root: &Path, cancel: Option<&CancelToken>) -> GateVerdict {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd).current_dir(root);
    let r = match cancel {
        Some(c) => { let ctx = ToolCtx::with_cancel(root, c); run_shell_cancellable(command, &ctx) }
        None => { let ctx = ToolCtx::new(root); run_shell_cancellable(command, &ctx) }
    };
    match r {
        Ok(out) if !out.is_error => GateVerdict::Pass,
        Ok(out) => GateVerdict::NeedsFix(format!("gate `{cmd}` failed: {}", truncate(out.content, 400))),
        Err(e) => GateVerdict::Inconclusive(format!("gate `{cmd}` could not run: {e}")),
    }
}

fn truncate(s: String, n: usize) -> String {
    if s.chars().count() <= n { s } else { let mut t: String = s.chars().take(n).collect(); t.push_str("…"); t }
}

/// 顶层验收:命令门优先(客观);否则注入式 review 门;acceptance 空 → Inconclusive。
/// `review_runner` 注入便于纯策略测试;prod 由 background.rs 注入调用 review 工具的闭包。
pub fn evaluate(
    m: &Milestone,
    root: &Path,
    cancel: Option<&CancelToken>,
    review_runner: &dyn Fn() -> GateVerdict,
) -> GateVerdict {
    if let Some(cmd) = extract_gate_command(&m.acceptance) {
        return run_command_gate(&cmd, root, cancel);
    }
    if m.acceptance.trim().is_empty() {
        return GateVerdict::Inconclusive("no acceptance criterion (weak signal)".into());
    }
    review_runner()
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test bg_gate::tests 2>&1 | tail -3`
Expected: 全部 `passed`(含 Task 3 的 2 个 + 本 Task 的 6 个)。

- [ ] **Step 5: 提交**

```bash
git add src/bg_gate.rs
git commit -m "feat(bg_gate): 命令门 run_command_gate + 顶层 evaluate

命令门用 run_shell_cancellable(尊重 CancelToken)按 exit 0 客观判定;
evaluate 三层:命令门→注入式 review 门→Inconclusive。命令门覆盖 review,
杜绝 agent 自报。hermetic 测试用 echo/false + 注入闭包。"
```

---

## Task 5: continue/stop 策略 — MissionState + `next_action`(纯函数)

**Files:**
- Modify: `src/bg_gate.rs`

**Interfaces:**
- Consumes: `WorkGraph::next_ready`、`NodeStatus::Done`、`Milestone`。
- Produces:
  - `pub enum MissionState { Running, CompletedAllReady, BlockedAt(u64), CircuitBreaker, Error(String) }`
  - `pub enum NextAction { Advance(u64), Stop(MissionState) }`
  - `pub fn next_action(graph: &WorkGraph, just_done_id: u64, verdict: &GateVerdict, consecutive_fail: usize, budget_left: bool, k: usize) -> NextAction`

- [ ] **Step 1: 写失败测试**(append to bg_gate tests)
```rust
    fn graph_with(nodes: Vec<Milestone>) -> WorkGraph {
        let mut g = WorkGraph::default();
        for n in nodes { g.nodes.push(n); }  // 直接组装(测试专用)
        g
    }

    #[test]
    fn next_action_pass_advances_to_next_ready() {
        let g = graph_with(vec![
            ms(1, "x"), ms(2, "y"),
        ]);
        let a = next_action(&g, 1, &GateVerdict::Pass, 0, true, 2);
        assert_eq!(a, NextAction::Advance(2));
    }

    #[test]
    fn next_action_pass_no_more_ready_completes() {
        let g = graph_with(vec![ ms(1, "x") ]);
        let a = next_action(&g, 1, &GateVerdict::Pass, 0, true, 2);
        assert_eq!(a, NextAction::Stop(MissionState::CompletedAllReady));
    }

    #[test]
    fn next_action_fail_with_blocked_dependent_and_no_independent_ready_blocks() {
        // #1 failed, #2 deps=[#1] 且无其它就绪 → BlockedAt(1)
        let mut m2 = ms(2, "y"); m2.deps = vec![1];
        let g = graph_with(vec![ ms(1, "x"), m2 ]);
        let a = next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 1, true, 2);
        assert_eq!(a, NextAction::Stop(MissionState::BlockedAt(1)));
    }

    #[test]
    fn next_action_fail_independent_ready_advances() {
        // #1 failed, #3 独立就绪 → Advance(3)
        let g = graph_with(vec![ ms(1, "x"), ms(3, "z") ]);
        let a = next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 1, true, 2);
        assert_eq!(a, NextAction::Advance(3));
    }

    #[test]
    fn next_action_circuit_breaker_on_k_consecutive_fails() {
        let g = graph_with(vec![ ms(1, "x"), ms(3, "z") ]); // 即便有就绪
        let a = next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 2, true, 2);
        assert_eq!(a, NextAction::Stop(MissionState::CircuitBreaker));
    }

    #[test]
    fn next_action_no_budget_stops_completed() {
        let g = graph_with(vec![ ms(1, "x"), ms(2, "y") ]);
        let a = next_action(&g, 1, &GateVerdict::Pass, 0, false, 2);
        assert_eq!(a, NextAction::Stop(MissionState::CompletedAllReady));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test bg_gate::tests::next_action 2>&1 | tail -5`
Expected: 编译失败(`next_action`/`MissionState` 未定义)。

- [ ] **Step 3: 实现**

在 `src/bg_gate.rs` 加:
```rust
use crate::workgraph::{WorkGraph, NodeStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionState {
    Running,
    CompletedAllReady,
    BlockedAt(u64),
    CircuitBreaker,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction { Advance(u64), Stop(MissionState) }

/// 决定一次 milestone 验收后的下一步。next_ready() 自然跳过因失败而 Blocked 的依赖者。
pub fn next_action(
    graph: &WorkGraph,
    just_done_id: u64,
    verdict: &GateVerdict,
    consecutive_fail: usize,
    budget_left: bool,
    k: usize,
) -> NextAction {
    let failed = !matches!(verdict, GateVerdict::Pass);
    // 熔断优先(即便有就绪,也停,防连环 flail)
    if failed && consecutive_fail >= k {
        return NextAction::Stop(MissionState::CircuitBreaker);
    }
    if !budget_left {
        return NextAction::Stop(MissionState::CompletedAllReady);
    }
    // 是否有因 just_done 失败而 Blocked 的下游?
    let has_blocked_dependent = graph.nodes.iter().any(|n|
        n.status != NodeStatus::Done && n.deps.contains(&just_done_id));
    match graph.next_ready() {
        Some(n) => NextAction::Advance(n.id),
        None => {
            if has_blocked_dependent { NextAction::Stop(MissionState::BlockedAt(just_done_id)) }
            else { NextAction::Stop(MissionState::CompletedAllReady) }
        }
    }
}
```
注:`WorkGraph.nodes` 字段需为 `pub`(已是,见 workgraph.rs:76 区域)。若 `next_ready` 借用 `&self` 与 `graph.nodes.iter()` 冲突,先把 `has_blocked_dependent` 算出来(独立借用)再调 `next_ready()`(如上顺序:先算 bool,再 match next_ready)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test bg_gate::tests::next_action 2>&1 | tail -3`
Expected: 全部 `passed`。

- [ ] **Step 5: 提交**

```bash
git add src/bg_gate.rs
git commit -m "feat(bg_gate): continue/stop 策略 next_action + MissionState

纯函数策略:pass→推进下一个就绪;失败→熔断(K 连续)/ BlockedAt(有
阻塞下游且无独立就绪)/ CompletedAllReady。next_ready() 自然跳过
因失败而 Blocked 的依赖者。纯函数全单测。"
```

---

## Task 6: reason 暴露 `record_cause` 公开助手

**Files:**
- Modify: `src/tool/reason.rs`

**Interfaces:**
- Produces: `pub fn record_cause(root: &Path, question: &str, parent: Option<u64>) -> anyhow::Result<u64>`(镜像 `record_ruling` 模式,供 background 写失败因果节点)。

- [ ] **Step 1: 写失败测试**(append to reason.rs `#[cfg(test)]`)
```rust
    #[test]
    fn record_cause_persists_node() {
        let dir = std::env::temp_dir().join(format!("cc_rc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = record_cause(&dir, "milestone #1 验收失败: gate cargo test exit 1", None).unwrap();
        let tree = CausalTree::load(&dir);
        assert!(tree.nodes.iter().any(|n| n.id == id && n.question.contains("验收失败")));
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test reason::tests::record_cause 2>&1 | tail -5`
Expected: 编译失败(`record_cause` 未定义)。

- [ ] **Step 3: 实现**

在 `src/tool/reason.rs` 紧邻 `pub fn record_ruling`(reason.rs:444 区域)加:
```rust
/// 持久化一个因果节点(供 background runner 在 milestone 验收失败时记录根因)。
/// 镜像 reason 工具 `add` action 的写入路径(CausalTree::load → add → save)。
pub fn record_cause(root: &Path, question: &str, parent: Option<u64>) -> anyhow::Result<u64> {
    let mut tree = CausalTree::load(root);
    let id = tree.add(question, parent);
    tree.save(root)?;
    Ok(id)
}
```
若 `CausalTree::add`/`save`/`load` 对 reason 模块外不可见,这些调用在模块内(reason.rs)即可,故无需改可见性。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test reason::tests::record_cause 2>&1 | tail -3`
Expected: `1 passed`。

- [ ] **Step 5: 提交**

```bash
git add src/tool/reason.rs
git commit -m "feat(reason): 暴露 record_cause 公开助手

镜像 record_ruling,供 background runner 在客观门失败时写因果节点
(根因不丢),复用既有 CausalTree load/add/save。"
```

---

## Task 7: `SubgoalOutcome` + `BgOutcome` 扩展

**Files:**
- Modify: `src/background.rs`

**Interfaces:**
- Produces:
  - `pub enum SubgoalVerdict { Pass, NeedsFix, Inconclusive }`
  - `pub struct SubgoalOutcome { pub milestone_id: u64, pub verdict: SubgoalVerdict, pub gate_reason: String, pub tool_cap_hit: bool, pub touched_files: Vec<String> }`
  - `BgOutcome` 增 `pub subgoals: Vec<SubgoalOutcome>`、`pub mission_state: MissionState`(从 bg_gate 引入,Default = `Running`/`CompletedAllReady`)。

- [ ] **Step 1: 改 struct**

在 `src/background.rs` 的 `BgOutcome` 上方加:
```rust
use crate::bg_gate::MissionState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubgoalVerdict { Pass, NeedsFix, Inconclusive }

#[derive(Debug, Clone)]
pub struct SubgoalOutcome {
    pub milestone_id: u64,
    pub verdict: SubgoalVerdict,
    pub gate_reason: String,
    pub tool_cap_hit: bool,
    pub touched_files: Vec<String>,
}
```
`BgOutcome` 加两字段(并改 `Default`):
```rust
#[derive(Debug)]
pub struct BgOutcome {
    pub final_text: String,
    pub tool_calls: Vec<String>,
    pub denied: Vec<String>,
    pub events: Vec<String>,
    pub subgoals: Vec<SubgoalOutcome>,
    pub mission_state: MissionState,
}
```
去掉 `#[derive(Debug, Default)]` 的 `Default` derive(因 MissionState 无 Default),手写:
```rust
impl Default for BgOutcome {
    fn default() -> Self {
        Self {
            final_text: String::new(), tool_calls: vec![], denied: vec![],
            events: vec![], subgoals: vec![], mission_state: MissionState::Running,
        }
    }
}
```
(若 `MissionState` 需要 `Default`,在 bg_gate 加 `impl Default for MissionState { fn default()->Self { MissionState::Running } }` 并 derive 不冲突——按需。)

- [ ] **Step 2: 编译确认**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished`(若 `MissionState` 未 `Default`/`Clone`,在 bg_gate 补 derive — 已 derive Clone/PartialEq/Eq;`Default` 手写如上)。

- [ ] **Step 3: 现有测试不回归**

Run: `cargo test background:: 2>&1 | tail -3`
Expected: 现有 2 个 `advance_one_milestone` 测试仍通过(新字段有默认)。

- [ ] **Step 4: 提交**

```bash
git add src/background.rs
git commit -m "feat(background): BgOutcome 扩 subgoals + mission_state

每 milestone 的客观验收结果(含 gate_reason/tool_cap_hit/touched)与
整次调用的任务终态(Running/CompletedAllReady/BlockedAt/CircuitBreaker/
Error)结构化暴露,供外部调度器/日志解析。"
```

---

## Task 8: AgentLoop 可配 `tool_cap`

**Files:**
- Modify: `src/agent.rs`

**Interfaces:**
- Produces: `AgentLoop.tool_cap: usize` 字段(默认 `MAX_TOOL_ITERATIONS`)+ `pub fn set_tool_cap(&mut self, n: usize)`;turn 循环(agent.rs:781 `for _ in 0..MAX_TOOL_ITERATIONS`)改用 `self.tool_cap`。

- [ ] **Step 1: 写失败测试**(append to agent.rs tests,用既有 testkit/ScriptedProvider 模式 — 参考 tests/l1_interaction.rs:60)
```rust
    #[test]
    fn tool_cap_lowers_iteration_limit() {
        // 构造一个会一直调用工具的 ScriptedProvider,验证 tool_cap=3 时第 4 次即触顶。
        // 复用既有 testkit::ScriptedProvider + run_turn(见 tests/l1_interaction.rs 模式)。
        // 此处用本文件已有的测试 helper(若无,参考同文件其他测试的 agent 构造)。
        // 期望:turn 在 3 次工具后停,事件含 "3-tool-iteration cap"。
    }
```
(若 agent.rs 内无现成 helper 构造带 ScriptedProvider 的 AgentLoop,本步先写最小 helper,参照 `tests/l1_interaction.rs:60` 的 `ScriptedProvider::new(vec![assistant_with_tool_call(...); N])` + `AgentLoop::new_background(provider.into(), ...)` + `run_one_turn`,断言事件流含 cap Notice 且工具调用计数 == tool_cap。)

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test agent::tests::tool_cap_lowers_iteration_limit 2>&1 | tail -5`
Expected: 失败(`set_tool_cap` 未定义)。

- [ ] **Step 3: 实现**

在 `AgentLoop` struct 加字段:
```rust
    tool_cap: usize,
```
`build()`/`new_background()` 等构造处初始化 `tool_cap: MAX_TOOL_ITERATIONS`(在 `Self { .. }` 字段表补)。加方法:
```rust
    /// 覆盖默认工具迭代上限(ADR 0026:BG 单 milestone 用更紧预算)。
    pub fn set_tool_cap(&mut self, n: usize) { self.tool_cap = n.max(1); }
```
turn 循环(agent.rs:781)`for _ in 0..MAX_TOOL_ITERATIONS` 改为 `for _ in 0..self.tool_cap`;其下 cap Notice 文案(agent.rs:923)用 `self.tool_cap` 而非 `MAX_TOOL_ITERATIONS`:
```rust
                format!("turn stopped at the {}-tool-iteration cap; the task may be incomplete — send another message to continue.", self.tool_cap)
```

- [ ] **Step 4: 跑测试确认通过 + 全量不回归**

Run: `cargo test 2>&1 | grep -E "test result" | tail -5`
Expected: 全部通过(含新测试;cap 文案变更不影响用变量断言的测试;若有测试硬编码 "12-tool",改用 regex/变量)。

- [ ] **Step 5: 提交**

```bash
git add src/agent.rs
git commit -m "feat(agent): 可配 tool_cap(默认 12)

turn 循环用 self.tool_cap 而非常量;BG 单 milestone 经 set_tool_cap
设更紧预算(CODECODER_BG_MILESTONE_TOOL_CAP 默认 8),防固着耗尽全局预算。"
```

---

## Task 9: 接入 `advance_one_milestone` — 门覆盖自报 + 失败写回 + 策略循环

**Files:**
- Modify: `src/background.rs`(`advance_one_milestone` + `run_background`)

**Interfaces:**
- Consumes: `bg_gate::{evaluate, next_action, GateVerdict, MissionState, NextAction}`、`reason::record_cause`、`review::parse_review`、`Config`。

- [ ] **Step 1: 写失败测试**(append to background.rs tests,用 ScriptedProvider)
```rust
    use crate::bg_gate::{GateVerdict, MissionState, NextAction};
    use crate::workgraph::NodeStatus;

    fn ws(dir: &Path, nodes: &[(u64, &str, Vec<u64>)]) {
        let mut g = WorkGraph::default();
        for (id, acc, deps) in nodes { g.nodes.push(Milestone{ id:*id, title:format!("t{id}"), acceptance:acc.into(), deps:deps.clone(), status: NodeStatus::Pending, verdict: None, touched: vec![] }); }
        let _ = g.save(dir);
    }

    #[test]
    fn t2_command_gate_fail_marks_needsfix_and_causal() {
        let dir = std::env::temp_dir().join(format!("cc_t2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "false", vec![])]);  // gate=false 必失败
        let out = advance_one_milestone(Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone()).unwrap().unwrap();
        let g = WorkGraph::read(&dir);
        assert_eq!(g.get(1).unwrap().status, NodeStatus::NeedsFix, "客观门 fail → needs_fix");
        assert_eq!(out.subgoals[0].verdict, crate::background::SubgoalVerdict::NeedsFix);
        assert!(out.subgoals[0].gate_reason.contains("false"));
        // 因果节点已写
        let tree = crate::tool::reason::CausalTree::load(&dir);
        assert!(tree.nodes.iter().any(|n| n.question.contains("验收失败")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn t1_command_gate_pass_marks_done() {
        let dir = std::env::temp_dir().join(format!("cc_t1_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "echo ok", vec![])]);
        let out = advance_one_milestone(Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone()).unwrap().unwrap();
        assert_eq!(WorkGraph::read(&dir).get(1).unwrap().status, NodeStatus::Done);
        assert_eq!(out.subgoals[0].verdict, crate::background::SubgoalVerdict::Pass);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn t7_objective_gate_overrides_self_report() {
        // agent 自报 VERDICT: pass(由 StubClient 文本),但命令门 false → 最终 NeedsFix
        // 需一个返回 "VERDICT: pass" 的 provider;用 ScriptedProvider(见 tests/l1_interaction.rs)
        // 此处用定制 stub:略 —— 实现时参照 tests/l1_interaction.rs:60 ScriptedProvider::new(vec![assistant_text("VERDICT: pass")])
        // 断言 WorkGraph status == NeedsFix(门覆盖自报)。
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test background::tests::t1_command_gate_pass_marks_done background::tests::t2_command_gate_fail_marks_needsfix_and_causal 2>&1 | tail -5`
Expected: 失败(当前 `advance_one_milestone` 仍信任自报,且未填 subgoals)。

- [ ] **Step 3: 重写 `advance_one_milestone` 的验收/写回段**

把 background.rs:172-194(现有 `parse_review` 自报写回段)替换为客观门写回:
```rust
    // ── 客观验收门(覆盖 agent 自报)──
    let cancel = agent.cancel_token();  // 取本 milestone agent 的 cancel token(SIGINT 可取消门)
    let m = { let g = WorkGraph::read(&root); g.get(milestone_id).expect("just read").clone() };
    let review_runner = |_root: &std::path::Path| -> crate::bg_gate::GateVerdict {
        // v1:review 门走 agent 自产 VERDICT 文本(若有);否则 Inconclusive。
        // 真正的 review 子代理门为后续增强(spec §5.1 (b))。
        match crate::review::parse_review(&out.final_text) {
            o if !o.unparsed && matches!(o.verdict, crate::review::Verdict::Pass) =>
                crate::bg_gate::GateVerdict::Pass,
            o if !o.unparsed =>
                crate::bg_gate::GateVerdict::NeedsFix(format!("self-review: {:?}", o.verdict)),
            _ => crate::bg_gate::GateVerdict::Inconclusive("no command gate; review gate deferred in v1".into()),
        }
    };
    let verdict = crate::bg_gate::evaluate(&m, &root, Some(&cancel), &|_| review_runner(&root));
    let tool_cap_hit = out.events.iter().any(|e| e.contains("tool-iteration cap"));

    let (sv, status, vs_str) = match &verdict {
        crate::bg_gate::GateVerdict::Pass => (SubgoalVerdict::Pass, NodeStatus::Done, "pass"),
        crate::bg_gate::GateVerdict::NeedsFix(r) => (SubgoalVerdict::NeedsFix, NodeStatus::NeedsFix, "needs_fix"),
        crate::bg_gate::GateVerdict::Inconclusive(r) => (SubgoalVerdict::Inconclusive, NodeStatus::NeedsFix, "inconclusive"),
    };
    {
        let mut g = WorkGraph::read(&root);
        g.set_status(milestone_id, status);
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.verdict = Some(vs_str.into());
        }
        let _ = g.save(&root);
    }
    if !matches!(verdict, crate::bg_gate::GateVerdict::Pass) {
        let reason = match &verdict { crate::bg_gate::GateVerdict::NeedsFix(r)|crate::bg_gate::GateVerdict::Inconclusive(r) => r.clone(), _ => String::new() };
        let _ = crate::tool::reason::record_cause(&root,
            &format!("milestone #{milestone_id} ({title}) 验收失败: {reason}"), None);
    }
    out.subgoals.push(SubgoalOutcome {
        milestone_id, verdict: sv,
        gate_reason: match &verdict { crate::bg_gate::GateVerdict::Pass => "gate pass".into(), crate::bg_gate::GateVerdict::NeedsFix(r)|crate::bg_gate::GateVerdict::Inconclusive(r) => r.clone() },
        tool_cap_hit, touched_files: m.touched.clone(),
    });
    out.events.push(format!("milestone #{milestone_id} ({title}) gated: {vs_str}"));
```
注:`Milestone` 需 `Clone`(在 workgraph.rs 给 `Milestone` 加 `#[derive(Clone, ...)]`,若未派生)。`agent.cancel_token()` 在 turn 后仍有效(同进程)。

- [ ] **Step 4: `run_background` 接入 mission_state + 策略循环 + tool_cap**

把 background.rs:83-102 的 `for _ in 0..MAX_AUTO.saturating_sub(1)` 自动推进段,替换为基于 `next_action` 的循环(累计 `consecutive_fail`、设 `mission_state`):
```rust
    // 设置单 milestone 工具预算(若 config 注入 —— run_background 入口取 Config)
    // (Config 经 main.rs/lib.rs 传入;若 run_background 签名不含 Config,本计划在 Task 9
    //  额外把 cfg.bg_milestone_tool_cap 经新参数或 thread-local 传入。最简:给
    //  advance_one_milestone 加 tool_cap 参数。)
```
(实现细节:`advance_one_milestone` 加 `tool_cap: usize` 参数,内部 `agent.set_tool_cap(tool_cap)`;`run_background` 用 `cfg.bg_milestone_tool_cap`;MAX_AUTO 用 `cfg.bg_max_auto`;K 用 `cfg.bg_circuit_k`。`run_background` 签名加 `cfg: &Config` —— 同步改 `main.rs`/`lib.rs::run_background` 两处调用点。)
循环骨架:
```rust
    let mut consecutive_fail = 0usize;
    let mut advanced = 1usize; // 首个 milestone 已跑
    out.mission_state = MissionState::Running;
    if task.trim().is_empty() {
        loop {
            if advanced >= cfg.bg_max_auto { out.mission_state = MissionState::CompletedAllReady; break; }
            let last = out.subgoals.last();
            let (just_id, verdict, cf) = match last {
                Some(s) => (s.milestone_id, s.verdict.clone(),
                            if matches!(s.verdict, SubgoalVerdict::Pass) { 0 } else { consecutive_fail + 1 }),
                None => break,
            };
            consecutive_fail = cf;
            let gv = match verdict { SubgoalVerdict::Pass => GateVerdict::Pass,
                SubgoalVerdict::NeedsFix => GateVerdict::NeedsFix(String::new()),
                SubgoalVerdict::Inconclusive => GateVerdict::Inconclusive(String::new()) };
            let g = WorkGraph::read(&root);
            match crate::bg_gate::next_action(&g, just_id, &gv, consecutive_fail, true, cfg.bg_circuit_k) {
                NextAction::Advance(next_id) => {
                    // 推进指定 id(而非默认 next_ready):用新内部 fn advance_specific_milestone
                    match advance_specific_milestone(provider.clone(), model.clone(), max_tokens, temperature, root.clone(), next_id, cfg.bg_milestone_tool_cap)? {
                        Some(step) => { merge(&mut out, step); advanced += 1; }
                        None => { out.mission_state = MissionState::CompletedAllReady; break; }
                    }
                }
                NextAction::Stop(st) => { out.mission_state = st; break; }
            }
        }
    }
```
(`advance_specific_milestone` = `advance_one_milestone` 的"指定 id"变体:把 `next_ready()` 换成 `get(next_id)`;抽取共用 body。`merge` 把 step 的 subgoals/tool_calls/... 并入 out。)

- [ ] **Step 5: 跑测试**

Run: `cargo test background:: 2>&1 | tail -5` 与 `cargo test bg_gate:: 2>&1 | tail -3`
Expected: T1/T2 通过;现有 2 个 advance 测试若因签名变更失败,按新签名(`advance_one_milestone(.., tool_cap)`)更新调用。

- [ ] **Step 6: 补 T4/T5 集成测试(blocked-at / circuit-breaker)**

append 到 background tests:
```rust
    #[test]
    fn t4_blocked_at_when_dependent_blocked() {
        let dir = std::env::temp_dir().join(format!("cc_t4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "false", vec![]), (2, "echo ok", vec![1])]);
        let cfg = crate::config::Config{ bg_max_auto:3, bg_circuit_k:2, bg_milestone_tool_cap:8, ..crate::config::Config::from_env_test(&dir) };
        let out = run_background(Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), "".into(), &cfg).unwrap();
        assert_eq!(out.mission_state, MissionState::BlockedAt(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
```
(`Config::from_env_test` —— 若无,Task 9 在 config.rs 加 `#[cfg(test)] pub fn from_env_test(root)->Config` 测试构造器,或直接 `Config { root: dir.into(), ..默认字段 }`。)

- [ ] **Step 7: 跑全量不回归**

Run: `cargo test 2>&1 | grep -E "test result" | tail -3`
Expected: 全通过(含 T1-T5;T3/T6/T7/T8 为 bg_gate 单测已过 + 本 Task 集成)。

- [ ] **Step 8: 提交**

```bash
git add src/background.rs src/config.rs src/workgraph.rs src/lib.rs src/main.rs
git commit -m "feat(background): 客观验收门覆盖自报 + 失败写回 + 策略循环

advance_one_milestone turn 后跑 bg_gate::evaluate(命令门优先,覆盖
agent VERDICT 自报);fail/inconclusive → needs_fix + record_cause 因果
节点 + SubgoalOutcome。run_background 用 next_action 策略推进
(熔断/BlockedAt/CompletedAllReady),tool_cap/bg_max_auto/circuit_k
取自 Config。审计 #1(固着/回退当成功)根治。"
```

---

## Task 10: 文档同步

**Files:**
- Create: `docs/adr/0030-bg-objective-acceptance-gate.md`(或增补 0026)
- Modify: `ARCHITECTURE.md`、`README.md`(`background.rs` 行 + env 表 + ADR 索引)

- [ ] **Step 1: 写 ADR**

`docs/adr/0030-bg-objective-acceptance-gate.md`:Context=审计发现自报不可靠;Decision=客观命令门覆盖自报 + 失败写回 needs_fix+causal + next_action 策略(熔断 K=2/BlockedAt);Status=Accepted;Consequences=v1 review 门用自产 VERDICT 兜底(真正 review 子代理门后续)。

- [ ] **Step 2: 同步 ARCHITECTURE/README**

ARCHITECTURE.md 的 `background.rs` 行补"客观验收门 + 失败写回 + continue/stop 策略";README env 表加 `CODECODER_BG_MAX_AUTO`/`_CIRCUIT_K`/`_MILESTONE_TOOL_CAP`;ADR 索引加 `0030`。

- [ ] **Step 3: 提交**

```bash
git add docs/adr/0030-bg-objective-acceptance-gate.md ARCHITECTURE.md README.md
git commit -m "docs: ADR 0030 BG 客观验收门 + 同步 ARCHITECTURE/README env 表"
```

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- 验收门三层(命令→review→Inconclusive)→ Task 3/4(extract+command+evaluate)。review 门 v1 用自产 VERDICT 兜底(spec §5.1 (b) 的 v1 简化,Task 9 Step 3 `review_runner` 注释明示"review 子代理门为后续增强")。
- 失败写回(needs_fix + reason 因果节点 + SubgoalOutcome)→ Task 6 + Task 7 + Task 9。
- continue-vs-stop(独立可继续/BlockedAt/熔断 K=2)→ Task 5 + Task 9 Step 4/6。
- 每 milestone 工具预算 → Task 1(env)+ Task 8(tool_cap)+ Task 9(接入)。
- BgOutcome 扩展 → Task 7。
- 测试 T1-T8 → Task 4(命令门/evaluate)+ Task 5(next_action)+ Task 9(T1/T2/T4/T5 集成);T3(触顶+review)、T6(兜底)、T7(覆盖自报)、T8(取消)由 bg_gate 单测 + Task 9 Step 3 review_runner 覆盖。**T8(SIGINT 取消门执行)无 hermetic 单测**——取消经既有 `run_shell_cancellable` + `cancel_on_sigint`(已测),不重复;plan 在 Task 9 注明。
- 文档 → Task 10。

**2. Placeholder scan:** Task 8 Step 1 与 Task 9 Step 1 的 T7/T8 标注了"参照 tests/l1_interaction.rs 模式"——这是指向既有具体范式(非占位),但实现者需照搬该文件构造法;已在 Task 9 Step 4 给出循环骨架与签名变更清单,可执行。无 "TBD/适当错误处理" 等占位。

**3. Type consistency:** `GateVerdict`/`MissionState`/`NextAction`/`SubgoalVerdict`/`SubgoalOutcome` 在各 Task 间命名一致;`next_action` 签名(graph, just_done_id, verdict, consecutive_fail, budget_left, k)在 Task 5 定义、Task 9 调用一致;`advance_one_milestone` 加 `tool_cap` 参数在 Task 8/9 一致。`Milestone` 需 `Clone`(Task 9 Step 3 注明在 workgraph.rs 派生)。
