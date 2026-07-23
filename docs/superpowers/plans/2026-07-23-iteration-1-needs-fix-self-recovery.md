# 迭代 1：needs_fix 自恢复循环 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** headless workgraph runner 在里程碑验收失败(needs_fix)后，自动把失败原因喂回 agent 并在预算内重试，而非停摆等人手动 reset pending。

**Architecture:** 在 `Milestone` 上持久化 `fix_attempts` / `last_failure` 两个字段；新增 `WorkGraph::next_retryable` 选取「未耗尽重试预算的 needs_fix 里程碑」；把 `advance_one_milestone` 的「跑 turn + 客观门 + 写回」核心抽成 `run_milestone_and_gate`，新增 `retry_one_milestone` 复用它并注入失败原因构造修复 prompt；改造 `run_background_cfg` 主循环，使「无 pending-ready 时先尝试自恢复一个 needs_fix」，只有重试预算耗尽仍 needs_fix 才落 `StuckNeedsFix`(exit 2)。

**Tech Stack:** Rust（无新依赖）；serde（字段默认值向后兼容，无需 bump workgraph schema_version）；`StubClient` + 一个有状态测试 Provider 做 hermetic L1 测试。

## Global Constraints

- 不新增任何 crate 依赖（纯 std + 现有 serde/anyhow/fs2）。
- 遵守 TDD：每个行为先写失败测试，再写最小实现。
- 所有单测必须 hermetic（`tempdir` + `StubClient`/测试 Provider，不触真实 LLM、不依赖网络）。
- 术语精确（CONTEXT.md）：milestone / needs_fix / mission_state / verdict / gate。
- 契约：`retry_one_milestone` 在里程碑运行**之前**递增 `fix_attempts`，使预算即便在 turn 崩溃时也被尊重（呼应 ADR 0034 持久化精神）。
- 重试**不**计入 `max_auto`（`advanced` 只统计非重试的 advance）；重试次数由每里程碑 `fix_attempts < max_fix_attempts` 独立约束。
- `max_fix_attempts == 0` 表示禁用自恢复，行为回退到迭代 1 之前（保留 `BlockedAt`/`CircuitBreaker`/`StuckNeedsFix` 既有语义）。

---

## 关键现状锚点（实现者须知）

- `src/workgraph.rs:59` `Milestone` 结构体；`add()`(179)、`set_status()`(208)、`next_ready()`(233)、`deps_done()`(240)、`with_lock()`(142)。新字段用 `#[serde(default)]` 即向后兼容，**无需**改 `WG_SCHEMA_VERSION` 或 `migrate`。
- `src/background.rs:102` `run_background_cfg`（workgraph 主循环在 138–212）；`advance_one_milestone`(240–342)。
- `src/bg_gate.rs`：`evaluate`(84)、`GateVerdict`(16)、`MissionState`(103，已含 `StuckNeedsFix(u64)`)、`next_action`(135)。
- 客观门：`extract_gate_command` 只认纯 ASCII 命令行（cargo/pytest/make/rustc…）；prose（含 CJK）→ 交 `review_runner`，后者用 `review::parse_review` 解析 agent 文本里的 `VERDICT: <pass|needs_fix|rebuild>`（`src/review.rs:141`）。**测试利用点**：给里程碑写 prose acceptance（如 `"渲染输出正确"`），再用测试 Provider 让 final_text 含 `VERDICT: needs_fix`/`VERDICT: pass`，即可确定性地驱动门的通过/失败。
- `Completion: From<Message>`；`Message { id, role: Role::Assistant, items: vec![MessageItem::Text { text }] }.into()`（见 `src/provider/stub.rs`）。

---

## Task 1: WorkGraph 重试状态字段 + next_retryable

**Files:**
- Modify: `src/workgraph.rs`（`Milestone` 结构体 ~59-73、`add()` ~189-197、新增 `next_retryable`）
- Modify: `src/bg_gate.rs`（测试辅助 `ms()` 的 `Milestone` 字面量 ~173-183）
- Test: `src/workgraph.rs`（`#[cfg(test)]`）

**Interfaces:**
- Produces: `Milestone.fix_attempts: usize`、`Milestone.last_failure: Option<String>`、`WorkGraph::next_retryable(&self, max_attempts: usize) -> Option<&Milestone>`

- [ ] **Step 1: 写失败测试**（加到 `src/workgraph.rs` 的 `mod tests`）

