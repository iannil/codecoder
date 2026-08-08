# Task Management + Cron + SendMessage 工具设计

> 在已完成的 MCP/LSP 工具基础上，为 CodeCoder 补全 P1 工作流工具。

## 1. Task 管理工具族

### 工具定义

5 个工具：`task_create` / `task_get` / `task_list` / `task_update` / `task_stop`

### 数据结构

```rust
struct TaskEntry {
    id: u64,                    // 自增，全局唯一
    subject: String,
    description: String,
    status: TaskStatus,          // Pending | InProgress | Completed | Deleted
    created_at: SystemTime,
    owner: Option<String>,       // 分配给谁
    blocked_by: Vec<u64>,        // 依赖的任务 id
    blocks: Vec<u64>,            // 此任务阻塞的任务 id
    active_form: Option<String>, // 进行中时显示的进行时形式
}
```

### 存储

`Mutex<Vec<TaskEntry>>` 全局单例（内存中，非持久化）。与 `milestone` 工具的持久化 `workgraph.json` 互补——task 是轻量级会话内跟踪，不跨 daemon 重启。

### 各工具规格

**`task_create`**
- 参数：`subject`(必填)、`description`、`status`、`active_form`、`blocked_by`、`owner`
- 返回：`{ "id": 1 }`
- 权限：`Permission::None`

**`task_get`**
- 参数：`task_id`(必填)
- 返回：单条 task 的全部字段（含 `blocks`、`blocked_by`）
- 权限：`Permission::None`

**`task_list`**
- 参数：可选 `status` 过滤（`pending` / `in_progress` / `completed` / `deleted`）
- 返回：匹配的 task 数组，每个含 id、subject、status、owner、blocked_by
- 权限：`Permission::None`

**`task_update`**
- 参数：`task_id`(必填)、`status`、`subject`、`description`、`owner`、`active_form`、`add_blocks`、`add_blocked_by`
- 行为：更新指定字段。`add_blocks`/`add_blocked_by` 是追加（不覆盖）
- 权限：`Permission::None`

**`task_stop`**
- 参数：`task_id`(必填)
- 行为：把状态设为 `deleted`（软删除，不实际移除）
- 权限：`Permission::None`

### 关联已有的 `Milestone` 机制

milestone 是持久化依赖图（存文件），task 是内存轻量跟踪。两者不冲突：
- `milestone` = 项目级里程碑，跨 session 存续
- `task` = 单次会话内的工作单元，daemon 重启后消失

---

## 2. Cron 调度工具族

### 工具定义

3 个工具：`cron_create` / `cron_delete` / `cron_list`

### 数据结构

```rust
struct CronEntry {
    id: u64,
    cron: String,           // 标准 5 字段 cron 表达式 "*/5 * * * *"
    prompt: String,         // 到点注入 agent 的 prompt
    description: String,
    created_at: SystemTime,
    last_fired: Option<SystemTime>,
    is_one_shot: bool,      // 是否只触发一次
}
```

### 存储与触发

- 存储：`Mutex<Vec<CronEntry>>` 全局单例，内存中
- 触发：daemon 启动一个后台线程，每秒扫描 cron 条目
  - 解析 cron 表达式，判断当前时间是否匹配
  - 匹配时，通过 `cmd_tx` 发送 `AgentCommand::ProcessMessage` 把 prompt 注入 agent
  - 标记 `last_fired = now`，防止同一秒重复触发
  - `is_one_shot` 的条目触发后自动删除

### 各工具规格

**`cron_create`**
- 参数：`cron`(必填)、`prompt`(必填)、`description`、`is_one_shot`
- 权限：`Permission::Ask { key: "cron" }`

**`cron_delete`**
- 参数：`id`(必填)
- 权限：`Permission::Ask { key: "cron" }`

**`cron_list`**
- 参数：无
- 返回：所有 cron 条目列表
- 权限：`Permission::None`

### 与 autotask 的关系

autotask（`CODECODER_AUTOTASK_SOURCE` 轮询 GitHub Issues 等外部源）和 cron（用户定义的定时任务）是两个独立机制，复用相同的 `cmd_tx` 注入通道。

---

## 3. SendMessage 工具

### 工具定义

1 个工具：`send_message`

### 通信模型

**双向支持：**

- **子→父 (Child→Parent)：** 子代理通过 `send_message { to: "main", message: "..." }` 发送消息，通过子代理的 `reply_tx` 通道回传给父 agent
- **父→子 (Parent→Child)：** 父 agent 通过 `send_message { to: "<agent_id>", message: "..." }` 给已存活的子代理发消息，通过子代理的消息接收通道 `receive_rx` 送达

### 核心变更

需要在 `agent.rs` 中改造子代理生命周期：

1. 子代理结构体新增 `receive_rx: Receiver<String>` 通道
2. `agent` 工具 spawn 子代理时，创建 `(sender, receiver)` 对
3. 子代理的消息循环中，额外监听 `receive_rx`（与完成任务的信号 select）
4. `send_message` 工具查找 agent 注册表，找到目标 `sender` 发送消息
5. 子代理终止时自动清理注册表

### 数据结构

```rust
// 全局 agent 注册表
struct AgentRegistry {
    agents: HashMap<String /* agent_id or name */, Sender<String>>,
}
```

### 各工具规格

**`send_message`**
- 参数：`to`(必填，agent_id 或固定名)、`message`(必填)、`summary`(可选)
- 返回：成功/失败
- 权限：`Permission::None`

### 注意事项

- 子代理收到的消息格式：`{ "from": "parent", "type": "instruction", "content": "..." }`
- 子代理应把消息当作新的 user message 处理
- 如果目标 agent 已被销毁，返回错误

---

## 实现计划

### 分任务依赖

```
Task 1: Task 管理工具族 ─── 独立，不依赖其他
Task 2: Cron 调度工具族 ─── 依赖 daemon 的 cmd_tx 注入机制
Task 3: SendMessage ─────── 依赖 agent 子代理生命周期改造
```

### 文件变更

| 文件 | 变更 |
|------|------|
| `src/tool/task_manage.rs` | 新建，5 个工具 |
| `src/tool/cron.rs` | 新建，3 个工具 |
| `src/tool/send_message.rs` | 新建，1 个工具 |
| `src/tool/mod.rs` | 注册所有新工具 |
| `src/agent.rs` | 子代理生命周期改造（SendMessage 支持） |
| `src/daemon/mod.rs` | 启动 cron 后台线程 |
| `README.md` | 工具表更新（31→40） |