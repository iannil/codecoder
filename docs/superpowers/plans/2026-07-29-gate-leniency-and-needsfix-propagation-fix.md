# 验收门宽容与 needs_fix 推进修复 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复两个阻塞性问题：(1) 验收门空 acceptance 时标记 Inconclusive 但代码实际已通过构建；(2) needs_fix 里程碑阻塞整个推进循环，不允许其他 pending 里程碑继续推进。

**Architecture:** 两处局部修改，不引入新模块：
- `bg_gate.rs` — `evaluate()` 对 `GateKind::None` 增加宽容判定：当 milestone 有 `touched` 文件且 `command` 字段存在时，降级走命令门而非直接 Inconclusive
- `background.rs` — milestone 推进循环中，needs_fix 里程碑即使有重试预算也不应阻塞其他 pending 里程碑的推进

**Tech Stack:** Rust（纯原生，无新依赖）

## 全局约束

- 所有改动必须通过 `cargo test` 全部测试（当前 348 pass / 3 ignore）
- 向后兼容：旧 workgraph.json（无 `touched` 字段的里程碑）行为不变
- stdout/stderr 输出风格与现有代码保持一致（英文日志、info 层用 eprintln）
- 不允许增加新的第三方依赖

---

### Task 1: 验收门宽容 — GateKind::None 时有 touched 文件则降级命令门

**问题背景：** `generate_milestones` 工具生成的里程碑缺少 `acceptance` 字段，导致 `gate_kind()` 返回 `GateKind::None`，`evaluate()` 直接返回 `Inconclusive("no acceptance criterion (weak signal)")`。

但实际代码已写完且 `npm run build` 通过。如果 milestone 有 `touched` 文件（证明代码已被修改），且 `command` 字段存在，应降级走命令门。

**Files:**
- Modify: `src/bg_gate.rs` — 修改 `evaluate()` 函数

**Interfaces:**
- Consumes: `Milestone.command`、`Milestone.touched`、`Milestone.acceptance`（已有字段）
- Produces: `evaluate()` 对 `GateKind::None` 增加宽容判定

- [ ] **Step 1: 修改 `evaluate()` 函数，GateKind::None 时增加宽容降级**

当前 `evaluate()` 的 `GateKind::None` 分支（第 196 行）：

```rust
GateKind::None => GateVerdict::Inconclusive("no acceptance criterion (weak signal)".into()),
```

改为：如果 milestone 有 `command` 字段且 `touched` 非空，降级走命令门；否则保持原 Inconclusive：

```rust
GateKind::None => {
    // 宽容模式: milestone 有显式 command + touched 文件(证明已产生代码)时,
    // 降级跑命令门验收,而非直接 Inconclusive。这解决 seed 阶段 generate_milestones
    // 生成空 acceptance 里程碑时,代码已通过构建但验收门仍标记为 needs_fix 的问题。
    if let Some(cmd) = &m.command {
        if !m.touched.is_empty() {
            let verdict = run_command_gate(cmd, root, cancel);
            // 命令门 pass 后执行 checks（Phase 1）
            if verdict == GateVerdict::Pass {
                if let Some(checks) = &m.checks {
                    if !checks.is_empty() {
                        if let Err(errors) = run_checks(checks, root) {
                            let detail = errors.join("; ");
                            return GateVerdict::NeedsFix(format!("command passed but checks failed: {detail}"));
                        }
                    }
                }
            }
            return verdict;
        }
    }
    GateVerdict::Inconclusive("no acceptance criterion (weak signal)".into())
}
```

- [ ] **Step 2: 编写单元测试**

```rust
#[test]
fn evaluate_none_with_command_and_touched_runs_command_gate() {
    let dir = tempdir().unwrap();
    let mut m = ms(1, "");
    m.command = Some("echo ok".into());
    m.touched = vec!["src/foo.tsx".into()];
    let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
    assert_eq!(v, GateVerdict::Pass);
}

#[test]
fn evaluate_none_without_touched_stays_inconclusive() {
    let dir = tempdir().unwrap();
    let mut m = ms(1, "");
    m.command = Some("echo ok".into());
    let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
    assert!(matches!(v, GateVerdict::Inconclusive(_)));
}

#[test]
fn evaluate_none_without_command_stays_inconclusive() {
    let dir = tempdir().unwrap();
    let mut m = ms(1, "");
    m.touched = vec!["src/foo.tsx".into()];
    let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
    assert!(matches!(v, GateVerdict::Inconclusive(_)));
}
```

- [ ] **Step 3: 运行测试确认全部通过**

```bash
cargo test bg_gate 2>&1
```

Expected: 全部 bg_gate 测试 pass。

- [ ] **Step 4: 提交**

