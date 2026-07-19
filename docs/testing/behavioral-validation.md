# 行为验证（黑盒测试分层）

CodeCoder 的测试套件是一个**黑盒行为验证**分层：集成测试只编译于 `src/lib.rs`
暴露的公共 API（`codecoder::{run, select_provider, Config, Provider, AgentLoop, ...}`），
黑盒边界由编译器强制。`tests/` 下的 L1 集成层驱动**真实的 `AgentLoop` + 真实内置工具**，
仅在 provider 处用 `ScriptedProvider` 注入确定性的模型回合。

## 三面可观测（three observable faces）

断言只落在三个外部可观测面，绝不断言内部私有状态：

1. **`AgentEvent` 流** —— agent 线程经 `event_rx` 回传的流式增量与结构化状态
   （`StreamDelta` / `ToolStarted` / `ToolFinished` / `PermissionRequest` / `AskUser` / …）。
2. **文件系统 + git** —— 工具的持久化副作用（`write_file`、`memory` 落盘、`skills/` /
   `prompts/` / `capabilities/` 产物、`commit`）。
3. **`ScriptedProvider` 记录的 `CompletionRequest`** —— 送往 provider 的 wire 级请求
   （消息历史、工具 schema），用于证明「模型听到了什么」（如 `ask_user` 回灌、compaction
   占位化）。

## 分层与运行方式

| 层 | 触发 | 依赖 | 命令 |
|----|------|------|------|
| **L1 主干** | 默认 | 无 key / 无网络 / 无 docker（hermetic） | `cargo test` |
| **L2 pty 冒烟** | 门控 `RUN_PTY_SMOKE=1` | 仅本机 pty（`portable-pty`），scripted provider | `RUN_PTY_SMOKE=1 cargo test --test l2_pty_smoke -- --ignored` |
| **L3 真实 LLM** | 门控 `RUN_LLM_SMOKE=1` + `CODECODER_API_KEY` | 真实模型、网络 | `RUN_LLM_SMOKE=1 CODECODER_API_KEY=... cargo test --test l3_llm_smoke -- --ignored` |
| Docker e2e | 默认 `#[ignore]` | Docker daemon | `cargo test -- --ignored`（在装有 docker 的机器上） |

**默认 `cargo test` 只跑 L1**：L2/L3/Docker 均 `#[ignore]`d，且 L2/L3 在其门控 env 变量
未设置时**提前 return**（no-op，而非失败）——因此即便显式 `-- --ignored`、但缺 key/未开门控，
套件仍保持绿色。

- **L2 pty 冒烟**（`tests/l2_pty_smoke.rs`）：在伪终端下 spawn 真实二进制，注入
  `CODECODER_SCRIPT`（一个序列化 `Vec<Message>` 的 JSON，单条 `assistant_text` 回合）+
  `CODECODER_ROOT`=临时目录，发一行用户输入（Enter 单独发送，避免批量 CR 被吞）后读取输出，
  断言脚本文本被 TUI 渲染，再发 `/exit` 收尾。测试**自限时**（reader 线程 + `recv_timeout` +
  `kill()` 收尾，**永不挂死**）。CI（`.github/workflows/ci.yml`）以**非阻塞 job**（`continue-on-error`
  + 硬 `timeout-minutes`）运行它——pty 时序在无头 runner 上可能抖动，故不作合并门禁；主门禁是
  hermetic 的 `cargo test`。本机实测稳定通过（~1.8s）。
- **L3 真实 LLM 冒烟**（`tests/l3_llm_smoke.rs`，**优先冒烟项**）：走与 L1 相同的
  `AgentLoop` + 真实工具，但用 `select_provider(&cfg)` 换成真实 provider，请模型创建
  `hello.txt`，只断言文件系统面（文件存在）。

## 设计与覆盖矩阵

- 设计规格 + §5 覆盖矩阵：`docs/superpowers/specs/2026-07-09-codecoder-behavioral-validation-design.md`
- 术语以 `CONTEXT.md` 为准；架构见 `ARCHITECTURE.md`；决策契约见 `docs/adr/`。

## 本套件曾暴露的产品缺口（REVEALS）

黑盒测试用 `#[ignore]`d 的 **REVEALS** 测试钉住已知产品缺口：它们描述**期望的**行为、当前失败，
而非绿灯掩盖。修复 `src/` 后即摘掉 `#[ignore]`，测试转为回归守卫。**目前两处均已修复，无剩余 REVEALS。**

**已修复：**

- ~~`AlwaysThisProject` 权限非持久化~~ ✅ 已修复：新增 `permission::ProjectAllowlist`，`AlwaysThisProject`
  授予落盘到 `<root>/codecoder.json` 并在启动时 `load` 回内存、与 session allowlist 并列参与闸门判定
  （ADR 0005）；同时按 ADR 0022 ceiling 规则把 `@shell` capability 授予降级为 session、绝不落盘。
  `grant_project_persists_allowlist_to_disk` + `persisted_project_grant_is_loaded_and_suppresses_prompt`
  + `shell_capability_project_grant_capped_not_persisted`（`tests/l1_permission.rs`）为回归守卫。
- ~~取消 in-flight `run_command`~~ ✅ 已修复：`run_command` 改为 `spawn` + 轮询 cancel token +
  杀子进程（`ToolCtx.cancel`，见 ADR 0016）；`cancel_interrupts_long_running_command`
  （`tests/l1_kernel.rs`）已转为常规回归测试。TUI 侧也已接线：有 turn 在跑时按 `Esc` 直接翻转
  共享 `CancelToken`（不走 `cmd_tx`，因 agent 线程 turn 内阻塞在 `process_turn`），
  由 `esc_during_activity_flips_the_cancel_token`（`src/tui/run.rs`）守卫。端到端取消现已可用。

## L4 全能力验证层

| 层 | 触发 | 依赖 | 命令 |
|----|------|------|------|
| **L4 能力验证** | `/verify` 命令（L1-L3 之后自动执行） | 无（hermetic，使用 stub provider） | `cargo test` 触发或 `/verify` |

**L4 分两个阶段：**

### 阶段 1: 骨架场景

脚本化验证场景，覆盖：
- 25 个工具逐一验证（read_file、write_file、edit_file、run_command、glob、grep、diff、commit、memory、agent、use_skill 等）
- 权限矩阵（grant_once、session、project、deny、shell cap 降级）
- Agent 对话流程（cancel、steer、resume、navigate）
- Session 持久化（session_persists、branching、resume）
- Capability 冒烟（generate、run、persistent）
- Skill 生命周期（generate、promote、use）
- 元验证（README 可读、ADR 一致性）

**失败策略**：critical=true（工具/binary 问题）→ 立即停止；critical=false（skill/提示词问题）→ 记录并继续

### 阶段 2: 自驱动探索

注入 `self-verify` skill，由 agent 自行驱动：
- 读取 `skills/` 下每个 `.md`，验证格式完整性
- 读取 `capabilities/` 下 manifest，验证声明完整性
- 尝试 `run_capability` 做冒烟测试
- 组合工具链做探索性测试

**自愈机制**：提示词级别问题通过 `self_heal` 工具自动修复；binary 级别错误记录并停止。

### L4 代码结构

- `src/verify/scenario.rs` — 场景定义框架 + 场景清单
- `src/verify/explore.rs` — 自驱动探索状态
- `src/verify/runner.rs` — L4Runner（场景执行 + 探索执行）
- `src/verify/state.rs` — L4State 扩展
- `src/tool/builtin.rs` — SelfHeal 工具
- `skills/self-verify.md` — 自驱动探索 skill
