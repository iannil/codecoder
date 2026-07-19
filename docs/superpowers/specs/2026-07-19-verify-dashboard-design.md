# CodeCoder 自验证仪表盘设计

- 日期：2026-07-19
- 状态：设计已批准，待转 writing-plans
- 目标读者：人与未来 agent

## 1. 目的

为 CodeCoder 设计一套**自验证仪表盘**，使 agent 能通过 `/verify` 命令运行现有的分层测试套件（L1/L2/L3），在 TUI 中以仪表盘形式实时展示验证进度与结果，并支持验证失败后的自我修复。

## 2. 核心原则

- **不改写任何现有测试代码**：复用 `cargo test --format json` 的原生输出
- **不改变现有测试的 hermetic 性**：L1 保持无外部依赖，L2/L3 保持门控
- **验证结果可被 agent 用于自我修复**：验证失败时 agent 知道什么失败了
- **TUI 仪表盘复用现有渲染管线**：新增 `Mode::VERIFY`，走已存在的 Mode 派生链

## 3. 架构概览

```
用户输入 /verify
       │
       ▼
TUI 拦截 /verify (Slash Command 本地处理)
       │
       ├─ 发送 AgentCommand::ProcessMessage("verify")
       └─ TUI 切换到 Mode::VERIFY, 初始化 VerifyState
              │
              ▼
       AgentLoop::process_turn 识别验证意图
              │
              ▼
       1. emit TestSuiteLoaded (预扫测试清单)
       2. spawn cargo test --test l1_* ... --format json
       3. 逐行解析 JSON 事件 → emit TestProgress
       4. 测试完成 → emit TestSuiteComplete
       5. 子进程退出，agent 回到空闲
              │
              ▼
       TUI 仪表盘实时渲染进度
       (用户可 Esc 退出 / 查看失败详情 / F5 重新运行)
```

## 4. 模块拆分

### 4.1 新增文件

```
src/
├── verify/
│   ├── mod.rs          # 模块根，导出公共类型
│   ├── runner.rs       # cargo test 子进程启动 + JSON 行解析
│   ├── state.rs        # VerifyState（TUI 端状态树）
│   └── event.rs        # AgentEvent 新增变体定义
├── tui/
│   └── verify.rs       # 新增：VerifyMode 的渲染和交互
```

### 4.2 修改文件

| 文件 | 修改内容 |
|------|---------|
| `src/tui/run.rs` | 新增 `/verify` 命令拦截处理 |
| `src/tui/render.rs` | 新增 `Mode::VERIFY` 分支，调用 `render_verify_dashboard` |
| `src/tui/mod.rs` | 新增 `pub mod verify` |
| `src/agent.rs` | 新增验证消息识别和验证流程 |
| `src/agent.rs` → `AgentEvent` | 新增 `TestProgress` / `TestSuiteLoaded` / `TestSuiteComplete` 变体 |

### 4.3 各模块职责

#### `src/verify/runner.rs` — 核心验证引擎

```rust
pub struct VerifyRunner {
    child: Option<Child>,           // cargo test 子进程
    reader: Option<JoinHandle<()>>,  // 输出读取线程
    cancel: CancelToken,             // 共享取消令牌
}

impl VerifyRunner {
    /// 启动 L1 测试（非阻塞，默认入口）
    pub fn start_l1(root: &Path, event_tx: Sender<AgentEvent>) -> Self;

    /// 启动指定测试文件
    pub fn start_tests(files: &[&str], root: &Path, event_tx: Sender<AgentEvent>) -> Self;

    /// 轮询子进程状态（agent loop 调用）
    pub fn poll(&mut self) -> Option<VerifyOutcome>;

    /// 取消运行中的测试
    pub fn cancel(&mut self);
}
```

`cargo test --format json` 输出格式示例：
```json
{"type":"test","event":"started","name":"turn_completes_and_streams"}
{"type":"test","event":"ok","name":"turn_completes_and_streams","exec_time":0.023}
{"type":"test","event":"failed","name":"write_file_denied_leaves_no_file","exec_time":0.045,"stdout":"assertion `left == right` failed: ..."}
{"type":"suite","event":"ok","name":"l1_kernel","passed":5,"failed":0,"allowed_fail":0,"ignored":0,"measured":0,"filtered_out":0,"exec_time":0.234}
```

