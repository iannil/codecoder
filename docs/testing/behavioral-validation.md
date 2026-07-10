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
  `CODECODER_ROOT`=临时目录，发一行用户输入后读取输出，断言脚本文本被 TUI 渲染，再发 `/exit`
  收尾。这是一个**最小、门控**的骨架：pty 时序天然易抖，契约是「可编译 + opt-in 时可跑」，
  非「绝对鲁棒」。
- **L3 真实 LLM 冒烟**（`tests/l3_llm_smoke.rs`，**优先冒烟项**）：走与 L1 相同的
  `AgentLoop` + 真实工具，但用 `select_provider(&cfg)` 换成真实 provider，请模型创建
  `hello.txt`，只断言文件系统面（文件存在）。

## 设计与覆盖矩阵

- 设计规格 + §5 覆盖矩阵：`docs/superpowers/specs/2026-07-09-codecoder-behavioral-validation-design.md`
- 术语以 `CONTEXT.md` 为准；架构见 `ARCHITECTURE.md`；决策契约见 `docs/adr/`。

## 本套件暴露的 2 个产品缺口（REVEALS）

黑盒测试有意保留两个 `#[ignore]`d 的 **REVEALS** 测试：它们描述**期望的**行为并当前失败，
用以钉住已知产品缺口（而非绿灯掩盖）：

1. **取消 in-flight `run_command`**（`tests/l1_kernel.rs`）——协作式取消（`Cancel` +
   cancellation token + 子进程 kill）尚不能可靠中断一个正在执行的长命令；测试记录了这一
   time-to-cancel 的缺口。
2. **`AlwaysThisProject` 权限非持久化**（`tests/l1_permission.rs`）——项目级 allowlist
   应持久化到 `codecoder.json` 并跨会话生效，但当前授予未落盘；测试记录该 gap。

移除这两个 `#[ignore]` 前，应先在 `src/` 修复对应行为。
