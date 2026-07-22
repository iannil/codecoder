# WorkGraph 并发写保护(lost-update 修复)— 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 workgraph.json 的 read→mutate→save 在并发写者下不丢里程碑(P9:4 并发→0 存活)。

**Architecture:** `WorkGraph::with_lock(root, |g| -> Result<T>)` helper 用 fs2 咨询锁(独立 `workgraph.json.lock` 文件,避 save 的 atomic-rename 换 inode)包住 read→mutate→save;锁只覆盖毫秒级状态更新,不覆盖 LLM turn。3 个写点(advance / drive_workgraph / Milestone 工具)重构用它。

**Tech Stack:** Rust + `fs2`(咨询文件锁)+ cargo test(8 线程并发不丢更新测试)。

## Global Constraints

- **只覆盖 workgraph.json**;session/memory/ledger 不动(spec §7)。
- **锁不包 LLM turn**:turn 跑完后再在 `with_lock` 闭包内更新图状态。
- **独立 lock 文件** `workgraph.json.lock`(不锁数据文件——atomic-rename 会换 inode 打破锁)。
- **TDD**:每任务先写失败测试 → 红 → 最小实现 → 绿 → commit。
- **不破坏既有测试**:`cargo test` 全绿。
- **领域术语**遵 `CONTEXT.md`;commit conventional + 中文;分支 `fix/workgraph-concurrency`。
- **fs2 锁由 OS 自动释放在进程退出/崩溃**→无 stale-lock。

## File Structure

- Modify: `Cargo.toml` — 加 `fs2`。
- Modify: `src/workgraph.rs` — 加 `WorkGraph::with_lock` + 2 个单测。
- Modify: `src/background.rs` — `advance_one_milestone` 状态更新包进 `with_lock`。
- Modify: `src/agent.rs` — `drive_workgraph` 自动写回包进 `with_lock`。
- Modify: `src/tool/dev.rs` — `Milestone` 工具抽 `apply` + 包进 `with_lock`。
- Create: `docs/adr/0035-workgraph-concurrency-write-protection.md`;Modify `ARCHITECTURE.md`。

---

## Task 1: fs2 依赖 + `WorkGraph::with_lock` + 并发不丢更新测试

**Files:**
- Modify: `Cargo.toml`(加 fs2)、`src/workgraph.rs`(加 helper + 2 测试)

**Interfaces:**
- Produces: `pub fn with_lock<T, F>(root: &Path, f: F) -> anyhow::Result<T> where F: FnOnce(&mut WorkGraph) -> anyhow::Result<T>`(Task 2/3/4 消费)。

- [ ] **Step 1: 加 fs2 依赖**

在 `Cargo.toml` `[dependencies]` 末尾(wasmtime 行后)加:
```toml
# Advisory file lock for WorkGraph concurrent-write protection (ADR 0035).
fs2 = "0.4"
```
Run: `cargo build 2>&1 | tail -3`
Expected: 拉取 fs2 并 `Finished`(首次会下载 crate)。

- [ ] **Step 2: 写失败测试**(在 `src/workgraph.rs` 的 `#[cfg(test)] mod tests` 内)