```rust
#[test]
fn next_retryable_picks_lowest_needs_fix_within_budget() {
    let mut g = WorkGraph::default();
    let a = g.add("a", "acc", vec![]).unwrap();
    let b = g.add("b", "acc", vec![]).unwrap();
    g.set_status(a, NodeStatus::NeedsFix);
    g.set_status(b, NodeStatus::NeedsFix);
    // 两个都在预算内 → 取最低 id。
    assert_eq!(g.next_retryable(3).map(|n| n.id), Some(a));
    // a 耗尽预算 → 取 b。
    g.nodes.iter_mut().find(|n| n.id == a).unwrap().fix_attempts = 3;
    assert_eq!(g.next_retryable(3).map(|n| n.id), Some(b));
    // 都耗尽 → None。
    g.nodes.iter_mut().find(|n| n.id == b).unwrap().fix_attempts = 3;
    assert_eq!(g.next_retryable(3), None);
}

#[test]
fn next_retryable_skips_pending_and_blocked_and_respects_deps() {
    let mut g = WorkGraph::default();
    let a = g.add("a", "acc", vec![]).unwrap();
    let b = g.add("b", "acc", vec![a]).unwrap(); // 依赖 a
    g.set_status(b, NodeStatus::NeedsFix);        // b needs_fix 但 dep a 未 Done
    // a 仍 Pending（非 needs_fix）→ 不可重试；b 的 dep 未 Done → 不可重试。
    assert_eq!(g.next_retryable(3), None);
    // 关闭预算 → 即便 needs_fix 也不选。
    g.set_status(a, NodeStatus::Done);            // 现在 b 的 dep 满足
    assert_eq!(g.next_retryable(3).map(|n| n.id), Some(b));
    assert_eq!(g.next_retryable(0), None);        // max_attempts=0 → 禁用
}

#[test]
fn new_milestone_defaults_retry_state() {
    let mut g = WorkGraph::default();
    let a = g.add("a", "acc", vec![]).unwrap();
    let n = g.get(a).unwrap();
    assert_eq!(n.fix_attempts, 0);
    assert_eq!(n.last_failure, None);
}
```

- [ ] **Step 2: 跑测试确认失败（编译错误亦可）**

Run: `cargo test --lib workgraph::tests::next_retryable`
Expected: FAIL — `no method named next_retryable` / `no field fix_attempts`。

- [ ] **Step 3: 加字段**（`src/workgraph.rs`，`Milestone` 结构体尾部 `touched` 之后）

```rust
    /// 自恢复循环:该 milestone 已消耗的 needs_fix 重试次数(ADR 0026 迭代 1)。
    #[serde(default)]
    pub fix_attempts: usize,
    /// 最近一次验收失败原因,供重试 prompt 注入;跨进程持久化。Pass 时清空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
```

- [ ] **Step 4: 初始化字段**（`src/workgraph.rs` `add()` 里的 `Milestone { … }` 字面量，`touched: Vec::new(),` 之后）

```rust
            fix_attempts: 0,
            last_failure: None,
```

- [ ] **Step 5: 新增 `next_retryable`**（`src/workgraph.rs`，紧接 `next_ready` / `deps_done` 之后）

```rust
    /// 下一个可重试的 needs_fix milestone:状态 NeedsFix、deps 全 Done、且重试预算
    /// 未耗尽(`fix_attempts < max_attempts`),取最低 id。`max_attempts == 0` 恒返回
    /// None(禁用自恢复)。`None` 表示无可重试项。
    pub fn next_retryable(&self, max_attempts: usize) -> Option<&Milestone> {
        self.nodes
            .iter()
            .filter(|n| {
                n.status == NodeStatus::NeedsFix
                    && self.deps_done(n)
                    && n.fix_attempts < max_attempts
            })
            .min_by_key(|n| n.id)
    }
```

- [ ] **Step 6: 修复 `src/bg_gate.rs` 测试字面量**（`ms()`，`touched: vec![],` 之后加两行）

```rust
            touched: vec![],
            fix_attempts: 0,
            last_failure: None,
```

- [ ] **Step 7: 编译全仓，修补其余 `Milestone { … }` 字面量（若有）**

Run: `cargo build --tests`
Expected: 若报 `missing fields fix_attempts, last_failure in initializer of Milestone`，在报错处补 `fix_attempts: 0, last_failure: None,`。修到通过为止。

- [ ] **Step 8: 跑测试确认通过**

Run: `cargo test --lib workgraph::tests::next_retryable && cargo test --lib workgraph::tests::new_milestone_defaults`
Expected: PASS（3 个新测试全绿）。

- [ ] **Step 9: 提交**

```bash
git add src/workgraph.rs src/bg_gate.rs
git commit -m "feat(workgraph): add fix_attempts/last_failure + next_retryable for self-recovery"
```

---

## Task 2: 配置项 bg_max_fix_attempts

**Files:**
- Modify: `src/config.rs`（`Config` 结构体 + `from_env` + 测试）
- Test: `src/config.rs`（`#[cfg(test)]`）

**Interfaces:**
- Produces: `Config.bg_max_fix_attempts: usize`（env `CODECODER_BG_MAX_FIX_ATTEMPTS`，默认 3）

- [ ] **Step 1: 写失败测试**（加到 `src/config.rs` `mod tests`）

```rust
#[test]
fn bg_max_fix_attempts_default_and_override() {
    unsafe { std::env::remove_var("CODECODER_BG_MAX_FIX_ATTEMPTS"); }
    assert_eq!(Config::from_env().bg_max_fix_attempts, 3);
    unsafe { std::env::set_var("CODECODER_BG_MAX_FIX_ATTEMPTS", "5"); }
    assert_eq!(Config::from_env().bg_max_fix_attempts, 5);
    unsafe { std::env::remove_var("CODECODER_BG_MAX_FIX_ATTEMPTS"); }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib config::tests::bg_max_fix_attempts`
Expected: FAIL — `no field bg_max_fix_attempts`。

- [ ] **Step 3: 加结构体字段**（`src/config.rs`，`bg_milestone_tool_cap` 之后）

```rust
    /// BG 自恢复:单 milestone needs_fix 后最多自动重试次数(ADR 0026 迭代 1)。
    /// 0 = 禁用自恢复(回退到旧的一失败即停语义)。
    pub bg_max_fix_attempts: usize,
```

- [ ] **Step 4: 加 env 解析**（`src/config.rs` `from_env`，`bg_milestone_tool_cap: …` 那块之后）

