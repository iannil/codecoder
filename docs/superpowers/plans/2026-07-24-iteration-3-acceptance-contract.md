# 迭代 3：acceptance 契约化 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 milestone 加结构化可选 `command` 验收通道 + 写入引导，并用结构化 `GateKind` 记录每个里程碑走的是强命令门还是弱 review 门（随账本序列化），让弱信号验收可观测。

**Architecture:** `Milestone` 新增可选 `command`；`bg_gate` 用单一路由 `gate_command(m)`（显式 command 优先、旧数据 `extract_gate_command` 兜底）派生 `gate_kind(m) -> GateKind{Command|Review|None}`，`evaluate` 与 `run_milestone_and_gate` 共用该路由；`SubgoalOutcome` 加 `gate_kind` 随 `bg_ledger.jsonl` 序列化；`milestone add` 工具接受 `command` 并在缺强门时提示。

**Tech Stack:** Rust（无新依赖）；serde（新字段 `#[serde(default)]` 向后兼容，无 schema bump）；hermetic 测试。

## Global Constraints

- 不新增 crate 依赖。TDD；hermetic 测试。
- 新字段用 `#[serde(default)]`（+ `skip_serializing_if = "Option::is_none"` for `command`）→ 旧 `workgraph.json`/`bg_ledger.jsonl` 兼容，无 schema bump。
- `evaluate` 对 `command == None` 的既有用例行为**等价于当前**（`gate_command` 退回 `extract_gate_command`）。
- 保留 `extract_gate_command` 启发式作旧数据兜底。
- 写入引导只提示，**绝不**因缺 command 拒绝 `add`。
- `GateKind` 的默认值为 `None`（旧账本记录无门类信息时最保守）。
- 单一事实源：门路由决策只在 `gate_command`/`gate_kind` 一处，`evaluate` 与 `run_milestone_and_gate` 都调用它，不复制分支逻辑。
- 术语精确（CONTEXT.md）：milestone / acceptance / gate / verdict。

---

## 关键现状锚点

- `src/workgraph.rs:59` `Milestone`（字段止于 `command` 将加处：`fix_attempts`/`last_failure` 之后）；`add()`（179）的 `Milestone { … }` 字面量；`render()`（319，逐节点 `line.push_str`）。
- 其它 `Milestone { … }` 字面量：`src/bg_gate.rs` 测试 `ms()`（~173）、`src/workgraph.rs` 的 `migrate_todos` 与 test `validate_detects_cycle`。
- `src/bg_gate.rs`：`extract_gate_command`（29）、`run_command_gate`（58）、`evaluate`（84）、`GateVerdict`（16）。
- `src/background.rs`：`SubgoalOutcome`（20，字段 milestone_id/verdict/gate_reason/tool_cap_hit/touched_files）；`run_milestone_and_gate` 里 `evaluate` 调用（389）+ `SubgoalOutcome { … }` 构造（422）；测试 `SubgoalOutcome` 字面量（~652）。
- `src/bg_ledger.rs`：测试 `outcome()`（~179）构造 `SubgoalOutcome` 字面量。
- `src/tool/dev.rs:151` milestone `schema()`；`add` 分支（199-211）。
- `src/tool/reason.rs:173` `wg.add(&title,&acceptance,vec![])`（**不改** `add` 签名 → 不受影响）。

---

## Task 1: Milestone.command 字段 + render 显示

**Files:**
- Modify: `src/workgraph.rs`（`Milestone` 结构体、`add()` 字面量、`render()`、测试）
- Modify: `src/bg_gate.rs`（测试 `ms()` 字面量补 `command: None`）

**Interfaces:**
- Produces: `Milestone.command: Option<String>`；`render()` 对有 command 的节点显示 `  cmd:<command>`。

- [ ] **Step 1: 写失败测试**（`src/workgraph.rs` `mod tests`）

```rust
#[test]
fn new_milestone_command_defaults_none() {
    let mut g = WorkGraph::default();
    let a = g.add("a", "acc", vec![]).unwrap();
    assert_eq!(g.get(a).unwrap().command, None);
}

#[test]
fn render_shows_command_when_present() {
    let mut g = WorkGraph::default();
    let a = g.add("core", "acc", vec![]).unwrap();
    g.nodes.iter_mut().find(|n| n.id == a).unwrap().command = Some("cargo test".into());
    let r = g.render();
    assert!(r.contains("cmd:cargo test"), "render should show command: {r}");
}

#[test]
fn load_legacy_json_without_command_defaults_none() {
    let raw = format!(
        r#"{{"schema_version":{WG_SCHEMA_VERSION},"nodes":[{{"id":1,"title":"t","acceptance":"a","deps":[],"status":"pending","touched":[]}}]}}"#
    );
    let g = WorkGraph::load(&raw).unwrap();
    assert_eq!(g.get(1).unwrap().command, None);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib workgraph::tests::new_milestone_command_defaults_none`