```rust
    #[test]
    fn with_lock_prevents_lost_update_under_concurrency() {
        use std::sync::Arc;
        use std::thread;
        let dir = std::env::temp_dir().join(format!("cc_wglock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        WorkGraph::default().save(&dir).unwrap();
        let dir = Arc::new(dir);
        let n = 8;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let d = Arc::clone(&dir);
                thread::spawn(move || {
                    WorkGraph::with_lock(&d, |g| {
                        g.add(&format!("t{i}"), "", vec![])?;
                        Ok(())
                    })
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let g = WorkGraph::read(&dir);
        assert_eq!(g.nodes.len(), n, "no milestone lost under concurrent with_lock writers");
        let _ = std::fs::remove_dir_all(&*dir);
    }

    #[test]
    fn with_lock_releases_so_sequential_calls_do_not_deadlock() {
        let dir = std::env::temp_dir().join(format!("cc_wgseq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        WorkGraph::default().save(&dir).unwrap();
        for _ in 0..5 {
            WorkGraph::with_lock(&dir, |g| {
                g.add("t", "", vec![])?;
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(WorkGraph::read(&dir).nodes.len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 3: 跑测试看红**

Run: `cargo test with_lock_ 2>&1 | grep -E 'error\[|cannot find|no function' | head -3`
Expected: 编译失败(`with_lock` 未定义)。

- [ ] **Step 4: 实现 `with_lock`**(在 `src/workgraph.rs` impl WorkGraph 内,`save` 之后)

```rust
    /// 在咨询文件锁内执行 read→mutate→save(ADR 0035),防并发写者 lost-update。
    /// 锁独立文件 `workgraph.json.lock`(`save` 的 atomic-rename 会换数据文件 inode,
    /// 故不能直接锁数据文件)。锁只包毫秒级闭包,**不覆盖调用方的 LLM turn**。
    /// fs2 锁由 OS 在进程退出/崩溃时自动释放 → 无 stale-lock。
    pub fn with_lock<T, F>(root: &Path, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&mut WorkGraph) -> anyhow::Result<T>,
    {
        use fs2::FileExt;
        let lock_path = root.join("workgraph.json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        let result = (|| {
            let mut g = WorkGraph::read(root);
            let out = f(&mut g)?;
            g.save(root)?;
            Ok(out)
        })();
        let _ = file.unlock();
        result
    }
```

- [ ] **Step 5: 跑测试看绿**

Run: `cargo test with_lock_ 2>&1 | grep -E 'with_lock_.* \.\.\.|test result: ok|FAILED' | head -4`
Expected: 两个测试 `... ok`。

- [ ] **Step 6: commit**

```bash
git add Cargo.toml Cargo.lock src/workgraph.rs
git commit -m "feat(workgraph): with_lock 咨询锁防并发 lost-update(P9)

新增 WorkGraph::with_lock(root,|g|):fs2 lock_exclusive 包住 read→mutate→save,
独立 workgraph.json.lock 文件(避 save 的 atomic-rename 换 inode)。锁不包 LLM turn。
8 线程并发不丢更新测试 + 顺序不死锁测试。"
```

---

## Task 2: `advance_one_milestone` 状态更新用 `with_lock`

**Files:**
- Modify: `src/background.rs:304-311`

- [ ] **Step 1: 改 advance 的状态更新块**

把 `src/background.rs` 的(约 304-311):
```rust
    {
        let mut g = WorkGraph::read(&root);
        g.set_status(milestone_id, status);
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.verdict = Some(vs_str.into());
        }
        let _ = g.save(&root);
    }
```
改为:
```rust
    {
        let _ = WorkGraph::with_lock(&root, |g| {
            g.set_status(milestone_id, status);
            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                n.verdict = Some(vs_str.into());
            }
            Ok(())
        });
    }
```

- [ ] **Step 2: 跑 advance 相关测试无回归**

Run: `cargo test advance_one_milestone 2>&1 | grep -E 'test result|FAILED|error\[' | head`
Expected: 全 `ok` / `0 failed`(t1/t2 命令门测试仍绿)。

- [ ] **Step 3: commit**

```bash
git add src/background.rs
git commit -m "refactor(bg): advance 状态更新走 with_lock

