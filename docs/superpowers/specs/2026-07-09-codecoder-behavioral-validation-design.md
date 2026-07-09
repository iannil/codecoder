# CodeCoder 行为验证方案（不通过阅读代码验证）

- 日期：2026-07-09
- 状态：已批准（brainstorming 阶段），待转 writing-plans
- 目标读者：人与未来 agent

## 1. 目的与约束

为 CodeCoder 的**全部能力**设计一套深度验证方案，硬性约束是**不能通过阅读源码来"验证"**——
验证只能建立在运行时的**可观测行为**之上。方案分层落地：一条确定性的通道级回归主干覆盖所有能力，
外加薄薄一层真实 LLM / TUI 冒烟以获得端到端信心。

### 决策边界（brainstorming 已确认）

- 验证边界：**分层**——通道级确定性回归为主干，少量真实 LLM 冒烟验证端到端。
- 交付形态：**文档 + 可运行 harness**（本文档 + 后续实现）。
- 覆盖面：**内核 + 25 工具 + 自我进化 + 权限 + 子 agent + session 持久化/迁移 + compaction tier-1**；不含 TUI 像素级渲染。
- 主干哈射形态：**方案 1 — 抽出 `lib.rs` + `tests/` 集成测试**（黑盒边界由编译器强制）。

### 两个决定形态的架构事实

1. **二进制仅有 crate，无 `lib.rs`、无 `tests/` 目录**；现有 53 个测试全部是模块内 `#[cfg(test)]` 单测。
   Rust 集成测试（`tests/*.rs`）只能看到 crate 的**公开库 API**；bin-only crate 不暴露任何公开面。
   → 必须抽出 `lib.rs` 才能让集成测试在"只碰公开 API"的前提下驱动 agent。
2. **`StubClient` 只返回一条固定文本、零 tool_call**。因此没有真实 LLM 时 agent **永不端到端执行任何工具**。
   → 行为验证工具必须引入一个**确定性、会发 tool_call 的脚本化 provider**。

## 2. 核心原则：把 agent 变成可观测黑盒

所有断言只落在三个**公开可观测面**上，绝不读内部逻辑：

1. **`AgentEvent` 事件流**（agent→TUI 通道的公开 enum）：流式增量、工具状态、`ToolResult`、
   `PermissionRequest`、ask、`Context{pct}`、`TurnComplete`、通知等。
2. **文件系统 + git 副作用**：write/edit 落盘、`skills/`·`prompts/`·`capabilities/`·`memory/`·`sessions/`
   产物、`promote_prompt` 的 draft→skill 迁移、`commit` 后的 `git log`、`codecoder.json` allowlist。
3. **`ScriptedProvider` 记录的 `CompletionRequest`**：agent 回灌给"LLM"的东西——系统提示注入
   （AGENTS.md + 目录）、context working set、compaction 结果、provider 中立线格式。

> 关键洞见：provider 边界是一个合法且强大的黑盒观察点。脚本化 provider 既**驱动**（发 tool_call），
> 又**观测**（记录每个 `CompletionRequest`），无需读任何内部代码即可验证系统提示、上下文与 compaction。

## 3. 分层结构

| 层 | 内容 | 确定性 | 进 CI |
|---|---|---|---|
| **L1 — 通道级回归（主干）** | `tests/` 集成测试：`ScriptedProvider` + `AgentLoop`，经 `cmd_tx`/`event_rx` 驱动；在 `TempDir` 里断言 事件 + 磁盘 + git。覆盖全部能力。 | 确定性、无外部依赖、快 | ✅ 默认 |
| **L2 — pty TUI 冒烟（薄）** | 1–2 个测试用伪终端驱动真实二进制，经环境变量选 provider，断言已知结果被渲染。验证 `main → TUI → agent` 接线。 | 基本确定 | 门控 / `#[ignore]` |
| **L3 — 真实 LLM 冒烟（最薄）** | 少量自然语言 prompt，需 `CODECODER_API_KEY` + `RUN_LLM_SMOKE=1`，默认 `#[ignore]`。验证真实模型确实能驱动工具。 | 非确定性 | 仅按需 |

