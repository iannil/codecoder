# Background Agent（headless 一次性 runner）设计

- 日期：2026-07-10
- 状态：已批准（brainstorming），待转 writing-plans
- 关联：CONTEXT.md「Background Agent」条目、ADR 0016（channel 拓扑）、0019（sub-agent 边界）、0005（权限 scope）、0021（capability lifecycle）

## 1. 目的

实现 CONTEXT.md 命名但 post-v1 未建的 **Background Agent**：一个**无 TUI、无用户在场、拥有完整 LLM loop** 的 agent，跑一个被委派的任务后退出。MVP 采用 **headless 一次性 runner**（外部 cron/CI 负责调度），并用**预授权 allowlist + 其余自动 Deny** 结构性消除 CONTEXT.md 点名的「无人在场谁答权限提示」难题。

## 2. 术语对齐（CONTEXT.md 两轴）

- **有 LLM loop？** 有。 **有用户在场？** 无。→ 这正是 Background Agent（区别于 sub-agent：有 loop 但用户在场、只读、同步 await；区别于 Persistent Capability：无 loop 的服务）。
- Background Agent **不是只读**：它用 `Toolbox::builtin()` 全集，能写文件、跑命令——但仅在预授权时（见 §4）。这与 sub-agent 的只读 9 工具集是**不同契约**，不可混淆。

## 3. 已定决策（brainstorming）

- **权限模型**：预授权 allowlist（`codecoder.json`，即已实现的 `ProjectAllowlist`）+ 其余自动 Deny。
- **入口**：headless 一次性 runner（环境变量触发），调度交给外部。

## 4. 无人在场的权限模型（核心）

- Background Agent 的 `AgentLoop` 带一个新的 `headless: bool` 态。
- 权限闸门（`dispatch_tool`）在 `headless` 时改判 `Permission::Ask { key }`：
  - `key` 在 `session allowlist` 或 `project_allowlist`（`codecoder.json`）中 → 允许直跑。
  - 否则 → **自动 Deny**：返回错误 `ToolResult { is_error: true, output: "denied: no user present; '<key>' not in project allowlist" }`。**绝不发 `PermissionRequest` 阻塞等 oneshot**（无人应答）。
- `ask_user` / `confirm` / `PlanApproval` 在 headless：不发交互事件，直接返回拒绝/默认失败的 `ToolResult`（agent 继续或收尾，不挂起）。
- 效果：用户通过**事先编辑 `codecoder.json`** 决定 Background Agent 能碰哪些 key（如 `write_file`、`run_command:git`）。这把「谁答权限」变成「启动前一次性授权」，无需运行时应答者。

> 复用而非新建：`ProjectAllowlist` 上一轮刚实现（`AlwaysThisProject` 持久化）。Background Agent 的预授权集与它**同一文件、同一 key 语义**，天然复用。

## 5. 组件与接口

### 5.1 `src/background.rs`（新）
- `pub struct BgOutcome { pub final_text: String, pub tool_calls: Vec<String>, pub denied: Vec<String>, pub events: Vec<String> }`
  - `final_text`：最后一条 assistant 文本。
  - `tool_calls`：本次实际执行的工具名（有序）。
  - `denied`：被无人权限模型拒掉的工具 key。
  - `events`：人类可读的事件行（milestones）。
- `pub fn run_background(provider: Arc<dyn Provider>, model: String, max_tokens: u32, temperature: f32, root: PathBuf, task: String) -> anyhow::Result<BgOutcome>`：
  1. 建 mpsc `(event_tx, event_rx)`。
  2. `let agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root);`
  3. **同线程**（不 spawn）驱动一个 turn：把 `task` 作为一条 user 消息，调 agent 的公开入口跑到 `TurnComplete`。
  4. turn 结束后从 `event_rx` **全量 drain** 事件，归类进 `BgOutcome`（确定性、无并发交叠）。
  5. 返回 `BgOutcome`。

### 5.2 `src/agent.rs`
- `AgentLoop` 加字段 `headless: bool`（`build` 参数；`new`/`new_sub` 传 `false`）。
- 新增 `pub fn new_background(provider, model, max_tokens, temperature, root) -> Self` = `build(..., Toolbox::builtin(), persist=true, headless=true)`。
- 新增 `pub fn run_one_turn(&mut self, task: String, event_tx: &Sender<AgentEvent>)`：headless 驱动单 turn（等价于 `process_turn` 的公开封装；若 `process_turn` 已够用则直接公开它/加薄封装）。
- `dispatch_tool` 权限闸门：`if let Permission::Ask { key } = ...`：
  - `if self.allowlist.allows(&key) || self.project_allowlist.allows(&key)` → 直跑（对 headless 与非 headless 都成立，逻辑复用）。
  - `else if self.headless` → 返回拒绝 `ToolResult`（不发 `PermissionRequest`）。
  - `else` → 现有交互路径（发 `PermissionRequest` 等 oneshot）不变。