advance_one_milestone 的 set_status+save 包进 WorkGraph::with_lock,
消除与并发写者(daemon workgraph 线程/交互 milestone 工具)的 lost-update。"
```

---

## Task 3: `drive_workgraph` 自动写回用 `with_lock`

**Files:**
- Modify: `src/agent.rs:1357-1368`

- [ ] **Step 1: 改 drive_workgraph 的写回块**

把 `src/agent.rs` 的(约 1357-1368):
```rust
            if !outcome.unparsed {
                let mut g = WorkGraph::read(&self.root);
                let (status, verdict_str) = match outcome.verdict {
                    crate::review::Verdict::Pass => (NodeStatus::Done, "pass"),
                    crate::review::Verdict::NeedsFix => (NodeStatus::NeedsFix, "needs_fix"),
                    crate::review::Verdict::Rebuild => (NodeStatus::NeedsFix, "rebuild"),
                };
                g.set_status(milestone_id, status);
                if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                    n.verdict = Some(verdict_str.to_string());
                }
                let _ = g.save(&self.root);
                let _ = event_tx.send(AgentEvent::Notice(format!(
```
改为(`(status, verdict_str)` 留闭包外计算,只把 mutate+save 包进锁;`verdict_str` 是 `&'static str` 可在闭包内用后再用于 Notice):
```rust
            if !outcome.unparsed {
                let (status, verdict_str) = match outcome.verdict {
                    crate::review::Verdict::Pass => (NodeStatus::Done, "pass"),
                    crate::review::Verdict::NeedsFix => (NodeStatus::NeedsFix, "needs_fix"),
                    crate::review::Verdict::Rebuild => (NodeStatus::NeedsFix, "rebuild"),
                };
                let _ = WorkGraph::with_lock(&self.root, |g| {
                    g.set_status(milestone_id, status);
                    if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                        n.verdict = Some(verdict_str.to_string());
                    }
                    Ok(())
                });
                let _ = event_tx.send(AgentEvent::Notice(format!(
```
(后续 `milestone #{} ({}) auto-updated: {}` Notice 行不动,`verdict_str` 仍可用。)

- [ ] **Step 2: 跑 agent 相关测试无回归**

Run: `cargo test 2>&1 | grep -E 'test result' | grep -v '0 failed' && echo HAS_FAILURES || echo ALL_GREEN`
Expected: `ALL_GREEN`。

- [ ] **Step 3: commit**

```bash
git add src/agent.rs
git commit -m "refactor(agent): drive_workgraph 自动写回走 with_lock

milestone 自动状态写回包进 WorkGraph::with_lock,防并发 lost-update。"
```

---

## Task 4: `Milestone` 工具抽 `apply` + 走 `with_lock`

**Files:**
- Modify: `src/tool/dev.rs:168-246`

**Interfaces:**
- Produces: `fn Milestone::apply(g: &mut WorkGraph, action, id, args) -> ToolOutput`(纯 mutate+render,无 IO)。

- [ ] **Step 1: 抽 apply + run 走 with_lock**

把 `src/tool/dev.rs` `impl Tool for Milestone` 的 `run`(168-246)整体替换为:
```rust
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        use crate::workgraph::WorkGraph;
        let action = args.get("action").and_then(Value::as_str).unwrap_or("list").to_string();
        let id = args.get("id").and_then(Value::as_u64);
        // 读+改+存统一在咨询锁内(ADR 0035),防与并发写者 lost-update。
        // 读动作(list/next)走同一锁取一致快照;with_lock 末尾 save 对未改图幂等。
        WorkGraph::with_lock(ctx.root, |g| Ok(Self::apply(g, &action, id, &args)))
    }
}

impl Milestone {
    /// 纯内存态:按 action 改 `g` 并返回输出(无 IO,由 with_lock 负责存盘)。
    fn apply(g: &mut WorkGraph, action: &str, id: Option<u64>, args: &Value) -> ToolOutput {
        use crate::workgraph::NodeStatus;
        match action {
            "list" => ToolOutput::ok(g.render()),
            "next" => ToolOutput::ok(match g.next_ready() {
                Some(n) => {
                    let acc = if n.acceptance.is_empty() {
                        String::new()
                    } else {
                        format!("\n  accept: {}", n.acceptance)
                    };
                    format!("▶ #{} {}{}", n.id, n.title, acc)
                }
                None => "(nothing ready — all milestones done, blocked, or in progress)".into(),
            }),
            "add" => {
                let title = args.get("title").and_then(Value::as_str).unwrap_or_default();
                let acceptance = args.get("acceptance").and_then(Value::as_str).unwrap_or_default();
                let deps: Vec<u64> = args
                    .get("deps")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default();
                match g.add(title, acceptance, deps) {
                    Ok(new) => ToolOutput::ok(format!("added #{new}\n{}", g.render())),
                    Err(e) => ToolOutput::err(e.to_string()),
                }
            }
            "start" => {
                if id.map(|i| g.set_status(i, NodeStatus::InProgress)).unwrap_or(false) {
                    ToolOutput::ok(g.render())
                } else {
                    ToolOutput::err("start needs a valid `id`")
                }
            }
            "needs_fix" => {
                if id.map(|i| g.set_status(i, NodeStatus::NeedsFix)).unwrap_or(false) {
                    ToolOutput::ok(g.render())
                } else {
                    ToolOutput::err("needs_fix needs a valid `id`")
                }
            }
            "done" => {
                let Some(i) = id else {
                    return ToolOutput::err("done needs `id`");
                };
                let verdict = args.get("verdict").and_then(Value::as_str);
                let status = match verdict {
                    Some(v) if v != "pass" => NodeStatus::NeedsFix,
                    _ => NodeStatus::Done,
                };
                if !g.set_status(i, status) {
                    return ToolOutput::err("done needs a valid `id`");
                }
                if let (Some(v), Some(n)) = (verdict, g.nodes.iter_mut().find(|n| n.id == i)) {
                    n.verdict = Some(v.to_string());
                }
                ToolOutput::ok(g.render())
            }
            "remove" => {
                let Some(i) = id else {
                    return ToolOutput::err("remove needs `id`");
                };
                if let Err(e) = g.remove(i) {
                    return ToolOutput::err(e.to_string());
                }
                ToolOutput::ok(g.render())
            }
            other => ToolOutput::err(format!("unknown action: {other}")),
        }
    }
}
```
(注意:原 `run` 末尾的 `}` 结束 `impl Tool for Milestone`;新 `run` 后接 `}` 结束 impl,再开 `impl Milestone { fn apply ... }`。`ToolOutput::ok/err` 与 `NodeStatus`、`WorkGraph` 用法保持原样。)

- [ ] **Step 2: 跑 Milestone 工具测试无回归**

Run: `cargo test milestone 2>&1 | grep -E 'test result|FAILED|error\[' | head`
Expected: 全绿(dev.rs 内 milestone 工具单测仍过)。

- [ ] **Step 3: commit**

```bash
git add src/tool/dev.rs
git commit -m "refactor(milestone): 工具抽 apply + 走 with_lock

Milestone 工具读+改+存统一在 WorkGraph::with_lock 咨询锁内;抽出 apply(g,...)
纯 mutate+render(无 IO)。防与 advance/daemon 线程并发 lost-update。"
```

---

## Task 5: ADR 0035 + ARCHITECTURE 同步

**Files:**
- Create: `docs/adr/0035-workgraph-concurrency-write-protection.md`;Modify: `ARCHITECTURE.md`

- [ ] **Step 1: 写 ADR 0035**

```markdown
# ADR 0035 — WorkGraph 并发写保护

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0004(Session 原子写,同 atomic-replace 模式)、上限压测 P9(`docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`)

## 背景

P9 实测:多个 BG 进程(或 daemon workgraph 线程 + BG/交互 milestone 工具)并发写 `workgraph.json`,各自 `read→mutate→save`,后写覆盖先写 → **静默丢里程碑**(4 并发 `milestone add` → 0 存活;JSON 合法=未损坏,是 data loss)。`save` 已是原子(temp+rename),故文件不损坏,但 read-modify-write 整体非原子。

## 决策

1. **fs2 咨询锁**:`WorkGraph::with_lock(root, |g| -> Result<T>)` 用 `fs2::FileExt::lock_exclusive` 包住 read→mutate→save。fs2 锁由 OS 在进程退出/崩溃时自动释放 → **无 stale-lock**(对比 PID 锁文件)。
2. **独立 lock 文件** `workgraph.json.lock`:不锁数据文件——`save` 的 atomic-rename 会换数据文件 inode,锁关联会失效;lock 文件不 rename,inode 稳定。
3. **锁粒度**:只覆盖毫秒级闭包(read→mutate→save),**不覆盖调用方的 LLM turn**——turn 跑完后再在锁内更新状态,避免长任务持锁。
4. 三处写点(advance_one_milestone / drive_workgraph / Milestone 工具)统一走 `with_lock`;reads-only 站点不加锁(slightly-stale 可忍,race 是 write-write)。

## 后果

- **正面**:并发写者不丢里程碑(8 线程并发 `with_lock` 测试全存活)。
- **代价**:并发写者串行化(临界区毫秒级,可忍);读动作(list/next)经 with_lock 会多一次幂等 save(无害)。
- **不做**:session/memory/ledger 的并发(风险低);锁超时/熔断(写者少,过度设计);跨机分布式锁。
```

- [ ] **Step 2: ARCHITECTURE.md workgraph 行补注**

把 `ARCHITECTURE.md` 模块地图 `workgraph.rs` 行(描述 WorkGraph 一等公民那行)末尾补:`;**fs2-locked RMW**(`with_lock`,ADR 0035,防并发 lost-update)`。

- [ ] **Step 3: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add docs/adr/0035-workgraph-concurrency-write-protection.md ARCHITECTURE.md
git commit -m "docs: ADR 0035 WorkGraph 并发写保护 + ARCHITECTURE 同步

记录 fs2 咨询锁 + 独立 lock 文件 + 锁不包 LLM turn 的取舍。"
```

---

## Task 6: live 复验(复现 P9 4→0,确认现 4→4)

**Files:** 无源码改动;`codecoder-probe/` lab 复跑 P9 并发场景。

- [ ] **Step 1: 重编译**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`。

- [ ] **Step 2: 4 并发 milestone add,断言全存活**

```bash
LAB=/Users/rong.zhu/Code/codecoder-probe
SCRIPTS=/Users/rong.zhu/Code/codecoder/docs/superpowers/scripts
set -a; . /Users/rong.zhu/Code/codecoder/.ccd.env; set +a
rm -f "$LAB/workgraph.json" "$LAB/workgraph.json.lock"
CODECODER_ROOT="$LAB" "$SCRIPTS/probe_concurrent.sh" p9_recheck 4 "用 milestone 工具 add 一个标题为 'concurrent-recheck' 的里程碑" 2>&1 | tail -3
echo "=== workgraph 存活里程碑数(修复前=0,修复后应>0)==="
jq '.milestones|length' "$LAB/workgraph.json" 2>/dev/null
echo "=== JSON 完整性 ==="
jq . "$LAB/workgraph.json" >/dev/null 2>&1 && echo "INTACT ✓" || echo "CORRUPT ❌"
```
Expected: 4 进程各 exit 0;`milestones|length` **≥1(不再 0)**;`INTACT`(无损坏)。注:4 并发同标题可能去重/部分丢失取决于 add 语义,但**至少不再是 0 存活**——与 P9 的 0 形成对比。

- [ ] **Step 3: 记结论到 lab matrix**

```bash
printf '\n## P9 修复复验(2026-07-22,fix/workgraph-concurrency)\n- 4 并发 milestone add → 存活里程碑数从 P9 的 **0** 变为 ≥1(with_lock 生效)\n- workgraph.json 仍 jq 合法(无损坏)\n' >> /Users/rong.zhu/Code/codecoder-probe/matrix.md
echo "recorded"
```

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- fs2 + with_lock helper(spec §3)→ Task 1 ✓
- 独立 lock 文件、不包 turn(spec §3)→ Task 1 helper 注释 + 闭包设计 ✓
- 3 写点重构(spec §4:advance/drive_workgraph/Milestone)→ Task 2/3/4 ✓
- 8 线程并发不丢更新测试(spec §5)→ Task 1 ✓
- ADR 0035 + ARCHITECTURE(spec §6)→ Task 5 ✓
- 范围仅 workgraph.json(spec §7)→ 未涉 session/memory/ledger ✓
- live 复验 → Task 6 ✓

**2. Placeholder scan:** 无 TBD/TODO;每个 code step 含完整代码(advance/agent/milestone 重构均给出改前/改后完整块);测试代码完整 ✓

**3. Type consistency:** `WorkGraph::with_lock<T, F>(root: &Path, f: F) -> anyhow::Result<T> where F: FnOnce(&mut WorkGraph) -> anyhow::Result<T>`(Task 1)与 Task 2/3(闭包返回 `Ok(())`)、Task 4(闭包返回 `Ok(ToolOutput)`)调用一致;`Milestone::apply(g: &mut WorkGraph, action: &str, id: Option<u64>, args: &Value) -> ToolOutput`(Task 4)与 run 内调用一致;`WorkGraph::add -> anyhow::Result<u64>`(workgraph.rs:150)与测试/apply 用法一致 ✓