## 4. 哈射机制（使能项）

1. **抽出 `lib.rs`**：只暴露公开面——`AgentLoop`、`AgentCommand`、`AgentEvent`、`Provider` +
   `CompletionRequest`/`Message`、`Config`，以及断言所需的 `permission`/`session`/`registry` 类型。
   `main.rs` 收敛为薄壳 `codecoder::run()`。**这正是让"黑盒边界由编译器强制"的东西**——
   集成测试物理上碰不到私有内部。抽取后**先跑现有 53 测试确认零回归**。

2. **`ScriptedProvider`（仅测试用）**：实现 `Provider` trait；持有一个脚本化程序——按迭代/请求状态
   返回下一条 assistant `Message`（文本 + `tool_call`）。同时把收到的每个 `CompletionRequest`
   记入 `Arc<Mutex<Vec<CompletionRequest>>>` 供断言。
   - L2/L3 的 provider 注入：在 lib `run()` 加一个 provider 选择的环境钩子（如
     `CODECODER_PROVIDER=scripted` + 脚本文件路径），仅供 pty 冒烟使用。

3. **驱动工具（driver）**：起 agent 线程，发 `ProcessMessage`，带**超时**把事件抽到 `TurnComplete`，
   按每个测试的策略经内嵌 `reply_tx` 自动应答 `PermissionRequest`/`ask_user`，
   返回 `(events, recorded_requests)`。

4. **临时工作区**：每个测试用全新的 `CODECODER_ROOT` TempDir（按需播种
   `AGENTS.md`/`CONTEXT.md`/`skills/`；commit 类测试 `git init`）。

## 5. 能力覆盖矩阵

> 每一行 = 一个可审计的验证点：输入（脚本 tool_call 或 NL）→ 仅可观测的断言。

### 5.1 内核 / turn 循环

| 能力 | 输入 | 可观测断言 |
|---|---|---|
| turn 生命周期 | 脚本单轮文本回复 | 流式增量按序 → `TurnComplete` 触发 |
| 多迭代工具循环 | 脚本 tool_call → 结果回灌 → 再 call | 第 2 个 `CompletionRequest` 里含上一步 `ToolResult` |
| `MAX_TOOL_ITERATIONS` | 脚本无限 tool_call | 在上限处停止（记录的请求数封顶） |
| 系统提示注入 | 任意一轮 | `CompletionRequest[0]` system 含 AGENTS.md 正文 + skill/capability 目录 |
| 取消（协作式） | 脚本长 run_command + 发 `Cancel` | turn 中止、子进程被 kill（观测：无后续事件 / Cancelled） |

### 5.2 文件 / 搜索 / 开发 / 执行工具

| 工具 | 权限 | 断言 |
|---|---|---|
| read_file · list_directory | None | 直跑，`ToolResult` 匹配播种文件 |
| write_file · edit_file | Ask | 发 `PermissionRequest{key}`；Allow→磁盘落文件；Deny→无文件 + 拒绝写入 ToolResult |
| glob · grep（含 tree-sitter AST） | None | 命中匹配播种内容（AST 查询单列一例） |
| diff | None | 输出符合预期 |
| run_command | Ask(`run_command:<类>`) | 权限 key 分类正确；Allow 后观测命令副作用（如 touch 文件） |

### 5.3 自我进化（三分闭环）

| 工具 | 断言 |
|---|---|
| generate_skill | `skills/<name>.md` 落盘；reload 后 use_skill 注入全文（下一个 `CompletionRequest` 含 skill 正文） |
| generate_prompt | `prompts/<name>.md` 落盘，目录标 `[draft]` |
| promote_prompt | 草稿删除 + skill 生成；**撞名 → 错误被上报**（观测 FS 迁移 + 错误 ToolResult） |
| generate_capability | `capabilities/<name>/` + manifest 落盘 |
| run_capability | 在声明的 Environment 执行（Shell 必测；Wasm/Docker 门控），权限 key = `名@env`，观测副作用 |

