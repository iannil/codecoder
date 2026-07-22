# WorkGraph 并发写保护(lost-update 修复)— 设计文档

- **日期**: 2026-07-22
- **状态**: 待用户审阅(Pending user review)
- **作者**: Claude Code(brainstorming 产物)
- **起因**: `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` P9 发现——workgraph.json 是非原子 read-modify-write,并发写**静默丢里程碑**(4 并发 `milestone add` → 0 存活;JSON 合法=未损坏,是 data loss)。
- **关联**: ADR 0004(Session 原子写,同模式)、`src/workgraph.rs`、`src/background.rs`、`src/agent.rs`、`src/tool/dev.rs`。

## 1. 背景与目标

上限压测(P9)坐实:多个 BG 进程(或 daemon workgraph 线程 + BG/交互 milestone 工具)并发写 `workgraph.json` 时,各自 `WorkGraph::read → mutate → save`,**后写覆盖先写 → 丢里程碑**。`save` 本身已是原子(temp+rename,同 Session),所以**文件不损坏**(jq 解析合法),但**数据丢失**。

**目标**: 让 workgraph 的 read→mutate→save 在并发写者下不丢更新;**不损坏既有行为、不加 LLM turn 持锁开销、不引入 stale-lock**。**只覆盖 workgraph.json**(session/memory/ledger 风险更低,见范围)。

## 2. 已锁定决策

| 维度 | 决策 |
|---|---|
| 机制 | **fs2 咨询锁**(`FileExt::lock_exclusive`)+ `WorkGraph::with_lock` helper |
| lock 文件 | **独立 `workgraph.json.lock`**(锁数据文件会被 save 的 atomic-rename 换 inode 打破) |
| 锁粒度 | **只包最终 read→mutate→save(毫秒级),不包 LLM turn** |
| 范围 | **仅 workgraph.json**;session/memory/ledger 不纳入 |
| ADR | **新建 ADR 0035《WorkGraph 并发写保护》** |

**为何 fs2**: 项目已重依赖(wasmtime/tree-sitter×5/tiktoken/signal-hook/ureq),`fs2` 轻量通用、不违和。fs2 咨询锁由 OS 管理,**进程崩/死自动释放→无 stale-lock**(对比 PID 锁文件的边界条件)。`retry.rs` 的"dependency-free"是该模块自实现,非项目级禁依赖规则。

**为何独立 lock 文件**: `save` 用 `tmp + rename`(atomic-replace),rename 后数据文件是**新 inode**;若直接锁数据文件,save 后锁关联失效。故锁一个**不 rename 的独立 `workgraph.json.lock`**,其 inode 稳定。

## 3. 架构

```
WorkGraph::with_lock(root, |g: &mut WorkGraph| -> anyhow::Result<T>) -> anyhow::Result<T>
  1. OpenOptions::new().create(true).read(true).write(true).open("<root>/workgraph.json.lock")
  2. file.lock_exclusive()            // fs2,阻塞;OS 自动释放在进程退出/崩溃
  3. let mut g = WorkGraph::read(root);   // 锁内读最新
  4. let r = f(&mut g);                    // 跑闭包(mutate)
  5. g.save(root)?;                       // 原子 temp+rename,锁内
  6. drop(file) → lock 释放               // 返回 r
```

**关键不变量**: 闭包内**只做内存态 mutate + 返回值计算**;**不在闭包内跑 LLM turn / IO 长任务**(否则持锁过久)。调用方先跑完 turn,再在 `with_lock` 闭包内只更新图状态。

## 4. 写点重构(3 处)

所有 read→mutate→save 改用 `with_lock`;reads-only 站点(next_ready 检查、metadata 读)**不改**(slightly-stale 可忍,race 是 write-write)。

### 4.1 `advance_one_milestone`(background.rs:305-310)

现状:turn 跑完后 `let mut g = WorkGraph::read(&root); g.set_status(...); g.nodes...verdict=...; g.save(&root);`。
改为(turn 不动,只包最终状态更新):
```rust
WorkGraph::with_lock(&root, |g| {
    g.set_status(milestone_id, status);
    if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
        n.verdict = Some(vs_str.into());
    }
    Ok(())
})?;
```

### 4.2 `drive_workgraph`(agent.rs:1358-1368)

同样把最终 `set_status` + `save` 包进 `with_lock` 闭包。

### 4.3 `Milestone` 工具(tool/dev.rs:172-244)

整个 action 分发(`add`/`start`/`done`/`needs_fix`/`remove`/`list`/`next`)包进 `with_lock` 闭包;`add` 经闭包返回值传出新 id。`list`/`next`(只读)也可走 `with_lock` 取一致快照,或保持无锁读(YAGNI:保持无锁,只锁写动作)。

## 5. 测试(TDD)

- **`with_lock_prevents_lost_update_under_concurrency`(核心,直接复现 P9)**:
  - `WorkGraph::default().save(dir)` 起空图;
  - 8 线程并发各 `WorkGraph::with_lock(&dir, |g| { g.add(&format!("t{i}"), "", vec![])?; Ok(()) })`;
  - 断言 `WorkGraph::read(&dir).nodes.len() == 8`(**无丢失**)。
  - 无锁的等价并发会丢(P9 的 4→0);有锁全存活——直接 before/after 证明。
- **`with_lock_releases_on_drop`**: 连续两次 `with_lock` 不死锁(drop 即释放)。
- **回归**: 既有 workgraph 测试(`save_load_roundtrip_and_next_id`、`validate_detects_cycle`、set_status 测试)+ advance/milestone 单测全过。

## 6. ADR / 文档

- **新建 ADR 0035《WorkGraph 并发写保护》**: 记录 fs2 咨询锁、独立 lock 文件(避 atomic-rename 换 inode)、锁不包 LLM turn 的粒度取舍、OS 自动释放无 stale-lock。
- **ARCHITECTURE.md**: `workgraph.rs` 模块行补"fs2-locked RMW(`with_lock`)"。
- README/CLAUDE 无 env 变化,不改。

## 7. 不在本范围内(YAGNI)

- **session.rs / memory/ / bg_ledger.jsonl 的并发**: session 每 session 文件单 client 单写者(低风险);ledger append-only;memory 分 key——均不纳入,实测到争用再单开。
- **reads-only 站点加共享锁**: 不加(slightly-stale 读可忍)。
- **锁超时/熔断**: 用阻塞 `lock_exclusive`(写者少、临界区毫秒级);不引入超时/livelock 处理(过度设计)。
- **跨 root / 跨机并发**: 单 root 内文件锁即可,不做分布式。