#### `src/verify/state.rs` — TUI 状态树

```rust
pub struct VerifyState {
    pub layers: [LayerState; 3],     // L1/L2/L3
    pub focus: VerifyFocus,            // 当前焦点
    pub running: bool,                 // 是否在运行中
    pub started_at: Instant,
    pub total_tests: usize,
    pub completed: usize,
}

pub struct LayerState {
    pub name: &'static str,         // "L1 主干"
    pub modules: Vec<ModuleState>,
    pub folded: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

pub struct ModuleState {
    pub name: &'static str,         // "tools", "kernel", ...
    pub cases: Vec<CaseState>,
    pub folded: bool,
}

pub struct CaseState {
    pub name: String,
    pub status: CaseStatus,
    pub output: Vec<String>,
    pub duration_ms: u64,
}

pub enum CaseStatus {
    Queued,
    Running,
    Passed,
    Failed(String),  // 失败原因
    Skipped,
}

pub enum VerifyFocus {
    None,
    Case { layer: usize, module: usize, case: usize },
    Module { layer: usize, module: usize },
    Layer(usize),
}
```

#### `src/verify/event.rs` — 事件类型

```rust
pub struct TestProgress {
    pub suite: String,
    pub case: String,
    pub status: TestStatus,
    pub output: Option<String>,
    pub duration_ms: u64,
}

pub struct TestSuiteLoaded {
    pub suites: Vec<SuiteInfo>,
}

pub struct TestSuiteComplete {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total: usize,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    pub error: Option<String>,
}

pub enum TestStatus { Passed, Failed, Skipped, Running }

pub struct SuiteInfo {
    pub name: String,
    pub module: String,
    pub layer: Layer,
    pub test_count: usize,
    pub test_names: Vec<String>,
}

pub enum Layer { L1, L2, L3 }
```

## 5. 测试模块映射

| 测试文件 | 模块名 | 层 | L1 |
|---------|--------|----|----|
| `l1_kernel.rs` | kernel | L1 | 5 |
| `l1_tools.rs` | tools | L1 | 8 |
| `l1_self_evolution.rs` | self-evolution | L1 | 5 |
| `l1_permission.rs` | permission | L1 | 5 |
| `l1_session.rs` | session | L1 | 2 |
| `l1_subagent.rs` | subagent | L1 | _ |
| `l1_background.rs` | background | L1 | 5 |
| `l1_interaction.rs` | interaction | L1 | _ |
| `l1_compaction.rs` | compaction | L1 | _ |
| `l2_pty_smoke.rs` | pty-smoke | L2 | 1 |
| `l3_llm_smoke.rs` | llm-smoke | L3 | 1 |

## 6. TUI 渲染设计

### 布局

```
┌─────────────────────────────────────────────────────────┐
│  CodeCoder 验证仪表盘                    [运行中] 0:12   │
├─────────────────────────────────────────────────────────┤
│  ┌─ L1 主干 (hermetic) ──────── 28/36 完成 ───── 78% ─┐ │
│  │  [✔] kernel      (7/7)   ████████████████  PASS    │ │
│  │  [✔] tools       (8/8)   ████████████████  PASS    │ │
│  │  [⏳] self-evolution (4/5) ████████████░░░  4/5    │ │
│  │  [✔] permission  (5/5)   ████████████████  PASS    │ │
│  │  [✔] session     (2/2)   ████████████████  PASS    │ │
│  │  [✗] subagent    (2/3)   ████████░░░░░░░░  2/3    │ │
│  │     └── subagent_cannot_write_file  FAIL           │ │
│  │         assertion failed: ...                       │ │
│  │  [⏳] background  (2/3)   ████████░░░░░░░░  2/3    │ │
│  │  [✔] interaction (3/3)   ████████████████  PASS    │ │
│  │  [✔] compaction  (2/2)   ████████████████  PASS    │ │
│  ├─ L2 pty 冒烟 ──────── 门控 ──────────────── ⏸ ────┤ │
│  │  [⏸] pty-smoke   (0/1)  ░░░░░░░░░░░░░░░░  SKIP   │ │
│  ├─ L3 真实 LLM ──────── 门控 ──────────────── ⏸ ────┤ │
│  │  [⏸] llm-smoke   (0/1)  ░░░░░░░░░░░░░░░░  SKIP   │ │
│  └─────────────────────────────────────────────────────┘ │
│  总计: 31/41 通过 | 1 失败 | 9 跳过 | 耗时 12.3s     │ │
│  Tab 展开/折叠  ↑↓ 选择  Enter 展开详情  Esc 退出     │ │
└─────────────────────────────────────────────────────────┘
```