### 5.4 子 agent 边界（ADR 0019）

- `agent` 派生子 agent → 父 agent 收到子 agent 汇报（观测父侧结果）
- 只读强制：脚本让子 agent 尝试 write_file → 不可用 / 报错（工具集仅 9 个 `Permission::None`）
- 深度锁 1：子 agent 尝试再派生 `agent` → 不可用

### 5.5 权限系统（ADR 0005）

- 两次 write_file：首次答 `AlwaysThisSession` → 第二次**不再**发 `PermissionRequest`
- 答 `AlwaysThisProject` → `codecoder.json` 持久化 allowlist 落盘（区分 Session allowlist vs 项目 allowlist）
- 答 `Once` → 第二次仍发 `PermissionRequest`

### 5.6 Session 持久化 / 迁移（ADR 0004）

- 一轮后 `sessions/<id>.json` 落盘且带版本号；内容 round-trip
- 前向迁移：播种旧版本 session JSON fixture → 经 resume 路径加载 → 断言迁移到当前版本、无数据丢失

### 5.7 Compaction tier-1（ADR 0023）

- 构造超阈值合成历史（多个大 `ToolResult`）→ 跑 turn → 经记录的 `CompletionRequest` 断言：
  `Reasoning` 被丢、旧 `ToolResult` 正文占位化、anchor + 近端 tail 保留
- `Context{pct}` 事件反映下降
- **磁盘上的持久 Session 不变**（compaction 是派生 working-set，非破坏性）

### 5.8 交互 / 本地 scratch

- ask_user / confirm：发 ask 事件带 `reply_tx` → 应答 → 答案进下一个 `CompletionRequest`
- plan · todo · memory：memory 写 `memory/<key>` 文件；观测落盘

### 5.9 联网工具（门控，非主干）

- search_web · search_github · reverse_api：非 hermetic。方案：本地 mock HTTP server 断言；
  否则下沉到 L3（`RUN_NET=1` 门控）。**主干保持无外部依赖。**

## 6. 风险与门控项

- **联网工具**非 hermetic → 本地 mock server 或下沉 L3（`RUN_NET=1`）。
- **Wasm / Docker** capability 环境需对应运行时 → 沿用现有 2 个 docker e2e 的 `#[ignore]` / env 门控。
- **`lib.rs` 抽取**触碰 `main.rs`——小但真实的改动，必须保持行为完全等价（抽完先跑现有 53 测试确认零回归）。
- **非确定性**：L3 断言只落在稳健的可观测结果上（如"文件是否存在"），不断言具体措辞。

## 7. 交付物

1. 本设计文档（含第 5 节完整覆盖矩阵，逐一映射 25 工具 + 各子系统 → 测试 id → 可观测断言，使覆盖可审计）。
2. 实现（经 writing-plans 细化）：
   - `lib.rs` 抽取；`main.rs` 收敛为薄壳。
   - `tests/testkit/`：`ScriptedProvider` + driver + 临时工作区脚手架。
   - 按类别的 `tests/l1_*.rs`（内核 / 文件 / 搜索执行 / 自我进化 / 子 agent / 权限 / session / compaction / 交互）。
   - `tests/l2_pty_smoke.rs`（门控）、`tests/l3_llm_smoke.rs`（env 门控）。
   - 文档化的 cargo 调用 / CI 说明；覆盖矩阵与测试 id 对齐。

## 8. 成功判据

- L1 主干在无 API key、无网络、无 Docker 的环境下**全绿且确定性**，覆盖第 5 节矩阵每一行。
- 每条断言都能追溯到三个可观测面之一，**无一条依赖读取被测内部实现**。
- 现有 53 个单测在 `lib.rs` 抽取后零回归。
- L2/L3 门控测试可按需运行并通过。
