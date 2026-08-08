# ADR 0041 — Task/Cron/SendMessage 工具：任务管理、定时调度与子代理通信

- **状态**: Accepted
- **日期**: 2026-08-08
- **关联**: ADR 0026 (Background Agent)、ADR 0033 (BG Ledger)、ADR 0035 (Workgraph Concurrency)、`CONTEXT.md`(领域术语)

## 背景

CodeCoder 的内置工具集（原 31 个）需要扩展三种新能力：

1. **任务管理**：用户需要跟踪多步并行/串行工作的进度，与已有的持久化 `workgraph.json` 里程碑图互补。任务管理应当是轻量级的、会话内的工作跟踪，不要求跨 daemon 重启存活。
2. **定时调度（Cron）**：用户需要设置定时或循环提醒，在指定时间向 agent 注入 prompt。这需要 daemon 常驻的后台线程，复用现有的 `cmd_tx` 注入通道。
3. **子代理通信（SendMessage）**：子代理在独立线程中执行，无法直接与用户或其他子代理通信。需要双向的消息通道，使子代理能向父代理或其他子代理发送消息。

## 决策

### 任务管理：内存 Mutex<Vec<TaskEntry>>

- 任务数据存储在 `LazyLock<Mutex<Vec<TaskEntry>>>` 全局单例中，每个 `TaskEntry` 包含 id、subject、description、status、activeForm、owner、blocks/blockedBy 依赖关系、metadata 等字段。
- 任务状态机：`pending` → `in_progress` → `completed`，或 `deleted`（永久移除）。
- 工具接口：`task_create`（创建并返回 id）、`task_get`（查询详情）、`task_list`（支持 status 过滤）、`task_update`（更新字段、状态、依赖关系）、`task_stop`（标记为 deleted）。
- **非持久化**：任务在 daemon 重启后自动消失，与持久化 `workgraph.json` 里程碑图互补——里程碑用于持久化、有依赖关系的工作图，任务用于轻量级会话内跟踪。

### 定时调度：内存 CronStore + daemon 后台线程

- `CronStore` 是内存中的 cron 任务注册表，每个条目包含 cron 表达式、注入 prompt、周期性/一次性标志、持久化标志。
- Daemon 启动时生成一个 `poll_cron_due()` 后台线程，每秒轮询 `CronStore` 中到期的任务，通过 `cmd_tx.send(AgentCommand::ProcessMessage { content })` 注入到活跃 session。
- 一次性任务触发后自动删除，周期性任务自动重新注册到期时间。
- 复用现有的 autotask 线程模式（ADR 0026 中确立的后台线程注入机制），避免引入外部 cron 调度器依赖。

### 子代理通信：全局 AgentRegistry + mpsc 通道

- `AgentRegistry` 是 `LazyLock<Mutex<HashMap<String, mpsc::Sender<String>>>>` 全局单例，管理所有活跃子代理的命名通道。
- 子代理 spawn 时自动注册到 `AgentRegistry`，生成唯一的 agent ID 并创建 `mpsc::channel`。
- 子代理的 turn loop 中定期轮询 `mpsc::Receiver`，接收来自父代理或其他子代理的消息。
- 子代理完成或取消时自动从 `AgentRegistry` 注销。
- `send_message` 工具：`to` 参数为目标 agent 名称或 ID，通过 `AgentRegistry` 查找通道发送消息。

## 理由

- **任务管理不持久化**：Task 是轻量级会话内跟踪，适合短期工作流编排。持久化工作图（`workgraph.json`）已经承担了跨重启的里程碑管理，任务不应重复此功能。
- **Cron 复用现有线程模式**：daemon 已经运行了 autotask 后台线程，`poll_cron_due()` 遵循相同的模式——后台线程通过 `cmd_tx` 注入 `AgentCommand`。这避免了引入外部 cron 守护进程依赖，简化了部署。
- **SendMessage 需要改造子代理生命周期**：子代理在自己的线程中运行，之前只有单向工具调用结果返回。`AgentRegistry` 全局注册表是管理动态子代理通道最直接的方式，mpsc 通道是 Rust 标准库的原生并发原语，无需额外依赖。

## 后果

### 正面
- 任务管理工具集填补了会话内工作跟踪的空白，与持久化里程碑互补。
- Cron 调度无需外部依赖，完全在 daemon 进程内完成。
- 子代理通信通道标准化，为未来扩展（如广播、分组）奠定基础。

### 负面
- 任务条目在 daemon 重启后消失，不适合需要跨 session 持久化的场景（此类场景应使用 `milestone` 工具和 `workgraph.json`）。
- Cron 调度依赖于 daemon 常驻——daemon 停止时所有 cron 任务丢失。
- SendMessage 的父→子方向已完整实现；子→父方向（`to: "main"`）暂未实现，需后续迭代。
- Cron 注入线程向第一个活跃 session 注入 prompt，不广播到所有 session。

### 限制与缓解
- 任务条目的非持久化已在工具文档中标注，用户应使用 `milestone` 工具处理需要持久化的进度。
- 周期性 cron 任务有 7 天自动过期机制，避免 session 无限延长。
- Cron 注入的 `ProcessMessage` 走的是 `cmd_tx` 通道，与用户输入同优先级，确保及时响应。

## 替代方案

### 外部 cron 守护进程
- **未采用**原因：引入额外依赖（如 `cronie` 或系统 `cron`），增加部署复杂度。CodeCoder 的 daemon 已经是常驻进程，内部轮询是最简单的方案。

### 文件持久化任务
- **未采用**原因：与 `workgraph.json` 里程碑图功能重叠。任务管理定位为轻量级会话内跟踪，文件持久化会增加复杂度，且需要处理并发写入问题（ADR 0035 已护 workgraph 并发写，但任务管理无需此保护）。

### event_tx 子→父消息
- **未采用**原因：`event_tx` 是 agent→daemon 的事件通道，用于流式增量和结构化状态回传，不是为消息传递设计的。使用独立的 `AgentRegistry` + mpsc 通道架构更清晰，避免与现有事件系统耦合。

## 实现

### 新增文件

```
src/tool/task.rs        — TaskEntry 结构体 + TaskCreate/TaskGet/TaskList/TaskUpdate/TaskStop 工具
src/tool/cron.rs        — CronStore + CronCreate/CronDelete/CronList 工具
src/tool/sendmessage.rs — AgentRegistry + SendMessage 工具
src/cron_scheduler.rs   — poll_cron_due() 后台线程
```

### 修改文件

```
src/tool/mod.rs         — 注册 9 个新工具，工具计数 31→40
src/daemon/daemon.rs    — 在 daemon 启动时 spawn cron 轮询线程
src/agent/subagent.rs   — 子代理 spawn 时注册到 AgentRegistry，turn loop 中轮询消息，完成时注销
```

### 工具计数

内置工具：31 → 40（新增 task_create/task_get/task_list/task_update/task_stop/cron_create/cron_delete/cron_list/send_message）