### 键盘交互

| 键 | 操作 |
|----|------|
| `↑` / `↓` | 上下移动焦点 |
| `Enter` | 展开/折叠当前焦点项 |
| `Tab` | 在焦点模式间切换（Layer→Module→Case） |
| `Esc` | 退出验证模式（正在运行则取消） |
| `F5` | 重新运行验证 |
| `Home` / `End` | 跳到列表首/尾 |

### 颜色方案

| 状态 | 颜色 |
|------|------|
| Passed | 绿色 |
| Failed | 红色 |
| Running | 黄色 |
| Skipped | 灰色 |

## 7. Mode 派生

在现有派生链中新增 `VERIFY`：

```
dialog → popup → search → browse → VERIFY → INSERT
```

进入方式：`/verify` 命令
退出方式：`Esc`（测试运行中则取消 → 再按一次退出模式）

## 8. 错误处理与边界情况

| 场景 | 处理 |
|------|------|
| `cargo` 不可用 | 报告错误，TUI 显示"cargo not found" |
| 编译失败 | 报告编译错误，可展开查看完整日志 |
| 单条测试超时（>60s） | 标记为 `Failed(timeout)`，继续跑其他测试 |
| 全部测试超时（>120s） | kill 子进程，emit 含超时标记的完成事件 |
| 子进程挂死 | reader 线程 `recv_timeout` 超时 → kill |
| 用户取消 | 翻转 CancelToken → kill 子进程 → 保留已完成的测试结果 |
| 测试输出过大 | 截断至 50 行 / 4096 字符，可展开查看完整输出 |
| L2/L3 门控未开 | 显示为 `⏸ SKIP`，灰色，不影响整体通过数 |
| 结果持久化 | 可选保存到 `verify-results/<timestamp>.json` |

## 9. 验证结果持久化

```
verify-results/
├── 2026-07-19T15-30-00.json
└── latest.json  (symlink)
```

报告格式：
```json
{
  "timestamp": "2026-07-19T15:30:00Z",
  "layers": [
    {
      "name": "L1",
      "passed": 28,
      "failed": 1,
      "skipped": 7,
      "elapsed_ms": 12345,
      "modules": [
        { "name": "tools", "passed": 8, "failed": 0, "cases": [...] }
      ]
    }
  ],
  "summary": { "passed": 31, "failed": 1, "skipped": 9, "total": 41 }
}
```

## 10. 不修改的文件

- `message.rs` — 验证过程不涉及消息模型变更
- `tool/builtin.rs` — 不新增工具，走 Slash Command
- `provider/*` — 不涉及 provider 变更
- `session.rs` — 不涉及
- `permission.rs` — 不涉及
- 所有现有测试文件 — 不改写

## 11. 成功判据

- 实现了 `/verify` 命令，TUI 切换到仪表盘模式
- 仪表盘正确展示 L1/L2/L3 三层结构，每层含模块和测试用例
- 实时进度条随测试完成更新
- 测试通过/失败/跳过状态正确显示（颜色 + 图标）
- 失败用例可展开查看详细输出
- 取消机制正常工作（Esc → 子进程终止 → 保留已有结果）
- `cargo` 不可用/编译失败等错误情况有合理提示
- 门控测试（L2/L3）在条件未满足时显示为 `SKIP`
- 现有测试套件零回归