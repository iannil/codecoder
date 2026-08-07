# CodeCoder Task/Agent 重设计

> 废弃 skill-engine 的 step 状态机后，回归"写好 prompt，信任模型"的简单模式。
> 子 agent 参照 Claude Code 的 Task/Agent 模式重新设计。

## 问题回顾

### 问题 1：`type: agent` 子 agent 是空白人

skill-engine 的 `type: agent` 步骤创建的子 agent 通过 `new_sub_full()` 构造，但：
- 不加载系统提示词（AGENTS.md、CONTEXT.md、技能目录）
- 仅注入 `recent_context(8)` 文本摘要，且跳过 ToolResult 正文
- 子 agent 不知道项目结构、技术栈、里程碑状态

结果：子 agent 陷入 stub 死循环——不知道代码在哪，于是自己创建桩代码，下一个 agent 看到桩代码又重建。

### 问题 2：过度用代码控制 LLM 行为

skill-engine 把 skill 解析成 steps 数组（prompt/check/gate/composite/parallel/agent），由 Rust 代码控制执行流程。这是一个代码级的状态机来控制 LLM，违背了"写好 prompt，信任模型"的原则。

对比：Pi 和 Claude Code 的 skill 都是纯文本注入 system prompt，模型自己决定怎么做。

### 问题 3：大文件写入

已存在 `append: true` 支持，但模型不知道可以跨回合使用。

## 架构决策

### ADR 1：废弃 skill-engine，回归纯文本技能

`src/skill_engine/` 整个模块已删除。技能（`.md` 文件）只作为纯文本注入 system prompt。`use_skill` 工具只做文件读取和文本注入，不做步骤执行。

### ADR 2：子 agent 两种模式——Fork 和 Fresh

参照 Claude Code 的 AgentTool 设计（`forkSubagent.ts` / `runAgent.ts`）：

| 模式 | 触发条件 | 上下文继承 | 工具箱 |
|------|---------|-----------|--------|
| **Fork** | 省略 `subagent_type` | 继承父 system prompt + 最近 20 条消息副本 | 完整（builtin） |
| **Fresh** | 指定 `subagent_type` | 仅继承父 system prompt | 完整（builtin） |

### ADR 3：review 工具保持只读

`review` 是独立评审，不能有写权限。保持 `read_only_child()` 工具箱。

### ADR 4：异步后台 + SendMessage 通信

支持 `background: true` 模式，子 agent 后台运行，通过 `TaskNotification` 事件通知父 agent。新增 `SendMessage` 工具用于 agent 间通信。

## 详细设计

### 1. Agent 工具 Schema

```rust
// src/tool/builtin.rs
pub struct Agent;

impl Tool for Agent {
    fn name(&self) -> &str { "agent" }
    fn description(&self) -> &str {
        "Delegate a task to a sub-agent. Omit subagent_type to fork (inherit full context). Specify subagent_type to start fresh."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "The delegated task." },
                "subagent_type": {
                    "type": "string",
                    "description": "Optional agent type. Omit = fork (inherits your full conversation context). Specify = fresh (zero context, provide full background in task)."
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Default: false. Background tasks notify on completion."
                }
            },
            "required": ["task"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None // 子 agent 自己走权限门控
    }
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::err("agent tool must be handled by the AgentLoop"))
    }
}
```

### 2. Fork 模式——子 agent 继承父上下文

```rust
// src/agent.rs — 新增
const FORK_CONTEXT_SIZE: usize = 20;

/// Fork 模式：子 agent 继承父上下文。
/// 把父 session 的最近 N 条消息复制到子 agent 的 session 中。
fn spawn_sub_agent_fork(&mut self, task: String, background: bool, 
    event_tx: &Sender<AgentEvent>) -> String
{
    // 1. 从父 session 复制最近 N 条消息
    let thread = self.session.active_thread();
    let start = thread.len().saturating_sub(FORK_CONTEXT_SIZE);
    let parent_messages: Vec<Message> = thread[start..]
        .iter()
        .filter(|m| m.role != Role::System)  // 跳过 System（子 agent 有自己的 system_prompt）
        .cloned()
        .collect();

    // 2. 创建 fork 子 agent
    let (child_tx, child_rx) = channel();
    let provider = Arc::clone(&self.provider);
    let model = self.model.clone();
    let mt = self.max_tokens;
    let temp = self.temperature;
    let root = self.root.clone();
    let system_prompt = self.system_prompt.clone();
    let registry = self.shared_registry.clone();
    let home = self.home.clone();
    let trust = self.trust;
    let headless = self.headless;

    let task_id = if background { Some(generate_task_id()) } else { None };

    let handle = thread::spawn(move || {
        let mut child = AgentLoop::fork_sub(
            provider, model, mt, temp, root,
            parent_messages,
            system_prompt,
            registry,
            home, trust, headless,
        );
        child.process_turn(task, &child_tx);
        child.last_assistant_text()
    });

    // 3. 后台模式：立即返回 task_id
    if let Some(tid) = task_id {
        let _ = event_tx.send(AgentEvent::TaskSpawned {
            task_id: tid.clone(),
            description: task.chars().take(80).collect(),
        });
        // 后台线程完成后通知
        thread::spawn(move || {
            let output = handle.join().unwrap_or_default();
            let _ = child_tx.send(AgentEvent::TaskNotification {
                task_id: tid,
                status: "completed".into(),
                result: output,
            });
        });
        return format!("task {tid} started in background");
    }

    // 4. 同步模式：等待完成
    for ev in child_rx {
        if let AgentEvent::ToolStarted { name, .. } = ev {
            let _ = event_tx.send(AgentEvent::SubAgentMilestone(name));
        }
    }
    handle.join().unwrap_or_default()
}
```