Expected: FAIL — `no field command`。

- [ ] **Step 3: 加字段**（`src/workgraph.rs` `Milestone`，`last_failure` 之后）

```rust
    /// 可选客观验收命令(迭代 3):独占一行裸命令(如 `cargo test`)。存在→bg_gate 走命令门;
    /// 缺失→回退 extract_gate_command(acceptance) 启发式,再回退 review 门。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
```

- [ ] **Step 4: add() 字面量补字段**（`src/workgraph.rs` `add()`，`last_failure: None,` 之后）

```rust
            command: None,
```

- [ ] **Step 5: render 显示 command**（`src/workgraph.rs` `render()`，在 `if let Some(v) = &n.verdict { … }` 之后、`lines.push(line);` 之前）

```rust
            if let Some(c) = &n.command {
                line.push_str(&format!("  cmd:{c}"));
            }
```

- [ ] **Step 6: 编译修补其它 Milestone 字面量**

Run: `cargo build --tests`
Expected: 报 `missing field command` 于 `src/bg_gate.rs` 的 `ms()`、`src/workgraph.rs` 的 `migrate_todos` 与 `validate_detects_cycle`。在每处补 `command: None,`。修到通过。

- [ ] **Step 7: 跑测试确认通过**

Run: `cargo test --lib workgraph::tests`
Expected: PASS（3 个新测试 + 既有全绿）。

- [ ] **Step 8: 提交**

```bash
git add src/workgraph.rs src/bg_gate.rs
git commit -m "feat(workgraph): optional Milestone.command + render cmd: display"
```

---

## Task 2: bg_gate GateKind + 单一路由 + evaluate 重构

**Files:**
- Modify: `src/bg_gate.rs`（GateKind、gate_command、gate_kind、evaluate、测试）

**Interfaces:**
- Consumes: `Milestone.command`（Task 1）、`extract_gate_command`、`run_command_gate`。
- Produces: `pub enum GateKind { Command, Review, None }`（`Serialize`/`Deserialize`，`Default = None`）；`pub fn gate_command(m: &Milestone) -> Option<String>`；`pub fn gate_kind(m: &Milestone) -> GateKind`。`evaluate` 行为对 `command==None` 用例等价。

- [ ] **Step 1: 写失败测试**（`src/bg_gate.rs` `mod tests`）

```rust
#[test]
fn gate_command_prefers_explicit_over_extract() {
    let mut m = ms(1, "cargo test");   // acceptance 含可提取命令
    assert_eq!(gate_command(&m), Some("cargo test".into())); // 无显式 command → 用 extract
    m.command = Some("cargo build".into());
    assert_eq!(gate_command(&m), Some("cargo build".into())); // 显式 command 优先
}

#[test]
fn gate_kind_classifies() {
    let mut m = ms(1, "cargo test");
    assert_eq!(gate_kind(&m), GateKind::Command);      // 裸命令 acceptance → 兜底命令门
    m.command = Some("cargo build".into());
    assert_eq!(gate_kind(&m), GateKind::Command);      // 显式 command
    let prose = ms(2, "渲染输出正确");
    assert_eq!(gate_kind(&prose), GateKind::Review);   // prose
    let empty = ms(3, "");
    assert_eq!(gate_kind(&empty), GateKind::None);     // 空
}

#[test]
fn evaluate_uses_explicit_command_over_review() {
    let dir = tempdir().unwrap();
    let mut m = ms(1, "渲染输出正确");                 // prose acceptance
    m.command = Some("rustc --version".into());        // 显式命令(纯 ASCII,exit 0)
    // review_runner 若被调用会返回独特标记;命令门应先生效 → 不应等于该标记。
    let v = evaluate(&m, dir.path(), None, &|| GateVerdict::NeedsFix("REVIEW_RAN".into()));
    assert_ne!(v, GateVerdict::NeedsFix("REVIEW_RAN".into()), "explicit command gate should fire, not review");
}

#[test]
fn gate_kind_default_is_none() {
    assert_eq!(GateKind::default(), GateKind::None);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bg_gate::tests::gate_kind_classifies`
Expected: FAIL — `cannot find type GateKind` / `function gate_kind`。

- [ ] **Step 3: 加 GateKind + gate_command + gate_kind**（`src/bg_gate.rs`，`evaluate` 之前）

