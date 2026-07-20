# TUI 自动化测试方案设计

## 背景

CodeCoder 是一个全屏 TUI agent 应用（基于 Ratatui + Crossterm），有 9 种 Mode（Insert、Search、RSearch、Dialog、Help、Model、Slash、Browse、Verify），3 区布局（transcript/activity/input+status），以及 Permission/Ask/Plan/Confirm/Trust 五种 Dialog。当前存在 L2 PTY 门控冒烟测试（`tests/l2_pty_smoke.rs`）和少量 Handler 单元测试（`src/tui/run.rs`），但缺少对渲染层的系统性覆盖。

## 目标

建立一个 **100% hermetic、全量 Mode 覆盖** 的 TUI 测试分层，消除对 PTY 端到端测试的依赖，使 TUI 布局变更在 CI 中可被自动检测。

## 方案

### 方案选择：A — Ratatui TestBackend 深度快照

利用 Ratatui 内置的 `TestBackend` 在内存中模拟终端屏幕（80×24 固定尺寸），渲染后通过 `insta` 做快照比对。三条测试层次全部走向 100% hermetic。

### 测试金字塔

```
┌─────────────────────────────────────┐
│  PTY 端到端（1 条，维持现状）          │ ← 门控，不扩展
├─────────────────────────────────────┤
│  Handler 逻辑测试（~15 条新增）        │ ← 纯状态转换断言
├─────────────────────────────────────┤
│  Render 快照测试（~30 条）            │ ← 核心新增层
│  每个 Mode × 关键状态 → TestBackend   │
│  渲染 → insta 快照比对                │
└─────────────────────────────────────┘
```

## 测试用例矩阵

### 1. Render 快照测试（`src/tui/render.rs` — `snapshot_tests` 模块）

借助 `ratatui::backend::TestBackend(80, 24)` 渲染并抓取纯文本网格。

#### Insert Mode（5 个快照）

| 用例 | 构造状态 | 验证点 |
|------|---------|--------|
| 空输入 | `input=""`, `blocks=[]` | 3 区布局正确，状态栏 INSERT，输入栏 placeholder |
| 含文本输入 | `input="hello world"` | 输入栏显示文本，光标位置 |
| 多行输入 | `input="line1\nline2\nline3"` | 输入栏高度自适应 |
| 有活动行 | `activity=Some(...)` | 活动区显示 spinner + label + "esc to cancel" |
| Permission Dialog | `dialog=Some(ToolPermission)` | 居中覆盖，4 选项，选中项高亮 |

#### Transcript 渲染（7 个快照）

| 用例 | 构造状态 | 验证点 |
|------|---------|--------|
| 空 transcript | `blocks=[]` | 空白消息区 |
| User 消息 | `blocks=[Block::User("hello")]` | "you › hello" 前缀 |
| Assistant 消息 | `blocks=[Block::Assistant("response")]` | "cc  · response" 前缀 |
| Tool 块（无结果） | `Block::Tool{result=None}` | "▪ name preview"，无结果行 |
| Tool 块（折叠长结果） | 10+ 行结果，`folded=true` | 首行 + "· N lines ▸" |
| Reasoning 折叠 | `Block::Reasoning{folded=true}` | "▸ reasoning (N lines)" |
| 混合多块 | 多种 Block 混合 | 块间空白分隔，顺序正确 |

#### Dialog 全类型（6 个快照）

| 用例 | 构造状态 | 验证点 |
|------|---------|--------|
| ToolPermission（4 选项） | 标准 PermissionDialog | 边框标题，选项列表，选中高亮 |
| ToolPermission（无 project） | `key="run_command:git"` | 不显示 project 选项（@shell 降级规则） |
| AskQuestion | `AskDialog{prompt, input}` | 边框标题 "Question"，输入行 |
| PlanApproval | `PlanDialog{selected=0}` | 边框标题 "Plan — approve?"，approve/reject 选项 |
| Confirm | `ConfirmDialog` | "yes/no" 选项 |
| Trust | `TrustDialog` | "always/once/never" 选项 |

#### Search / Browse / Help / Verify（8 个快照）

| 用例 | 构造状态 | 验证点 |
|------|---------|--------|
| Search 活跃 | `search_active=true` | 搜索栏替换输入栏 |
| R-Search 活跃 | `search_active=true, reverse_search=true` | "r-search> " 前缀 |
| Browse 选中 | `browsing=true, browse_sel=0` | 选中块反色，状态栏提示 |
| Help 打开 | `help_open=true` | 居中覆盖，键位绑定列表 |
| Verify 运行中 | `running=true`，部分用例通过 | spinner + 进度条 + 用例列表 |
| Verify 全部通过 | 全部 passed | 绿色完成状态 |
| Verify 有失败 | 含 Failed 用例 | 红色状态，错误详情 |
| Verify 状态栏 | 同上 | 状态栏显示 "Tab expand · ↑↓ select · F5 rerun · Esc exit" |

