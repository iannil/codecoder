# 全能力自验证系统设计

> 2026-07-19

## 背景

CodeCoder 已有三面行为验证（L1 主干/L2 pty 冒烟/L3 真实 LLM），但缺一个能覆盖**所有功能、能力、工具**的自动化验证系统。同时，这个验证系统将是后续 24h 持续自进化循环的基础前提。

## 总体架构

```
┌──────────────────────────────────────────────────────┐
│                    TUI 仪表盘                          │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐  │
│  │ L1 主干   │ │ L2 冒烟  │ │ L3 真实  │ │ L4 能力│  │
│  │ (hermetic)│ │ (pty)    │ │ (LLM)    │ │ 验证   │  │
│  └──────────┘ └──────────┘ └──────────┘ └────────┘  │
└──────────────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  L4 验证引擎                                           │
│  ┌────────────────────────┐ ┌──────────────────────┐  │
│  │ 阶段 1: 骨架场景        │ │ 阶段 2: 自驱动探索    │  │
│  │ 脚本化 + 精确断言       │ │ Agent 驱动的探索     │  │
│  │ 25 工具 / 权限 / 对话   │ │ Skill/Capability     │  │
│  │ 失败 → 停止             │ │ 健康检查 + 自愈      │  │
│  └────────────────────────┘ └──────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

## 核心数据结构

### 阶段 1: 骨架场景

```rust
// src/verify/scenario.rs

/// 场景类别
pub enum ScenarioCategory {
    Tool,       // 25 个工具逐一验证
    Permission, // 权限授予/拒绝/scope/降级
    AgentFlow,  // steer/resume/cancel/navigate
    Session,    // 持久化/恢复/分支
    Capability, // 能力生成/运行
    Skill,      // skill 加载/使用
    Meta,       // 自检/文档一致性
}

/// 场景定义
pub struct VerifyScenario {
    pub name: &'static str,
    pub category: ScenarioCategory,
    /// true = 失败即停止（核心工具/binary 问题）
    pub critical: bool,
    pub setup: Option<fn()>,
    pub steps: Vec<ScenarioStep>,
}