```rust
/// 本里程碑将走哪种验收门(迭代 3 可观测)。默认 None(旧账本记录无门类信息时最保守)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GateKind {
    Command,
    Review,
    None,
}

impl Default for GateKind {
    fn default() -> Self {
        GateKind::None
    }
}

/// 客观命令门要跑的命令:显式 `command` 优先,旧数据裸命令启发式(extract_gate_command)兜底。
pub fn gate_command(m: &Milestone) -> Option<String> {
    m.command.clone().or_else(|| extract_gate_command(&m.acceptance))
}

/// 门路由决策(单一事实源):有命令→Command;否则 acceptance 空→None;否则→Review。
pub fn gate_kind(m: &Milestone) -> GateKind {
    if gate_command(m).is_some() {
        GateKind::Command
    } else if m.acceptance.trim().is_empty() {
        GateKind::None
    } else {
        GateKind::Review
    }
}
```

- [ ] **Step 4: 重构 evaluate 走单一路由**（`src/bg_gate.rs` `evaluate` 函数体替换）

```rust
pub fn evaluate(
    m: &Milestone,
    root: &Path,
    cancel: Option<&CancelToken>,
    review_runner: &dyn Fn() -> GateVerdict,
) -> GateVerdict {
    match gate_kind(m) {
        GateKind::Command => {
            let cmd = gate_command(m).expect("gate_kind==Command ⇒ gate_command is Some");
            run_command_gate(&cmd, root, cancel)
        }
        GateKind::None => GateVerdict::Inconclusive("no acceptance criterion (weak signal)".into()),
        GateKind::Review => review_runner(),
    }
}
```

- [ ] **Step 5: 跑测试确认通过（含既有 evaluate 用例不回退）**

Run: `cargo test --lib bg_gate::tests`
Expected: PASS — 4 个新测试绿；既有 `evaluate_uses_command_gate_when_present`、`evaluate_falls_back_to_review_runner`、`evaluate_inconclusive_when_acceptance_empty`、`extract_gate_command_*` 全绿（command==None 时等价）。

- [ ] **Step 6: 提交**

```bash
git add src/bg_gate.rs
git commit -m "feat(bg_gate): GateKind + single-source gate_command/gate_kind routing"
```

---

## Task 3: SubgoalOutcome.gate_kind + 账本可观测

**Files:**
- Modify: `src/background.rs`（`SubgoalOutcome` 字段、`run_milestone_and_gate` 填入、测试字面量）
- Modify: `src/bg_ledger.rs`（测试 `outcome()` 字面量补字段 + 旧-JSON 兼容测试）

**Interfaces:**
- Consumes: `bg_gate::GateKind`、`bg_gate::gate_kind`（Task 2）。
- Produces: `SubgoalOutcome.gate_kind: crate::bg_gate::GateKind`（`#[serde(default)]`）。

- [ ] **Step 1: 写失败测试**（`src/bg_ledger.rs` `mod tests`）

```rust
#[test]
fn subgoal_outcome_serializes_gate_kind() {
    let sg = SubgoalOutcome {
        milestone_id: 1,
        verdict: SubgoalVerdict::Pass,
        gate_reason: "gate pass".into(),
        tool_cap_hit: false,
        touched_files: vec![],
        gate_kind: crate::bg_gate::GateKind::Command,
    };
    let j = serde_json::to_string(&sg).unwrap();
    assert!(j.contains("Command"), "gate_kind should serialize: {j}");
}

#[test]
fn legacy_subgoal_json_without_gate_kind_defaults_none() {
    // 旧账本记录缺 gate_kind 字段 → 反序列化落 GateKind::None。
    let j = r#"{"milestone_id":1,"verdict":"Pass","gate_reason":"x","tool_cap_hit":false,"touched_files":[]}"#;
    let sg: SubgoalOutcome = serde_json::from_str(j).unwrap();
    assert_eq!(sg.gate_kind, crate::bg_gate::GateKind::None);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bg_ledger::tests::legacy_subgoal_json_without_gate_kind_defaults_none`
Expected: FAIL — `no field gate_kind` / `missing field`。

- [ ] **Step 3: 加字段**（`src/background.rs` `SubgoalOutcome`，`touched_files` 之后）

```rust
    /// 本里程碑实际走的验收门类型(迭代 3 可观测)。旧账本记录缺此字段 → 默认 None。
    #[serde(default)]
    pub gate_kind: crate::bg_gate::GateKind,
```

- [ ] **Step 4: run_milestone_and_gate 填入**（`src/background.rs` `SubgoalOutcome { … }` 构造，422 附近；`m` 是已读的 Milestone clone）

在该字面量补一行：
```rust
        gate_kind: crate::bg_gate::gate_kind(&m),
```

