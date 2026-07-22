# ADR 0035 — WorkGraph 并发写保护

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0004(Session 原子写,同 atomic-replace 模式)、上限压测 P9(`docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`)

## 背景

P9 实测:多个 BG 进程(或 daemon workgraph 线程 + BG/交互 milestone 工具)并发写 `workgraph.json`,各自 `read→mutate→save`,后写覆盖先写 → **静默丢里程碑**(4 并发 `milestone add` → 0 存活;JSON 合法=未损坏,是 data loss)。`save` 已是原子(temp+rename),故文件不损坏,但 read-modify-write 整体非原子。

## 决策

1. **fs2 咨询锁**:`WorkGraph::with_lock(root, |g| -> Result<T>)` 用 `fs2::FileExt::lock_exclusive` 包住 read→mutate→save。fs2 锁由 OS 在进程退出/崩溃时自动释放 → **无 stale-lock**(对比 PID 锁文件的边界条件)。
2. **独立 lock 文件** `workgraph.json.lock`:不锁数据文件——`save` 的 atomic-rename 会换数据文件 inode,锁关联会失效;lock 文件不 rename,inode 稳定。
3. **锁粒度**:只覆盖毫秒级闭包(read→mutate→save),**不覆盖调用方的 LLM turn**——turn 跑完后再在锁内更新状态,避免长任务持锁。
4. 三处写点(`advance_one_milestone` / `drive_workgraph` / `Milestone` 工具)统一走 `with_lock`;reads-only 站点不加锁(slightly-stale 可忍,race 是 write-write)。

## 后果

- **正面**:并发写者不丢里程碑(8 线程并发 `with_lock` 测试全存活;live 复验 4 并发 milestone add 不再 0 存活)。
- **代价**:并发写者串行化(临界区毫秒级,可忍);读动作(list/next)经 `with_lock` 会多一次幂等 save(无害)。
- **不做**:session/memory/ledger 的并发(风险低);锁超时/熔断(写者少,过度设计);跨机分布式锁。