/// 场景步骤
pub enum ScenarioStep {
    /// 向 agent 提交一条用户消息
    SubmitMessage(String),
    /// 期望收到某个 event（按 pattern 匹配）
    ExpectEvent { pattern: &'static str, timeout_ms: u64 },
    /// 断言文件系统状态
    AssertFile { path: &'static str, predicate: FilePredicate },
    /// 断言 provider 请求中包含某内容
    AssertRequest { contains: &'static str },
    /// 等待
    Wait(u64),
}

pub enum FilePredicate {
    Exists,
    NotExists,
    Contains(&'static str),
    NotContains(&'static str),
    LineCount(usize),
}
```

### 阶段 2: 自驱动探索状态

```rust
// src/verify/explore.rs

/// 探索模式状态
pub struct ExploreState {
    /// 当前正在检查的 skill/capability 路径
    pub current_target: Option<String>,
    /// 已检查的 skill 列表
    pub checked_skills: Vec<String>,
    /// 已检查的 capability 列表
    pub checked_capabilities: Vec<String>,
    /// 已修复的条目
    pub healed: Vec<String>,
    /// 无法修复的条目
    pub failed: Vec<String>,
    /// 是否正在运行
    pub running: bool,
}

/// 自愈记录
pub struct HealRecord {
    pub target: String,
    pub diagnosis: String,
    pub applied: bool,
    pub diff: String,
}
```

### VerifyState 扩展

```rust
// src/verify/state.rs 扩展

#[derive(Debug, Clone)]
pub struct VerifyState {
    // ... 现有字段保持不变 ...

    /// L4 验证层状态（新增）
    pub l4: L4State,
}

#[derive(Debug, Clone)]
pub struct L4State {
    pub phase: L4Phase,
    pub scenarios: Vec<ScenarioState>,
    pub explore: ExploreState,
    pub folded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum L4Phase {
    Idle,
    /// 阶段 1: 骨架场景
    Scenarios,
    /// 阶段 2: 自驱动探索
    Exploration,
    Complete,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ScenarioState {
    pub name: String,
    pub category: ScenarioCategory,
    pub critical: bool,
    pub status: ScenarioStatus,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioStatus {
    Queued,
    Running,
    Passed,
    Failed(String),
    Skipped,
}
```

## 组件设计

### L4Runner

```
L4Runner
├── run_scenario(scenario) → Result
│   ├── 创建独立 AgentLoop 实例（或复用当前实例但重置状态）
│   ├── 执行 setup（如有）
│   ├── 逐条执行 steps
│   │   ├── SubmitMessage → cmd_tx.send(ProcessMessage)
│   │   ├── ExpectEvent → 监听 event_rx，超时模式匹配
│   │   ├── AssertFile → 检查文件系统
│   │   ├── AssertRequest → 检查 ScriptedProvider 记录
│   │   └── Wait → thread::sleep
│   └── 汇总结果
│
├── run_exploration() → Result
│   ├── 注入 self-verify skill
│   ├── 提交 __self_check__ 消息
│   ├── Agent 自行驱动探索
│   ├── 监听 ToolFinished / ToolStarted 事件更新 explore 状态
│   └── 收集结果
│
├── heal(target) → Result
│   ├── 读取当前内容
│   ├── 调用 LLM 生成修复建议
│   ├── 用 diff 展示改动
│   └── 写回（需用户确认）
│
└── 事件转发
    ├── ScenarioProgress → AgentEvent::TestProgress
    ├── ExploreProgress → AgentEvent::TestProgress
    └── L4Complete → AgentEvent::TestSuiteComplete
```

### SelfHeal 工具

```rust
// src/tool/builtin.rs 扩展

pub struct SelfHeal;

impl Tool for SelfHeal {
    fn name(&self) -> &str { "self_heal" }
    fn description(&self) -> &str {
        "尝试修复一个 skill/capability 文件中的问题。\
         读取当前内容，调用 LLM 生成修复，展示 diff 并写回。\
         仅用于 L4 自验证阶段 2 中的提示词级别问题。"
    }
    fn permission(&self, _args, _root) -> Permission {
        Permission::Ask("self_heal")
    }
    fn run(&self, args, ctx) -> Result<ToolOutput> {
        // 1. 读取 target_path
        // 2. 调用 LLM 生成修复版本
        // 3. 与原文件做 diff
        // 4. 展示 diff 给用户确认
        // 5. 写回
    }
}
```

### Self-Verify Skill

```markdown
# skills/self-verify.md

## 触发
当用户发送 `__self_check__` 时激活。

## 任务
你是一个自验证 agent。请按以下顺序检查：

1. **Skill 健康检查**：读取 skills/ 下每个 .md 文件，检查格式完整性。
   - 是否有 name 和 description 字段？
   - 是否有明确的触发条件？
   - 内容是否可读、无残缺？
   - 发现问题：使用 self_heal 工具修复。

2. **Capability 完整性检查**：读取 capabilities/ 下每个 manifest。
   - Environment 声明是否完整？
   - Lifecycle 是否有效？
   - 引用的入口文件是否存在？

3. **Capability 冒烟测试**：对每个 OnDemand 类型的能力，尝试 run_capability。
   - 只读能力优先，不执行破坏性操作。

4. **探索性测试**：组合工具链，测试边界条件。
   - 例如：write_file → edit_file → read_file → diff 流程
   - 例如：glob 搜索结果 → read_file 验证

## 规则
- 工具/binary 错误：记录并标记为 blocking，停止探索
- 提示词/内容问题：使用 self_heal 修复
- 每个步骤记录到 memory/verify-logs/
```

## 触发路由

```
AgentLoop::process_message
  ├── "__verify__" → 启动 L1+L2+L3+L4 验证
  │   ├── 先跑 L1-L3（现有逻辑）
  │   └── L4 阶段 1 → 阶段 2
  │
  ├── "__self_check__" → 仅启动 L4 阶段 2（自驱动探索）
  │
  └── 正常消息 → 不变
```

## TUI 仪表盘扩展

新增 L4 层显示，在现有 L3 层下方：

```
▸ [ ] L4 能力验证  0/25  0%                    ← 可折叠层
    ▸ [ ] 骨架场景  0/25  0%                    ← 阶段 1 进度
        ✔ [工具] read_file        120ms
        ✗ [工具] write_file       "permission denied"  ← 红色
        · [权限] grant_once        排队中
        ...
    ▸ [ ] 自驱动探索                              ← 阶段 2 进度
        ✔ [skill] debug-causal.md
        ⏳ [capability] my-script
        ✗ [heal] skills/xxx.md   已修复  ← 自愈记录
```

## 场景清单（骨架场景）

### Tool 场景（25 个，critical=true）

每个工具至少一个场景：提交消息 → 期望工具被调用 → 断言结果。

| 工具 | 场景名 | 关键步骤 |
|------|--------|----------|
| read_file | read_file_returns_content | 提交 "read src/main.rs" → expect ToolStarted(read_file) → expect ToolFinished |
| write_file | write_file_creates_file | 提交 "write hello.txt with 'hello'" → assert file exists → assert contains |
| edit_file | edit_file_modifies_line | 同 write_file 后编辑 → assert line changed |
| run_command | run_command_executes | 提交 "run ls" → expect ToolStarted(run_command) |
| commit | commit_creates_git_commit | 提交 "commit with message" → assert git log |
| diff | diff_shows_changes | 提交 "show diff" → expect ToolStarted(diff) |
| glob | glob_finds_files | 提交 "find all .rs files" → expect ToolStarted(glob) |
| grep | grep_finds_pattern | 提交 "search for 'fn main'" → expect ToolStarted(grep) |
| search_web | search_web_returns_results | 提交 "search web for rust" → expect ToolStarted(search_web) |
| agent | agent_spawns_subagent | 提交 "use agent to read Cargo.toml" → expect ToolStarted(agent) |
| use_skill | use_skill_loads_and_applies | 提交 "use self-verify skill" → confirm skill loaded |
| run_capability | run_capability_executes | 提交 "run capability my-script" → expect ToolStarted(run_capability) |
| ... | ... | ... |

### Permission 场景（critical=true）

| 场景名 | 关键验证点 |
|--------|-----------|
| grant_once_allows_one_call | 授予 Once → 工具可用一次 → 第二次需重新授权 |
| grant_session_remembers | 授予 Session → 同一 session 内不再弹窗 |
| grant_project_persists | 授予 Project → 持久化到 codecoder.json |
| deny_blocks_execution | 拒绝 → 工具不执行 |
| shell_cap_capped_at_session | @shell 能力 Project 授予降级为 Session |
| deny_on_unauthorized | 未授权操作自动拒绝 |

### AgentFlow 场景（critical=false）

| 场景名 | 关键验证点 |
|--------|-----------|
| cancel_interrupts_turn | 发送消息 → 取消 → 工具不执行 |
| steer_queues_follow_up | 发送消息 → 第二消息被 steer |
| resume_loads_session | 保存 session → 重启 → resume 恢复对话 |
| navigate_switches_branch | 创建分支 → navigate 切换 |
| clear_resets_state | 发送 clear → 对话清空 |

### Session 场景（critical=false）

| 场景名 | 关键验证点 |
|--------|-----------|
| session_persists_to_disk | 对话后 session 文件存在且有效 |
| session_branching_creates_fork | 分支后形成独立记录 |
| session_resume_restores_context | resume 后上下文正确 |

### Capability 场景（critical=false）

| 场景名 | 关键验证点 |
|--------|-----------|
| generate_capability_creates_files | 生成能力 → 产物文件存在 |
| run_capability_executes_shell | 运行 Shell OnDemand → 输出正确 |
| run_capability_persistent_starts | 运行 Persistent → 进程存活 |

### Skill 场景（critical=false）

| 场景名 | 关键验证点 |
|--------|-----------|
| generate_skill_creates_skill | 生成 skill → 文件存在且格式正确 |
| promote_prompt_creates_skill | 晋升 prompt → skill 文件存在 |
| use_skill_injects_content | 使用 skill → 后续请求包含 skill 内容 |

### Meta 场景（critical=false）

| 场景名 | 关键验证点 |
|--------|-----------|
| readme_allows_without_gh_token | 无 GITHUB_TOKEN 时可读 |
| arch_docs_consistent | ARCHITECTURE.md 描述与代码一致 |
| adr_cross_references_valid | ADR 间引用可解析 |

## 自愈流程

```
Agent 发现 skill/xxx.md 格式问题
  │
  ├─→ self_heal(target="skills/xxx.md", diagnosis="缺少 name 字段")
  │
  ├─→ LLM 生成修复版本
  │
  ├─→ 展示 diff（TUI 中显示）
  │
  ├─→ 用户确认 / 自动确认（L4 模式）
  │
  └─→ 写回文件
       │
       ├─→ 成功 → 记录到 heal_records
       └─→ 失败 → 记录到 failed，继续
```

## 与 24h 自进化循环的衔接

L4 验证系统是自进化循环的基础：

```
每个循环（~1h）:
  ┌─────────────────────────────────────┐
  │ 1. L4 验证（骨架场景 + 自驱动探索）  │
  │ 2. 收集失败记录                      │
  │ 3. 分析失败根因                      │
  │ 4. 尝试修复（自愈）                  │
  │ 5. 如果修复 → 重新验证              │
  │ 6. 生成进化报告                      │
  │ 7. 等待 → 下一个循环                │
  └─────────────────────────────────────┘
```

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/verify/scenario.rs` | 新建 | 场景定义框架 + 全部场景清单 |
| `src/verify/explore.rs` | 新建 | 自驱动探索状态 |
| `src/verify/event.rs` | 扩展 | 新增 L4 事件类型 |
| `src/verify/state.rs` | 扩展 | 新增 L4State |
| `src/verify/runner.rs` | 扩展 | 新增 L4Runner |
| `src/verify/mod.rs` | 扩展 | 导出新模块 |
| `src/tool/builtin.rs` | 扩展 | 新增 SelfHeal 工具 |
| `src/agent.rs` | 扩展 | 新增 __self_check__ 路由 |
| `src/tui/verify.rs` | 扩展 | L4 层仪表盘渲染 + 键盘交互 |
| `skills/self-verify.md` | 新建 | 自驱动探索 skill |
| `docs/testing/behavioral-validation.md` | 更新 | 新增 L4 描述 |