- [ ] **Step 5: 编译修补其它 SubgoalOutcome 字面量**

Run: `cargo build --tests`
Expected: 报 `missing field gate_kind` 于 `src/background.rs` 测试（~652）与 `src/bg_ledger.rs` 测试 `outcome()`（~179）。各补 `gate_kind: crate::bg_gate::GateKind::None,`（测试语义无关，取默认）。修到通过。

- [ ] **Step 6: 跑测试确认通过 + 无回归**

Run: `cargo test --lib bg_ledger::tests && cargo test --lib background::tests`
Expected: PASS（新测试 + 既有全绿）。

- [ ] **Step 7: 提交**

```bash
git add src/background.rs src/bg_ledger.rs
git commit -m "feat(bg): SubgoalOutcome.gate_kind recorded to ledger (weak-signal observability)"
```

---

## Task 4: milestone 工具 command 参数 + 写入引导

**Files:**
- Modify: `src/tool/dev.rs`（`schema()`、`add` 分支、测试）

**Interfaces:**
- Consumes: `Milestone.command`（Task 1）、`bg_gate::gate_command`（Task 2）。

- [ ] **Step 1: 写失败测试**（`src/tool/dev.rs` `mod tests`；沿用该模块既有的工具调用测试风格 — 通过 `Milestone.run` + `ToolCtx`，或直接 `WorkGraph`。下面用工具 `run` 走 `add`）

```rust
#[test]
fn milestone_add_with_command_sets_and_renders() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolCtx::new(dir.path());
    let out = Milestone.run(
        json!({"action":"add","title":"core","acceptance":"渲染正确","command":"cargo test"}),
        &mut ctx,
    ).unwrap();
    assert!(!out.is_error);
    assert!(out.content.contains("cmd:cargo test"), "render should show command: {}", out.content);
    // 有强门 → 不提示。
    assert!(!out.content.contains("review gate"), "should not warn when command present: {}", out.content);
}

#[test]
fn milestone_add_prose_only_emits_guidance() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolCtx::new(dir.path());
    let out = Milestone.run(
        json!({"action":"add","title":"ui","acceptance":"渲染输出正确"}),
        &mut ctx,
    ).unwrap();
    assert!(out.content.contains("review gate"), "prose-only acceptance should warn: {}", out.content);
}

#[test]
fn milestone_add_bare_command_acceptance_no_guidance() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolCtx::new(dir.path());
    let out = Milestone.run(
        json!({"action":"add","title":"core","acceptance":"cargo test"}),
        &mut ctx,
    ).unwrap();
    // acceptance 是裸命令 → extract 兜底给强门 → 不提示。
    assert!(!out.content.contains("review gate"), "bare-command acceptance should not warn: {}", out.content);
}
```

（若 `ToolCtx::new` / `ToolOutput.content` / `ToolOutput.is_error` 字段名与该模块既有测试不符，以既有测试的用法为准 —— 读 `src/tool/dev.rs` 现有 `#[cfg(test)]` 里 `Milestone` 调用样例对齐。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib dev::tests::milestone_add_with_command_sets_and_renders`
（或该测试模块的实际路径，如 `tool::dev::tests::…`；按现有测试运行方式。）
Expected: FAIL — `command` 未被设置 / 无引导文案。

- [ ] **Step 3: schema 加 command**（`src/tool/dev.rs` `schema()` 的 `properties`，`acceptance` 之后）

```rust
                "command": { "type": "string", "description": "Bare runnable acceptance command (e.g. `cargo test`) — objective gate; prefer over prose acceptance." },
```

- [ ] **Step 4: add 分支设置 command + 引导**（`src/tool/dev.rs` `apply` 的 `"add" =>` 分支替换）

```rust
            "add" => {
                let title = args.get("title").and_then(Value::as_str).unwrap_or_default();
                let acceptance = args.get("acceptance").and_then(Value::as_str).unwrap_or_default();
                let command = args
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let deps: Vec<u64> = args
                    .get("deps")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default();
                match g.add(title, acceptance, deps) {
                    Ok(new) => {
                        if let Some(cmd) = command {
                            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == new) {
                                n.command = Some(cmd.to_string());
                            }
                        }
                        let mut msg = format!("added #{new}\n{}", g.render());
                        // 写入引导:无强门(既无显式 command 也无可提取命令)且 acceptance 非空 → 提示。
                        let node = g.get(new).expect("just added");
                        if crate::bg_gate::gate_command(node).is_none() && !node.acceptance.trim().is_empty() {
                            msg.push_str(
                                "\nnote: no runnable command; this milestone will use the weaker \
                                 review gate. Pass `command` (e.g. \"cargo test\") for an objective gate.",
                            );
                        }
                        ToolOutput::ok(msg)
                    }
                    Err(e) => ToolOutput::err(e.to_string()),
                }
            }