```rust
            bg_max_fix_attempts: env("CODECODER_BG_MAX_FIX_ATTEMPTS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test --lib config::tests::bg_max_fix_attempts`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/config.rs
git commit -m "feat(config): add CODECODER_BG_MAX_FIX_ATTEMPTS (default 3)"
```

---

## Task 3: build_repair_prompt 纯函数

**Files:**
- Modify: `src/background.rs`（新增 `pub(crate) fn build_repair_prompt` + 测试）
- Test: `src/background.rs`（`#[cfg(test)]`）

**Interfaces:**
- Consumes: `crate::workgraph::Milestone`
- Produces: `pub(crate) fn build_repair_prompt(m: &Milestone, last_failure: &str) -> String`

- [ ] **Step 1: 写失败测试**（加到 `src/background.rs` `mod tests`）

```rust
#[test]
fn build_repair_prompt_injects_failure_and_title() {
    use crate::workgraph::{Milestone, NodeStatus};
    let m = Milestone {
        id: 7,
        title: "CRDT 核心".into(),
        acceptance: "cargo test".into(),
        deps: vec![],
        status: NodeStatus::NeedsFix,
        verdict: None,
        touched: vec![],
        fix_attempts: 1,
        last_failure: Some("gate `cargo test` failed: 2 failed".into()),
    };
    let p = build_repair_prompt(&m, "gate `cargo test` failed: 2 failed");
    assert!(p.contains("CRDT 核心"), "含标题: {p}");
    assert!(p.contains("gate `cargo test` failed: 2 failed"), "含失败原因: {p}");
    assert!(p.contains("cargo test"), "含 acceptance: {p}");
    assert!(p.trim_end().ends_with("VERDICT: <pass|needs_fix|rebuild>"), "以 VERDICT 行结尾: {p}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib background::tests::build_repair_prompt`
Expected: FAIL — `cannot find function build_repair_prompt`。

- [ ] **Step 3: 实现纯函数**（`src/background.rs`，放在 `advance_one_milestone` 之前）