- `ask_user`/`confirm`/`PlanApproval` 拦截点：`if self.headless` → 返回拒绝/默认，不发事件。

### 5.3 `src/lib.rs`
- `pub fn run_background(cfg: Config, task: String) -> anyhow::Result<()>`：`select_provider(&cfg)` → `background::run_background(...)` → 把 `BgOutcome` 打到 stdout（§6）→ `Ok(())`。

### 5.4 `src/main.rs`
```rust
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    codecoder::run(cfg)
}
```

## 6. 报告
- 结束把 `BgOutcome` 以两段写 stdout：人类可读（`final_text` + 执行工具 + 被拒工具）+ 一行机器可读摘要。
- session 照常落盘（`sessions/*.json`，ADR 0004），可 TUI `/resume` 复查全量。
- 退出码：正常 0；LLM/传输错误传播为非 0（`run_background` 返回 `Err`）。

## 7. 降级与边界
- 无 API key → `StubClient`：跑通但只有 stub 文本、零工具——用于确定性烟测。
- Background Agent 内部仍可用 `agent` 工具派生 sub-agent（深度锁 1 不变）；Background Agent 本身**不是** sub-agent。
- 取消：headless 无 Esc；`CancelToken` 仍在但 MVP **不接 SIGINT**（记为后续）。
- 并发/多 runner 资源限制：**超出 MVP**（CONTEXT.md 点名的难题之一），外部调度器自行限制并发。

## 8. 测试

### 8.1 黑盒 L1（`tests/l1_background.rs`，复用 testkit `ScriptedProvider`）
1. `background_runs_task_and_reports_final_text`：脚本 `read_file`（读播种文件）+ 文本回复；`run_background` 跑；断言 `final_text` 含回复文本、`tool_calls` 含 `read_file`、`sessions/*.json` 落盘。
2. `background_denies_unauthorized_ask_tool`（**核心安全断言**）：脚本一个 `write_file`（无 `codecoder.json`）；断言目标文件**未**创建、`denied` 含 `write_file`、turn 仍完成（不挂起等 oneshot）。
3. `background_allows_preauthorized_tool`：预置 `codecoder.json` 含 `write_file`；脚本 `write_file`；断言文件**已**创建、不在 `denied`。

### 8.2 单测（`src/agent.rs`）
- headless 权限闸门：`Ask{key}` 且 key 不在任何 allowlist + `headless=true` → 产出拒绝 `ToolResult`，**未发 `PermissionRequest`**（用一个不连 TUI 的 event sink 断言事件里无 `PermissionRequest`）。

## 9. 文档
- CONTEXT.md「Background Agent」条目：从「post-v1 未建、无 runner」更新为「已实现 headless 一次性 runner，预授权 allowlist 权限模型；调度交外部；SIGINT/scheduler/多 runner 限制仍为后续」。
- 新增 `docs/adr/0026-background-agent-headless-runner.md`。
- README/ARCHITECTURE/CLAUDE：把 Background Agent 移出「已知未实现」，更新测试数与模块数（新增 `background.rs` → 24 模块）。

## 10. 交付物
1. 本设计文档。
2. 实现（经 writing-plans）：`src/agent.rs`（`headless` 字段 + 闸门改判 + `new_background`/`run_one_turn` + 单测）；`src/background.rs`（`BgOutcome` + `run_background`）；`src/lib.rs`（`run_background` 包装）；`src/main.rs`（env 分派）；`tests/l1_background.rs`；ADR 0026；文档更新。

## 11. 成功判据
- `CODECODER_BG_TASK=<任务>` 启动二进制 → 无 TUI 跑完一个 turn → 把结果打 stdout + session 落盘 → 退出。
- 需 Ask 且未预授权的工具在无人模式下**自动 Deny**，绝不挂起；预授权工具正常执行。
- 现有交互（TUI）权限行为**零回归**（闸门只对 headless 分支改判）。
- 默认套件保持 hermetic 全绿；CONTEXT.md/ADR/计数同步；Background Agent 移出「已知未实现」。