```

- [ ] **Step 5: 跑测试确认通过 + 无回归**

Run: `cargo test --lib` （聚焦 dev/milestone 测试）
Expected: PASS — 3 个新测试绿；既有 `milestone_add_deps_and_done_gates_next` 等不回退。

- [ ] **Step 6: 提交**

```bash
git add src/tool/dev.rs
git commit -m "feat(tool): milestone add accepts command + guides toward objective gate"
```

---

## Task 5: 文档 + ADR 0030 修订

**Files:**
- Modify: `docs/adr/0030-bg-objective-acceptance-gate.md`（追加迭代 3 修订）
- Modify: `README.md`（若列 milestone 工具/参数，补 `command`）
- Modify: `ARCHITECTURE.md`（若述验收门，补结构化 acceptance/gate_kind 一句）
- Modify: `docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`（§5 Issue A 补注）

- [ ] **Step 1: 核对代码事实**

Run: `grep -n "GateKind\|gate_command\|gate_kind\|pub command" src/bg_gate.rs src/workgraph.rs src/background.rs | head`
Expected: 确认 GateKind/gate_command/gate_kind/command/gate_kind 字段就位。

- [ ] **Step 2: ADR 0030 追加修订段**

```markdown
## 修订（2026-07-24，迭代 3：acceptance 契约化）

- Milestone 新增可选结构化 `command`（客观验收命令）。`bg_gate::gate_command(m)` = 显式 `command` 优先、旧数据 `extract_gate_command(acceptance)` 兜底；`gate_kind(m) -> GateKind{Command|Review|None}` 为单一路由事实源，`evaluate` 与 `run_milestone_and_gate` 共用。
- `SubgoalOutcome.gate_kind` 随 `bg_ledger.jsonl` 序列化 → 编排者可过滤「弱信号（Review/None）通过」的里程碑；旧记录 `#[serde(default)]` 落 `None`。
- `milestone add` 接受 `command`；缺强门且 acceptance 非空时提示建议传 command（只提示，不拒绝）。
- 行为对无 `command` 的既有里程碑等价（退回原 extract 启发式）。
```

- [ ] **Step 3: README / ARCHITECTURE / 评估报告**

- README：milestone 工具参数处补 `command`（若无该表则跳过，报告说明）。
- ARCHITECTURE：验收门描述处补一句「acceptance 支持结构化 command 通道，gate_kind 记录强/弱门」。
- 评估报告 §5 Issue A 末补：`（迭代 3 已契约化：结构化 command 通道 + 写入引导 + gate_kind 可观测）`。

- [ ] **Step 4: 全仓测试 + 数字核对**

Run: `cargo test 2>&1 | tail -3`
Expected: PASS。若文档引用测试计数，按实更新。

- [ ] **Step 5: 提交**

```bash
git add docs/adr/0030-bg-objective-acceptance-gate.md README.md ARCHITECTURE.md docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md
git commit -m "docs: acceptance contract-ization (ADR 0030 revision, README, ARCHITECTURE)"
```

---

## Self-Review

- **Spec coverage**：改动点 1（command 字段）=Task 1；改动点 2（GateKind + 单一路由 + evaluate）=Task 2；改动点 3（SubgoalOutcome.gate_kind + 账本）=Task 3；改动点 4（工具 command + 引导）=Task 4；改动点 5（render）=Task 1；文档/ADR=Task 5。测试覆盖 spec §4 全部：gate_command 优先级、gate_kind 四类、evaluate 显式命令优先、command 默认/旧JSON、工具三态引导、gate_kind 序列化 + 旧账本默认。
- **Placeholder scan**：无 TBD；每改代码步骤给出完整代码。Task 4 Step 1 括注「以既有测试用法为准」是对 `ToolCtx`/`ToolOutput` API 的对齐提示（该 API 已存在于代码，非需求占位）。
- **Type consistency**：`GateKind{Command,Review,None}` + `Default=None`、`gate_command(&Milestone)->Option<String>`、`gate_kind(&Milestone)->GateKind`、`Milestone.command: Option<String>`、`SubgoalOutcome.gate_kind: crate::bg_gate::GateKind` 跨 Task 一致；`evaluate` 签名不变。
- **已知取舍**：门路由逻辑集中在 `gate_command`/`gate_kind`（bg_gate），`evaluate`/工具引导/`run_milestone_and_gate` 均调用它 → 单一事实源，无分支复制。