```rust
/// 构造 needs_fix 重试的修复 prompt:注入上一轮失败原因 + acceptance,要求先针对
/// 失败做实际改动,再自评,并以内核可解析的 VERDICT 行结尾。纯函数,便于单测。
pub(crate) fn build_repair_prompt(m: &crate::workgraph::Milestone, last_failure: &str) -> String {
    format!(
        "workgraph milestone #{} ({}) 上一轮验收未通过,需要修复后重试。\n\
         上一轮失败原因:\n{}\n\n\
         acceptance: {}\n\n\
         请针对上述失败原因做出实际代码改动来修复它(不要只解释),然后自评。\
         你必须以下面这行精确格式结尾(其后不要有任何内容),以便内核解析并自动更新\
         里程碑状态:\n\
         VERDICT: <pass|needs_fix|rebuild>",
        m.id,
        m.title,
        last_failure.trim(),
        if m.acceptance.is_empty() { "(none)" } else { &m.acceptance },
    )
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib background::tests::build_repair_prompt`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/background.rs
git commit -m "feat(bg): build_repair_prompt injects prior failure reason for retries"
```

---

## Task 4: 抽出 run_milestone_and_gate 核心（重构，无行为变更）

**Files:**
- Modify: `src/background.rs`（`advance_one_milestone` 240-342 拆分）

**Interfaces:**
- Produces: `fn run_milestone_and_gate(provider: Arc<dyn Provider>, model: String, max_tokens: u32, temperature: f32, root: PathBuf, milestone_id: u64, task_text: String, title: String) -> anyhow::Result<BgOutcome>`
- `advance_one_milestone` 签名不变，改为委托：选 `next_ready` → 构造常规 prompt → 调核心。

- [ ] **Step 1: 新增核心函数**（`src/background.rs`，替换 `advance_one_milestone` 中「`let mut agent = AgentLoop::new_background…`」到 `Ok(Some(out))` 的整段为对核心的调用；把那段逻辑搬进新函数）

```rust
/// 跑一个已选定 milestone 的 turn + 客观验收门 + 写回状态。被 `advance_one_milestone`
/// (pending 常规推进)与 `retry_one_milestone`(needs_fix 自恢复)共用。
fn run_milestone_and_gate(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    milestone_id: u64,
    task_text: String,
    title: String,
) -> anyhow::Result<BgOutcome> {
    use crate::workgraph::{NodeStatus, WorkGraph};
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    if let Err(e) = agent.cancel_token().cancel_on_sigint() {
        eprintln!("ccd: SIGINT cancel not wired: {e}");
    }
    let cfg = crate::config::Config::from_env();
    agent.set_tool_cap(cfg.bg_milestone_tool_cap);
    let cancel = agent.cancel_token();
    let mut out = BgOutcome::default();
    out.events.push(format!("task: workgraph milestone #{} ({})", milestone_id, title));
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(task_text, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);
    if let Some(e) = agent.last_error() {
        return Err(anyhow::anyhow!(e.to_string()));
    }

    // ── 客观验收门(覆盖 agent 自报 VERDICT)──
    let m = {
        let g = WorkGraph::read(&root);
        g.get(milestone_id).expect("just read").clone()
    };
    let tool_cap_hit = out.events.iter().any(|e| e.contains("tool-iteration cap"));
    let review_runner = || -> crate::bg_gate::GateVerdict {
        let o = crate::review::parse_review(&out.final_text);
        if !o.unparsed && matches!(o.verdict, crate::review::Verdict::Pass) {
            crate::bg_gate::GateVerdict::Pass
        } else if !o.unparsed {
            crate::bg_gate::GateVerdict::NeedsFix(format!("self-review: {:?}", o.verdict))
        } else {
            crate::bg_gate::GateVerdict::Inconclusive("no command gate; review gate deferred in v1".into())
        }
    };
    let verdict = crate::bg_gate::evaluate(&m, &root, Some(&cancel), &review_runner);

    let (sv, status, vs_str) = match &verdict {
        crate::bg_gate::GateVerdict::Pass => (SubgoalVerdict::Pass, NodeStatus::Done, "pass"),
        crate::bg_gate::GateVerdict::NeedsFix(_) => (SubgoalVerdict::NeedsFix, NodeStatus::NeedsFix, "needs_fix"),
        crate::bg_gate::GateVerdict::Inconclusive(_) => (SubgoalVerdict::Inconclusive, NodeStatus::NeedsFix, "inconclusive"),
    };
    let gate_reason = match &verdict {
        crate::bg_gate::GateVerdict::Pass => "gate pass".to_string(),
        crate::bg_gate::GateVerdict::NeedsFix(r) | crate::bg_gate::GateVerdict::Inconclusive(r) => r.clone(),
    };
    {
        let reason_for_persist = gate_reason.clone();
        let _ = WorkGraph::with_lock(&root, |g| {
            g.set_status(milestone_id, status);
            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                n.verdict = Some(vs_str.into());
                // Task 5 在此持久化/清空 last_failure(先占位,Task 5 填充)。
                let _ = &reason_for_persist;
            }
            Ok(())
        });
    }
    if !matches!(verdict, crate::bg_gate::GateVerdict::Pass) {
        let _ = crate::tool::reason::record_cause(
            &root,
            &format!("milestone #{milestone_id} ({title}) 验收失败: {gate_reason}"),
            None,
        );
    }
    out.subgoals.push(SubgoalOutcome {
        milestone_id,
        verdict: sv,
        gate_reason,
        tool_cap_hit,
        touched_files: m.touched.clone(),
    });
    out.events.push(format!("milestone #{} ({}) gated: {vs_str}", milestone_id, title));
    Ok(out)
}
```

- [ ] **Step 2: 改写 `advance_one_milestone` 为委托**（`src/background.rs`，替换整个函数体）

```rust
/// 推进 workgraph 的下一个就绪(pending)里程碑：跑一个 turn、客观门、写回状态。
/// 无就绪里程碑时返回 `Ok(None)`。daemon 与 background runner 共用此函数。
pub fn advance_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::WorkGraph;
    let (milestone_id, task_text, title) = {
        let g = WorkGraph::read(&root);
        let Some(n) = g.next_ready() else { return Ok(None); };
        let t = format!(
            "workgraph milestone #{}: {}\nacceptance: {}\n\n\
             Complete this milestone, then self-review. You MUST end your reply \
             with a final line in EXACTLY this format (nothing after it) so the \
             kernel can parse and auto-update the milestone status:\n\
             VERDICT: <pass|needs_fix|rebuild>",
            n.id, n.title,
            if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
        );
        (n.id, t, n.title.clone())
    };
    run_milestone_and_gate(provider, model, max_tokens, temperature, root, milestone_id, task_text, title)
        .map(Some)
}
```

- [ ] **Step 3: 跑既有测试确认无行为变更**

Run: `cargo test --lib background::tests`
Expected: PASS — `advance_one_milestone_returns_none_when_empty`、`advance_one_milestone_runs_a_turn`、`t1`、`t2`、`t4`、`t5`、`stuck_needs_fix_*` 等全部维持绿（纯重构）。

- [ ] **Step 4: 提交**

```bash
git add src/background.rs
git commit -m "refactor(bg): extract run_milestone_and_gate core from advance_one_milestone"
```

---

## Task 5: 验收门写回时持久化/清空 last_failure

**Files:**
- Modify: `src/background.rs`（`run_milestone_and_gate` 的 `with_lock` 写回闭包）
- Test: `src/background.rs`

**Interfaces:**
- Consumes: `run_milestone_and_gate`（Task 4）、`Milestone.last_failure`（Task 1）

- [ ] **Step 1: 写失败测试**（加到 `src/background.rs` `mod tests`）

```rust
#[test]
fn gate_failure_persists_last_failure_reason() {
    let dir = std::env::temp_dir().join(format!("cc_lastfail_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // prose acceptance(无命令模式)+ StubClient 无 VERDICT → 评审门 Inconclusive → NeedsFix。
    ws(&dir, &[(1, "渲染输出正确", vec![])]);
    let _ = advance_one_milestone(
        Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
    ).unwrap().unwrap();
    let n = WorkGraph::read(&dir).get(1).unwrap().clone();
    assert_eq!(n.status, NodeStatus::NeedsFix);
    assert!(n.last_failure.is_some(), "needs_fix 应记录 last_failure");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib background::tests::gate_failure_persists_last_failure_reason`
Expected: FAIL — `last_failure` 为 `None`（Task 4 只占位未写）。

- [ ] **Step 3: 填充写回闭包**（`src/background.rs` `run_milestone_and_gate`，把 Task 4 里 `let _ = &reason_for_persist;` 那行替换）

```rust
                if matches!(status, NodeStatus::NeedsFix) {
                    n.last_failure = Some(reason_for_persist.clone());
                } else {
                    n.last_failure = None; // Pass 时清空,避免陈旧原因污染未来重试
                }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib background::tests::gate_failure_persists_last_failure_reason`
Expected: PASS。

- [ ] **Step 5: 跑既有测试确认无回归**

Run: `cargo test --lib background::tests`
Expected: PASS（全绿）。

- [ ] **Step 6: 提交**

```bash
git add src/background.rs
git commit -m "feat(bg): persist last_failure on needs_fix, clear on pass"
```

---

## Task 6: retry_one_milestone

**Files:**
- Modify: `src/background.rs`（新增 `retry_one_milestone` + 有状态测试 Provider + 测试）
- Test: `src/background.rs`

**Interfaces:**
- Consumes: `WorkGraph::next_retryable`（Task 1）、`build_repair_prompt`（Task 3）、`run_milestone_and_gate`（Task 4）
- Produces: `pub fn retry_one_milestone(provider: Arc<dyn Provider>, model: String, max_tokens: u32, temperature: f32, root: PathBuf, max_fix_attempts: usize) -> anyhow::Result<Option<BgOutcome>>`
- Produces（测试专用）: `struct FlakyProvider { fail_until: usize, calls: std::sync::Mutex<usize> }`——前 `fail_until` 次 `complete` 返回 `VERDICT: needs_fix`，其后返回 `VERDICT: pass`。

- [ ] **Step 1: 写失败测试**（加到 `src/background.rs` `mod tests`；FlakyProvider 供本 Task 与 Task 7 共用）

```rust
struct FlakyProvider {
    fail_until: usize,
    calls: std::sync::Mutex<usize>,
}
impl crate::provider::Provider for FlakyProvider {
    fn name(&self) -> &str { "flaky" }
    fn complete(
        &self,
        _req: &crate::provider::CompletionRequest,
    ) -> anyhow::Result<crate::provider::Completion> {
        use crate::message::{Message, MessageItem, Role};
        let mut c = self.calls.lock().unwrap();
        let i = *c;
        *c += 1;
        let text = if i < self.fail_until { "VERDICT: needs_fix" } else { "VERDICT: pass" };
        Ok(Message {
            id: 0,
            role: Role::Assistant,
            items: vec![MessageItem::Text { text: text.into() }],
        }
        .into())
    }
}

#[test]
fn retry_one_milestone_bumps_attempt_and_can_pass() {
    let dir = std::env::temp_dir().join(format!("cc_retry1_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 种一个 needs_fix 里程碑(prose acceptance → 走评审门,读 Provider 的 VERDICT)。
    let mut g = WorkGraph::default();
    let id = g.add("core", "渲染输出正确", vec![]).unwrap();
    g.set_status(id, NodeStatus::NeedsFix);
    g.nodes.iter_mut().find(|n| n.id == id).unwrap().last_failure = Some("上轮失败".into());
    g.save(&dir).unwrap();

    // Provider 立即 pass(fail_until=0)。
    let out = retry_one_milestone(
        Arc::new(FlakyProvider { fail_until: 0, calls: std::sync::Mutex::new(0) }),
        "m".into(), 4096, 0.0, dir.clone(), 3,
    ).unwrap();
    assert!(out.is_some(), "有可重试项应返回 Some");
    let n = WorkGraph::read(&dir).get(id).unwrap().clone();
    assert_eq!(n.status, NodeStatus::Done, "pass 后应 Done");
    assert_eq!(n.fix_attempts, 1, "重试应递增 fix_attempts");
    assert_eq!(n.last_failure, None, "pass 后应清空 last_failure");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retry_one_milestone_none_when_nothing_retryable() {
    let dir = std::env::temp_dir().join(format!("cc_retry0_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ws(&dir, &[(1, "渲染输出正确", vec![])]); // 默认 Pending,非 needs_fix
    let out = retry_one_milestone(
        Arc::new(StubClient), "m".into(), 4096, 0.0, dir.clone(), 3,
    ).unwrap();
    assert!(out.is_none(), "无 needs_fix 时应 None");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib background::tests::retry_one_milestone`
Expected: FAIL — `cannot find function retry_one_milestone`。

- [ ] **Step 3: 实现 retry_one_milestone**（`src/background.rs`，放在 `advance_one_milestone` 之后）

```rust
/// 自恢复一个 needs_fix 里程碑(ADR 0026 迭代 1)。选 `next_retryable`,**先**递增其
/// `fix_attempts`(即便 turn 崩溃预算也被尊重),再注入上一轮失败原因构造修复 prompt,
/// 跑一 turn + 客观门。无可重试项(无 needs_fix 或全部耗尽预算)时返回 `Ok(None)`。
pub fn retry_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    max_fix_attempts: usize,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::WorkGraph;
    let (milestone_id, prompt, title) = {
        let g = WorkGraph::read(&root);
        let Some(n) = g.next_retryable(max_fix_attempts) else { return Ok(None); };
        let last = n
            .last_failure
            .clone()
            .unwrap_or_else(|| "(无记录的失败原因)".to_string());
        (n.id, build_repair_prompt(n, &last), n.title.clone())
    };
    // 先记账再跑:即便本次 turn 崩溃,预算也已消耗,避免无限重试。
    let _ = WorkGraph::with_lock(&root, |g| {
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.fix_attempts += 1;
        }
        Ok(())
    });
    run_milestone_and_gate(provider, model, max_tokens, temperature, root, milestone_id, prompt, title)
        .map(Some)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib background::tests::retry_one_milestone`
Expected: PASS（2 个测试绿）。

- [ ] **Step 5: 提交**

```bash
git add src/background.rs
git commit -m "feat(bg): retry_one_milestone drives budgeted needs_fix self-recovery"
```

---

## Task 7: 把自恢复接入 run_background_cfg 主循环

**Files:**
- Modify: `src/background.rs`（`run_background`/`run_background_cfg` 签名 + workgraph 主循环 138-212；更新既有测试 callsite）
- Test: `src/background.rs`

**Interfaces:**
- Consumes: `advance_one_milestone`、`retry_one_milestone`（Task 6）、`Config.bg_max_fix_attempts`（Task 2）
- Produces: `run_background_cfg(…, max_auto, circuit_k, tool_cap, max_fix_attempts)`（末尾新增第 10 个参数 `max_fix_attempts: usize`）

- [ ] **Step 1: 写失败测试**（加到 `src/background.rs` `mod tests`；复用 Task 6 的 `FlakyProvider`）

```rust
#[test]
fn workgraph_auto_retries_needs_fix_until_pass() {
    let dir = std::env::temp_dir().join(format!("cc_selfrec_pass_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ws(&dir, &[(1, "渲染输出正确", vec![])]); // prose → 评审门读 Provider VERDICT
    // 首次 advance + retry#1 都 needs_fix,retry#2 pass(fail_until=2)。
    let out = run_background_cfg(
        Arc::new(FlakyProvider { fail_until: 2, calls: std::sync::Mutex::new(0) }),
        "m".into(), 0.0f32 as _ , dir.clone(), "".into(),
        5,   // max_auto
        10,  // circuit_k(高,验证自恢复而非熔断主导)
        8,   // tool_cap
        3,   // max_fix_attempts
    ).unwrap();
    // 占位:签名见下方 Step 3;此调用按最终签名书写。
    assert_eq!(out.mission_state, MissionState::CompletedAllReady, "{:?}", out.mission_state);
    let n = WorkGraph::read(&dir).get(1).unwrap().clone();
    assert_eq!(n.status, NodeStatus::Done);
    assert_eq!(n.fix_attempts, 2, "两次重试后通过");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workgraph_gives_up_after_max_fix_attempts() {
    let dir = std::env::temp_dir().join(format!("cc_selfrec_giveup_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ws(&dir, &[(1, "渲染输出正确", vec![])]);
    // 恒 needs_fix(fail_until 极大)。
    let out = run_background_cfg(
        Arc::new(FlakyProvider { fail_until: 9999, calls: std::sync::Mutex::new(0) }),
        "m".into(), 0.0f32 as _, dir.clone(), "".into(),
        10, // max_auto
        10, // circuit_k
        8,  // tool_cap
        2,  // max_fix_attempts
    ).unwrap();
    assert_eq!(out.mission_state, MissionState::StuckNeedsFix(1), "{:?}", out.mission_state);
    let n = WorkGraph::read(&dir).get(1).unwrap().clone();
    assert_eq!(n.fix_attempts, 2, "预算耗尽应等于 max_fix_attempts");
    let _ = std::fs::remove_dir_all(&dir);
}
```

> 注：`max_tokens` 参数类型是 `u32`；上面 `0.0f32 as _` 是 `temperature` 占位写法易误读，Step 3 落定后请按真实签名 `(provider, model, max_tokens=256, temperature=0.0, root, task, max_auto, circuit_k, tool_cap, max_fix_attempts)` 修正这两个调用的实参顺序：`256, 0.0,`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib background::tests::workgraph_auto_retries`
Expected: FAIL — 参数个数不匹配 / 行为不符。

- [ ] **Step 3: 改签名 + 主循环**（`src/background.rs`）

先把 `run_background`(86) 的转发补上新参数：

```rust
    run_background_cfg(
        provider, model, max_tokens, temperature, root, task,
        cfg.bg_max_auto, cfg.bg_circuit_k, cfg.bg_milestone_tool_cap, cfg.bg_max_fix_attempts,
    )
```

改 `run_background_cfg` 签名（102-112），末尾加参数：

```rust
pub(crate) fn run_background_cfg(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    task: String,
    max_auto: usize,
    circuit_k: usize,
    tool_cap: usize,
    max_fix_attempts: usize,
) -> anyhow::Result<BgOutcome> {
```

把 workgraph 主循环（现 138-212）整体替换为：

```rust
    loop {
        if advanced >= max_auto {
            out.mission_state = crate::bg_gate::MissionState::CompletedAllReady;
            break;
        }
        // 选取:优先就绪(pending)里程碑;无就绪则尝试自恢复一个 needs_fix。
        let ready_id = { crate::workgraph::WorkGraph::read(&root).next_ready().map(|n| n.id) };
        let (step, from_retry) = if ready_id.is_some() {
            match advance_one_milestone(
                provider.clone(), model.clone(), max_tokens, temperature, root.clone(),
            ) {
                Ok(Some(s)) => (s, false),
                Ok(None) => break, // race-safe:重读后已无就绪
                Err(e) => {
                    out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
                    break;
                }
            }
        } else {
            match retry_one_milestone(
                provider.clone(), model.clone(), max_tokens, temperature, root.clone(), max_fix_attempts,
            ) {
                Ok(Some(s)) => (s, true),
                Ok(None) => {
                    // 既无就绪、也无可重试 needs_fix → 终态。仅在仍 Running 时置态,
                    // 区分"真完成"与"卡在 needs_fix(预算耗尽)"。
                    if out.mission_state == crate::bg_gate::MissionState::Running {
                        let g = crate::workgraph::WorkGraph::read(&root);
                        let needs_fix = g
                            .nodes
                            .iter()
                            .find(|n| n.status == crate::workgraph::NodeStatus::NeedsFix);
                        out.mission_state = match needs_fix {
                            Some(n) => crate::bg_gate::MissionState::StuckNeedsFix(n.id),
                            None => crate::bg_gate::MissionState::CompletedAllReady,
                        };
                    }
                    break;
                }
                Err(e) => {
                    out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
                    break;
                }
            }
        };

        // 累积输出(先取 last 再 extend,避免所有权移动)。
        let last = step.subgoals.last().cloned();
        out.final_text.push_str(&step.final_text);
        out.tool_calls.extend(step.tool_calls);
        out.denied.extend(step.denied);
        out.events.extend(step.events);
        out.subgoals.extend(step.subgoals);
        let Some(sg) = last else { break; };

        if from_retry {
            // 重试的成败由 fix_attempts 预算约束,不计入 max_auto / consecutive_fail /
            // next_action;下一轮 selection 会再重试(若仍有预算)或落 StuckNeedsFix。
            continue;
        }
        advanced += 1;
        let passed = matches!(sg.verdict, SubgoalVerdict::Pass);
        if !passed {
            // 该 milestone 仍有重试预算 → 交给下一轮 selection 自恢复,不计 cf、不走 next_action。
            let has_budget = {
                let g = crate::workgraph::WorkGraph::read(&root);
                g.get(sg.milestone_id).map(|n| n.fix_attempts < max_fix_attempts).unwrap_or(false)
            };
            if has_budget {
                continue;
            }
        }
        // pass,或失败且预算耗尽(硬失败)→ 沿用既有 next_action 语义(BlockedAt/CircuitBreaker/…)。
        if passed {
            consecutive_fail = 0;
        } else {
            consecutive_fail += 1;
        }
        let gv = if passed {
            crate::bg_gate::GateVerdict::Pass
        } else if matches!(sg.verdict, SubgoalVerdict::Inconclusive) {
            crate::bg_gate::GateVerdict::Inconclusive(sg.gate_reason.clone())
        } else {
            crate::bg_gate::GateVerdict::NeedsFix(sg.gate_reason.clone())
        };
        let g = crate::workgraph::WorkGraph::read(&root);
        let budget_left = advanced < max_auto;
        match crate::bg_gate::next_action(
            &g, sg.milestone_id, &gv, consecutive_fail, budget_left, circuit_k,
        ) {
            crate::bg_gate::NextAction::Advance(_) => continue,
            crate::bg_gate::NextAction::Stop(st) => {
                out.mission_state = st;
                break;
            }
        }
    }
    Ok(out)
```

- [ ] **Step 4: 更新既有 `run_background_cfg` 调用点（补第 10 参数）**

`src/background.rs` 测试里 4 处调用，按各自意图补 `max_fix_attempts`：

- `explicit_task_provider_error_yields_error_state`（~406）：显式 task 分支不进循环 → 末尾加 `, 0`。
- `workgraph_provider_error_yields_error_state`（~432）：首个 advance 即 provider 错 → 末尾加 `, 0`。
- `t4_blocked_at_when_dependent_blocked`（~499）：**保留 BlockedAt 语义,禁用自恢复** → 末尾加 `, 0`。
- `t5_circuit_breaker_on_consecutive_fails`（~513）：**保留 CircuitBreaker 语义,禁用自恢复** → 末尾加 `, 0`。
- `stuck_needs_fix_when_only_needs_fix_and_nothing_ready`（~531）：验证 fresh 进程终态,禁用自恢复以走即时 StuckNeedsFix → 末尾加 `, 0`。

例（t4）：

```rust
        let out = run_background_cfg(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), "".into(), 3, 2, 8, 0,
        ).unwrap();
```

并把 Step 1 两个新测试的实参顺序落定为：

```rust
    let out = run_background_cfg(
        Arc::new(FlakyProvider { fail_until: 2, calls: std::sync::Mutex::new(0) }),
        "m".into(), 256, 0.0, dir.clone(), "".into(),
        5, 10, 8, 3,
    ).unwrap();
```

```rust
    let out = run_background_cfg(
        Arc::new(FlakyProvider { fail_until: 9999, calls: std::sync::Mutex::new(0) }),
        "m".into(), 256, 0.0, dir.clone(), "".into(),
        10, 10, 8, 2,
    ).unwrap();
```

- [ ] **Step 5: 跑新测试 + 既有测试确认全绿**

Run: `cargo test --lib background::tests`
Expected: PASS — 新的 `workgraph_auto_retries_needs_fix_until_pass`、`workgraph_gives_up_after_max_fix_attempts` 绿；`t4`(BlockedAt)、`t5`(CircuitBreaker)、`stuck_needs_fix_*`、两个 provider_error 测试维持绿。

- [ ] **Step 6: 全仓测试**

Run: `cargo test`
Expected: PASS（无回归；原 244 通过 + 新增测试）。

- [ ] **Step 7: 提交**

```bash
git add src/background.rs
git commit -m "feat(bg): wire budgeted needs_fix self-recovery into workgraph runner loop"
```

---

## Task 8: 文档与 ADR 同步

**Files:**
- Modify: `README.md`（环境变量表加 `CODECODER_BG_MAX_FIX_ATTEMPTS`）
- Modify: `ARCHITECTURE.md`（Background/workgraph 段落补自恢复循环描述）
- Modify: `CLAUDE.md`（Background Agent 段落：needs_fix 不再"需手动重置",改为"预算内自动重试,耗尽才 StuckNeedsFix"）
- Modify: `docs/adr/0026-background-agent-headless-runner.md` 与 `docs/adr/0033-bg-ledger-and-exit-codes.md`（补"needs_fix 自恢复循环"修订说明）
- Modify: `docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`（§6.4 footgun 标注为已修）

- [ ] **Step 1: README 环境变量表**——在 BG 相关 env 行附近新增一行：

```markdown
| `CODECODER_BG_MAX_FIX_ATTEMPTS` | `3` | headless workgraph 中单个 milestone 验收 needs_fix 后最多自动重试次数（0 = 禁用自恢复）。 |
```

- [ ] **Step 2: ARCHITECTURE.md**——在描述 workgraph 逐里程碑推进处补：

```markdown
里程碑验收 `needs_fix` 后，runner 在预算内（`CODECODER_BG_MAX_FIX_ATTEMPTS`，默认 3）自动把失败原因注入修复 prompt 重试；预算耗尽仍 `needs_fix` 才落 `StuckNeedsFix`（退出码 2）。重试计数 `fix_attempts` 持久化在 `workgraph.json` 的里程碑上，跨进程尊重预算。
```

- [ ] **Step 3: CLAUDE.md**——把「`needs_fix` 需手动重置 pending 才重试」更新为自恢复描述（保留 headless 只跑 `pending` 的说明，补充「needs_fix 由 runner 在预算内自动重试」）。

- [ ] **Step 4: ADR 0026 / 0033**——各加一段「修订（2026-07-23，迭代 1）」，记录：`fix_attempts`/`last_failure` 字段、`next_retryable`、`retry_one_milestone`、重试不计入 `max_auto`、`StuckNeedsFix` 仅在预算耗尽时落、CircuitBreaker 在启用自恢复时被 per-node 预算取代（列为已知取舍/后续可细化）。

- [ ] **Step 5: 评价报告 footgun §6.4**——把「`needs_fix` 需手动重置 pending 才重试」标注 `（已修：迭代 1 自恢复循环）`。

- [ ] **Step 6: 校验数字一致 + 全仓测试**

Run: `cargo test 2>&1 | tail -5`
Expected: PASS。核对 README/ARCHITECTURE/CLAUDE 中测试数量等描述与实际一致（若引用了具体测试数，按新增数更新）。

- [ ] **Step 7: 提交**

```bash
git add README.md ARCHITECTURE.md CLAUDE.md docs/adr/0026-background-agent-headless-runner.md docs/adr/0033-bg-ledger-and-exit-codes.md docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md
git commit -m "docs: sync self-recovery loop (ADR 0026/0033, README, ARCHITECTURE, CLAUDE)"
```

---

## 已知取舍 / 后续工作（不在本迭代）

- **CircuitBreaker 与自恢复的关系**：启用自恢复（`max_fix_attempts > 0`）时，跨里程碑的 `consecutive_fail` 熔断实际被 per-node 重试预算 + `max_auto` 取代（重试路径不累加 `consecutive_fail`）。这符合本迭代「用有界重试替代过早熔断」的目标；未来可让「耗尽预算的硬失败里程碑」累加一个独立计数以恢复跨里程碑熔断。
- **退避 backoff**：本迭代重试无显式时间退避（turn 本身有成本，且 `max_auto`/预算已兜底）。若接真实 LLM 后出现 rate-limit 抖动，可复用 `retry.rs` 分类器加退避——留待迭代 2/4。
- **失败原因粒度**：`last_failure` 存 gate_reason（命令门取 stderr 尾部、评审门取 verdict）。更结构化的失败证据（如 diff、具体失败测试名）留待 acceptance 契约化（迭代 3）。

---

## Self-Review

- **Spec coverage**：本计划覆盖 spec 迭代 1 的四个改动点——失败原因捕获（Task 5）、重试预算持久化（Task 1/2/6）、自恢复注入（Task 3/6/7）、退出码语义（Task 7 `StuckNeedsFix` 仅预算耗尽落）。L1 验收测试对应 spec 三条：`workgraph_auto_retries_needs_fix_until_pass`、`workgraph_gives_up_after_max_fix_attempts`、`build_repair_prompt_injects_failure_and_title`。
- **Placeholder scan**：无 TBD/TODO；每个改代码步骤均给出完整代码。Task 4 的 `let _ = &reason_for_persist;` 是**显式占位**，由 Task 5 Step 3 替换（已在文中标明）。
- **Type consistency**：`fix_attempts: usize` / `last_failure: Option<String>` / `next_retryable(usize) -> Option<&Milestone>` / `retry_one_milestone(…, max_fix_attempts: usize)` / `run_milestone_and_gate(…, milestone_id: u64, task_text: String, title: String) -> anyhow::Result<BgOutcome>` / `build_repair_prompt(&Milestone, &str) -> String` 在各 Task 间一致；`run_background_cfg` 新第 10 参数 `max_fix_attempts: usize` 在 Task 7 统一落定并更新全部 callsite。