```bash
git add -A && git commit -m "fix(bg_gate): lenient gate for milestones with touched files + command

When acceptance is empty but the milestone has both a command field
and touched files (proving code was produced), degrade to running the
command gate instead of immediately returning Inconclusive. This
fixes the scenario where generate_milestones creates milestones
without acceptance criteria, the agent writes real code that builds,
but the gate still marks it as needs_fix.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: needs_fix 不阻塞 pending 里程碑推进

**问题背景：** 当前推进循环（`background.rs` 第 248-370 行）的逻辑是：
1. 有就绪 pending → 推进
2. 无就绪 pending → 尝试重试 needs_fix
3. 无可重试 needs_fix → 结束

当 M6 needs_fix 但 M7 仍然是 pending 时，循环应该能推进 M7。但当前代码中，M6 的 Inconclusive 导致 `fix_attempts` 未递增（因为 Inconclusive 和 NeedsFix 都走 `NodeStatus::NeedsFix`，但 `fix_attempts` 在 `retry_one_milestone` 的 `bump` 中递增），所以 `next_retryable` 一直返回 M6，导致循环卡在 M6 重试上。

**修复方案：** 在 `next_retryable` 中排除 `gate_kind == None` 的里程碑，因为这类里程碑的 failures 不是"代码问题"而是"验收标准缺失"，重试也无法解决。

- [ ] **Step 1: 修改 `next_retryable` 排除 GateKind::None 的里程碑**

```rust
pub fn next_retryable(&self, max_attempts: usize) -> Option<&Milestone> {
    self.nodes
        .iter()
        .filter(|n| {
            n.status == NodeStatus::NeedsFix
                && self.deps_done(n)
                && n.fix_attempts < max_attempts
                // 排除 GateKind::None 的里程碑 — 这类 failures 是"验收标准缺失"
                // 而非代码问题,重试也无法解决。允许其他 pending 里程碑继续推进。
                && crate::bg_gate::gate_kind(n) != crate::bg_gate::GateKind::None
        })
        .min_by_key(|n| n.id)
}
```

- [ ] **Step 2: 在推进循环中，当 needs_fix 里程碑不可重试时，检查是否有其他 pending 里程碑**

在 `background.rs` 的推进循环中，`retry_one_milestone` 返回 `Ok(None)` 时，当前逻辑是立即结束。但此时可能还有 pending 里程碑未被阻塞（M7 不依赖 M6）。

修改 `background.rs` 第 271 行附近（`retry_one_milestone` 返回 `Ok(None)` 的分支）：

```rust
Ok(None) => {
    // 无可重试 needs_fix。先检查是否有 pending 里程碑可推进。
    let has_pending = {
        let g = crate::workgraph::WorkGraph::read(&root);
        g.nodes.iter().any(|n| n.status == crate::workgraph::NodeStatus::Pending)
    };
    if has_pending {
        // 有 pending 里程碑但当前无 ready(可能被阻塞)→继续循环让下一轮
        // advance_one_milestone 的 next_ready() 自行判断。
        continue;
    }
    // 既无就绪、也无可重试 needs_fix → 终态。...
    if out.mission_state == crate::bg_gate::MissionState::Running {
        // ... 原有逻辑 ...
    }
    break;
}
```

- [ ] **Step 3: 编写单元测试**

```rust
#[test]
fn next_retryable_skips_gate_kind_none() {
    let mut m = ms(100, "");
    m.status = NodeStatus::NeedsFix;
    m.fix_attempts = 0;
    m.command = Some("echo ok".into());
    let mut m2 = ms(101, "");
    m2.status = NodeStatus::NeedsFix;
    m2.fix_attempts = 0;
    let g = graph_with(vec![m, m2]);
    let retryable = g.next_retryable(3);
    assert!(retryable.is_some());
    assert_eq!(retryable.unwrap().id, 100);
}
```

- [ ] **Step 4: 运行测试确认全部通过**

```bash
cargo test workgraph 2>&1
cargo test bg_gate 2>&1
cargo test background 2>&1
```

Expected: 所有测试 pass。

- [ ] **Step 5: 提交**

```bash
git add -A && git commit -m "fix(workgraph): skip GateKind::None milestones in retry loop

Milestones that failed acceptance due to empty acceptance criteria
(GateKind::None) are not code problems — retrying them is futile.
Exclude them from next_retryable so the loop can advance other
pending milestones instead of getting stuck retrying the same
inconclusive milestone.

Also ensure that when retryable is None but pending milestones
exist, the loop continues rather than exiting prematurely.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### 任务依赖关系

```
Task 1 (验收门宽容)
  └──── 独立

Task 2 (needs_fix 推进修复)
  └──── 独立

两个 task 可并行执行。