### 3. Fork 子 agent 构造方法

```rust
/// 创建 fork 子 agent：继承父 system prompt + 最近消息 + 完整工具箱。
fn fork_sub(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    parent_messages: Vec<Message>,
    system_prompt: String,
    shared_registry: Option<Arc<RwLock<Registry>>>,
    home: PathBuf,
    trust: TrustState,
    headless: bool,
) -> Self {
    let mut agent = Self::build(
        provider, model, max_tokens, temperature, root,
        Toolbox::builtin(),  // 完整工具箱！
        /* persist */ false,
        headless,
        shared_registry,
    );
    agent.system_prompt = system_prompt;
    agent.home = home;
    agent.trust = trust;
    // 把父的最近消息写入 session
    for msg in parent_messages {
        agent.session.append(msg);
    }
    agent
}
```

### 4. Fresh 模式

```rust
/// Fresh 模式：零上下文，仅继承父 system prompt + 完整工具箱。
fn spawn_sub_agent_fresh(&mut self, task: String, background: bool,
    event_tx: &Sender<AgentEvent>) -> String
{
    // 与 fork 类似，但不复制 parent_messages
    // ...
}
```

### 5. Review 保持 read_only_child

```rust
// 保持不变——review 工具仍用只读子 agent
pub fn run_review(&mut self, target: &str, ...) -> (ReviewOutcome, String) {
    let raw = self.spawn_sub_agent_text(...);  // 保持 read_only_child
    ...
}
```

### 6. AgentEvent 新增变体

```rust
pub enum AgentEvent {
    // ... 现有所有变体

    /// 后台任务已启动
    TaskSpawned {
        task_id: String,
        description: String,
    },
    /// 后台任务完成通知
    TaskNotification {
        task_id: String,
        status: String,  // "completed" | "failed" | "killed"
        result: String,
    },
}
```

### 7. dispatch_tool 改造

```rust
fn dispatch_tool(&mut self, call_id: &str, name: &str, 
    args: Value, event_tx: &Sender<AgentEvent>) -> ToolOutcome
{
    if name == "agent" {
        return self.spawn_sub_agent(call_id, &args, event_tx);
    }
    // ... 其他拦截
}

fn spawn_sub_agent(&mut self, call_id: &str, 
    args: &Value, event_tx: &Sender<AgentEvent>) -> ToolOutcome
{
    let task = args.get("task").and_then(|v| v.as_str()).unwrap_or_default();
    let subagent_type = args.get("subagent_type").and_then(|v| v.as_str());
    let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);

    if task.is_empty() {
        return ToolOutcome::err("missing required arg: task");
    }

    let output = if subagent_type.is_some() {
        // Fresh 模式：指定了 subagent_type → 零上下文
        self.spawn_sub_agent_fresh(task.to_string(), background, event_tx)
    } else {
        // Fork 模式：省略 subagent_type → 继承上下文
        self.spawn_sub_agent_fork(task.to_string(), background, event_tx)
    };

    ToolOutcome::ok(output)
}
```

### 8. 后台实现约束

当前 CodeCoder 的事件循环是同步的（`process_turn` 内串行工具循环）。后台通知需要跨 turn 传递：

- 后台子 agent 完成后通过 `AgentEvent::TaskNotification` 发送
- TUI 在下一轮 `process_turn` 前收到通知
- 通知注入为 System 消息，让父 agent 在下一轮 LLM 请求中看到

**约束**：后台通知仅在父 agent 进入下一轮 `process_turn` 时可见。如果父 agent 已结束，通知丢失。这是当前架构的限制，后续可改为持久化通知队列。

### 9. 大文件写入

已存在 `append: true` 支持。需在 system prompt 中添加指导：

> 对于大文件，可以使用 `write_file` 的 `append: true` 参数分多次写入：先用 `write_file` 写入第一部分，然后多次调用 `write_file` 并设置 `append: true` 追加后续内容。

## 实现计划

### 阶段 1：Agent 工具 Schema 扩展 + Fork 模式

1. 修改 `src/tool/builtin.rs` 的 Agent 工具：扩展 schema，增加 `subagent_type` 和 `background` 字段
2. 在 `src/agent.rs` 实现 `spawn_sub_agent_fork` 和 `fork_sub` 构造方法
3. 修改 `spawn_sub_agent` 分派逻辑：根据 `subagent_type` 是否指定走 fork 或 fresh
4. 修改 `Toolbox::read_only_child()` 不再用于 agent 工具（review 保留）

### 阶段 2：后台异步 + TaskNotification

1. 在 `AgentEvent` 中新增 `TaskSpawned` 和 `TaskNotification` 变体
2. 实现后台线程管理和通知路由
3. 修改 `dispatch_tool` 处理 `background: true`

### 阶段 3：大文件写入指导

1. 在 system prompt 中添加 `append` 使用指导
2. 确保 `write_file` 的 append 模式稳定可用

### 阶段 4：测试

1. Fork 模式：子 agent 继承上下文后能正常工作
2. Fresh 模式：零上下文子 agent
3. Review 仍保持只读
4. 后台模式的基础功能