#### Popup（2 个快照）

| 用例 | 构造状态 | 验证点 |
|------|---------|--------|
| Slash 补全 | `popup={kind:Slash, items}` | 浮窗在输入区上方，选中项高亮 |
| 文件补全 | `popup={kind:File, items}` | 同上，显示文件路径 |

### 2. Handler 逻辑测试（`src/tui/run.rs` — 扩展 `tests` 模块）

#### Insert Key Handler（8 条）

| 用例 | 输入 | 断言 |
|------|------|------|
| 提交空文本 | `"  "` → Enter | 不发送消息，不增加 blocks |
| 提交普通文本 | `"hello"` → Enter | `blocks` 有 User 块，`cmd_tx` 收到 `ProcessMessage("hello")` |
| 提交时已有 turn | `"hello"` + `activity=Some` → Enter | steer 队列收到消息，不发送 ProcessMessage |
| /exit | `"/exit"` → Enter | `should_quit=true`，发送 `Shutdown` |
| /resume | `"/resume"` → Enter | 发送 `Resume` |
| /clear | `"/clear"` → Enter | `blocks` 清空，`browsing=false` |
| Ctrl+C | Ctrl+C 按键 | `should_quit=true` |
| Shift+Enter | Shift+Enter | 输入栏插入 `\n`，不提交 |

#### Dialog Handler（7 条）

| 用例 | 输入 | 断言 |
|------|------|------|
| Permission 选 once | Enter 选中项 | dialog 关闭，reply_tx 收到 `Grant(Once)` |
| Permission Esc 拒绝 | Esc | dialog 关闭，收到 `Deny` |
| Ask 输入并提交 | 输入 "yes" → Enter | dialog 关闭，reply_tx 收到 "yes" |
| Ask Esc 取消 | Esc | dialog 关闭，收到空字符串 |
| Plan 审批 | 按 'a' | dialog 关闭，reply_tx 收到 `true` |
| Plan 拒绝 | 按 'r' 或 Esc | dialog 关闭，reply_tx 收到 `false` |
| Confirm 确认 | 按 'y' 或 Enter | dialog 关闭，reply_tx 收到 `true` |

#### Agent Event Handler（5 条）

| 用例 | 输入 | 断言 |
|------|------|------|
| 连续 StreamDelta | 收到 3 个 delta | `streaming=true`，最后一个 Assistant 块累积拼接 |
| ToolStarted 结束流 | delta → ToolStarted | 新 Tool 块创建，`streaming=false` |
| ToolFinished 填充 | ToolStarted → ToolFinished | 对应 Tool 块 result 被填充，折叠状态正确 |
| PermissionRequest 弹出 | 收到 PermissionRequest | `app.dialog` 变成 ToolPermission |
| TurnComplete 清空 | 收到 TurnComplete | `streaming=false`, `activity=None` |

### 3. PTY 端到端测试（`tests/l2_pty_smoke.rs`）

维持现有 L2 单条冒烟测试，**不扩展**。新增的 Render 快照 + Handler 逻辑测试已覆盖全量交互场景。

## 文件分布

| 文件 | 当前内容 | 新增内容 | 新增测试数 |
|------|---------|---------|-----------|
| `src/tui/render.rs` | 纯渲染函数 | 新增 `snapshot_tests` 模块 | ~30 个快照测试 |
| `src/tui/run.rs` | 现有 5 个测试 | 扩展 `tests` 模块 | ~15 个逻辑测试 |
| `src/tui/mod.rs` | 现有 7 个测试 | 保持不动 | 0 |
| `tests/l2_pty_smoke.rs` | 1 个门控测试 | 保持不动 | 0 |

## 依赖

- `insta` crate（快照测试框架）— 需加入 `[dev-dependencies]`
- `ratatui` 的 `TestBackend` — 已通过 `ratatui` 依赖可用

## 快照管理

```
cargo test                    # 生成 / 更新快照文件
cargo insta review            # 交互式审查 diff
git add src/tui/snapshots/    # 快照随代码提交
```

快照文件由 `insta` 自动管理在 `src/tui/snapshots/` 下。

## 未覆盖范围

- 动画（Tick 事件驱动的 activity spinner 动画仅在 ~20fps 下可见，静态渲染无法捕捉）
- 鼠标交互（ScrollUp/ScrollDown 已在 Handler 测试中覆盖 scroll 字段变化）
- 真实终端尺寸变化（固定 80×24，响应式布局测试留作后续扩展）