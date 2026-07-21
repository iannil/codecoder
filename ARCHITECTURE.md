# CodeCoder 架构

自主 AI agent,Rust 编写,**事件驱动、文件系统即自我**。本文串起 41 个源文件 ↔ 23 个 ADR ↔ 26 个内置工具 ↔ 6 点自我进化诉求,供人与未来 agent 导航。术语以 `CONTEXT.md` 为准,决策依据见 `docs/adr/`。

## 一句话

一个跑在 OS 线程 + channel 上的 agent 内核,用 provider 中立的消息模型对话,用一套自报权限的工具行动,把「技能/能力」当文件存在磁盘上自我扩展,并能在 shell/wasm/docker 三种环境里执行自撰的代码。

## 运行时形状(ADR 0016 + client-server migration)

```
        cmd_tx (AgentCommand: ProcessMessage/Resume/Reload/Cancel/Shutdown)
  ┌──────────────┐ ───────────────────────────────▶ ┌──────────────┐
  │  daemon 线程  │                                    │  agent 线程   │
  │ (Unix socket)│ ◀─────────────────────────────── │  AgentLoop   │
  └──────────────┘  event_rx (AgentEvent: 流式增量/    └──────────────┘
      cc 客户端          工具状态/权限请求/ask/通知)              │
      stdin/stdout     经 AgentEvent 内嵌 reply_tx oneshot ◀────┘
   (permission/ask)
```

- **client-server 架构**: `ccd` daemon 长驻,监听 Unix socket; `cc` 客户端无状态,经 stdin/stdout 交互(ADR 0032)。
- **权限/ask/confirm/plan/trust 弹窗**经 daemon wire protocol 往返,`cc` 在终端行内显示 `y/n` 提示(Task 9a)。
- **无 async 运行时**:阻塞式,HTTP 用 ureq,子 agent/服务用 OS 线程/子进程。
- **取消**是协作式:`cc` 的 `Ctrl+C`(仅当有 turn 在跑时)**直接翻转共享 `CancelToken`**。`run_command` 与 shell Capability 经同一 `run_shell_cancellable` 轮询该 token 并 kill 子进程,turn 循环在每次迭代顶部再检查一次(不硬杀线程)。

## 模块地图

| 模块 | 职责 | ADR |
|---|---|---|
| `main.rs` | 入口分发:`CODECODER_BG_TASK`→`run_background`, 否则→`run_daemon` | 0016 0026 0032 |
| `config.rs` | `CODECODER_*` 环境变量 | — |
| `message.rs` | `Message`/`MessageItem`/`MessageId`/`Role`(provider 中立) | 0015 0017 |
| `provider/{mod,openai,stub}` | `Provider` trait;`OpenAiClient`(chat-completions)/`StubClient` | 0017 |
| `agent.rs` | `AgentLoop`、turn 循环、工具分派、子 agent、ask_user、reload、**workgraph 自动推进(`drive_workgraph`)** | 0016 0019 |
| `background.rs` | Background Agent headless one-shot runner;`run_background` 驱动一个 turn 到结束,汇总为 `BgOutcome`;**无显式 task 时从 workgraph 取就绪里程碑推进** | 0026 |
| `permission.rs` | `PermScope`/`Permission`/`PermissionKey`/`SessionAllowlist` | 0005 0018 |
| `tool/mod.rs` | `Tool` trait、`Toolbox`(父/子 read-only)、wire schema | 0018 0019 |
| `tool/builtin.rs` | 文件/执行/自我进化/委派/验收工具 | 0018 0020 0022 |
| `tool/{net,dev,reason,search,wasm}.rs` | 联网 / 开发 / **推理树(reason 工具,**跨 session 检索**) / glob·grep(含 AST)/ wasm 运行器 | 0018 0021 |
| `session.rs` | `Session`、自动落盘、前向迁移链、`/resume`、**树状会话(Phase A:parent 指针 + leaf)** | 0004 |
| `compaction.rs` | 派生的 Context Working Set(不毁持久记录)。**tier-1 + tier-2 已实现** | 0023 |
| `tokenizer.rs` | tiktoken 精确计数 + 模型→窗口表 | 0023 |
| `registry.rs` | 扫 `skills/`+`prompts/`+`capabilities/` → 常驻目录(prompts 标 `[draft]`);**每个条目带 `SourceInfo` 溯源元数据** | 0020 0025 |
| `capability.rs` | `Environment`/`Lifecycle`/manifest/`RunningServiceTable`、`Supervisor`(Persistent 服务崩溃→标记 Failed 可见,**不自动重启**,0021) | 0021 |
| `memory.rs` | `memory/<key>` 文件级 KV + 数据索引(**跨 session 共享**) | — |
| `workgraph.rs` | **Work Graph(一等公民 #2)**:持久化、依赖有序的里程碑图,`Milestone` 节点含 `NodeStatus`(含 `Hypothesis`/`Locked`)、`next_ready()` 调度、`render_for_prompt()` 摘要 | 设计文档 |
| `review.rs` | **结构化验收裁决(一等公民 #4)**:`Verdict`(pass/needs_fix/rebuild)+ 四信号(`foundation`/`over_engineering`/`volume`/`terminology`),纯函数解析 | 设计文档 |
| `daemon/{mod,bus,proto,session_manager,socket}` | **Daemon(长驻服务)**:Unix socket 监听、多 client 复用、session 管理、permission/ask/confirm/plan/trust 往返 wire protocol | 0032 |
| `client/mod.rs` | **cc 客户端**:daemon 连接、stdin→消息、消息→stdout 格式化、permission 弹窗行内 `y/n` | 0032 |

## 一个 turn 的生命周期

1. 用户在 `cc` 客户端输入 → 经 Unix socket 送 `AgentCommand::ProcessMessage`。
2. `AgentLoop::process_turn`:追加 user `Message` → Session **自动落盘**(0004)。
3. 取 **Context Working Set**(0023,派生自全量 messages)+ 前置 **System prompt**(AGENTS.md + 目录 + **workgraph 状态摘要**,0020)。
4. `tokenizer` 精确计 token → `AgentEvent::Context{pct}` 驱动状态栏 `ctx%`。
5. `Provider::complete`:中立消息 ↔ OpenAI 线格式翻译(0017);`Reasoning` 不回灌(0004)。
6. 回复含 `ToolCall` → 逐个**串行**分派(0016):
   - 权限闸门(0018):`None` 直跑;`Ask{key}` 查 allowlist,否则发 `PermissionRequest` **阻塞等 oneshot**。
   - `agent`/`review`/`ask_user` 被 `AgentLoop` **拦截**(需 provider/事件通道)。
   - 执行 → `ToolResult` 回灌 → 再问 LLM,直到无工具调用或触及 `MAX_TOOL_ITERATIONS`。
7. 无工具调用 → `TurnComplete` → `run()` 循环自动调用 `drive_workgraph()` 推进 workgraph 就绪里程碑(**Plan #2 自动驱动**)。

## 工具体系(25 内置)

`Tool` 自报 `Permission`(0018):`None`(只读免问)/ `Ask{key}`(细粒度 key,命令类/前缀甜点区)。**子 agent 只能用 `Permission::None` 的一个只读子集(9 个),且无 `agent`——深度锁 1(0019)。**

| 类 | 工具 | 权限 | 子 agent |
|---|---|---|---|
| 文件 | read_file · list_directory | None | ✓✓ |
| | write_file · edit_file | Ask | |
| 搜索 | glob · grep(text+AST) | None | ✓✓ |
| 执行 | run_command | Ask(`run_command:<类>`) | |
| 自我进化 | use_skill | None | ✓ |
| | run_capability | Ask(`run_capability:<名>@<env>`) | |
| | generate_skill · generate_prompt · promote_prompt · generate_capability | Ask | |
| 委派/交互 | agent(子 agent)· review(只读子 agent → 结构化 Verdict + 漂移 rubric,`review.rs`)· ask_user | 拦截 | |
| 联网 | search_web · search_github · reverse_api | None | ✓✓✓ |
| 开发 | diff | None | ✓ |
| | commit | Ask(`commit`) | |
| | plan · milestone(工作图,`workgraph.rs`)· memory | None(本地 scratch) | |
| 推理 | reason(推理树,`reason.rs` · `causal_tree.json`) | None(本地 scratch) | |

## 自我进化闭环

三分(`CONTEXT.md`):**Tool 天生(编译进二进制)· Skill 学来的想法(`.md` 知识,只改怎么想)· Capability 造出来的手脚(可执行物,真行动)。**

```
agent 深思 → generate_skill / generate_prompt / generate_capability
                                                (写文件到 skills//prompts//capabilities/)
           → /reload                            (Registry 重扫 → 常驻目录 + System prompt)
           → use_skill(注入全文,含 [draft] Prompt) / run_capability(执行)
           → promote_prompt                     (草稿证明有用后 → skills/,删草稿,ADR 0025)
```

**Skill 有一个更低成熟度的草稿前身 `Prompt`(`prompts/`,0025):同为注入式知识,`Registry` 标 `[draft]`、排在 Skills 之后,`use_skill` 按 `skills/`→`prompts/` 回退激活;`promote_prompt` 原子地把草稿转正为 Skill 并删草稿(撞名报错)。它不是第四种 kind,是"学来的想法"这一类的草稿台阶。**

**Capability 执行**(0021)= `Environment` × `Lifecycle`:

| Environment | | Lifecycle |
|---|---|---|
| `Shell` 宿主(信任,过权限) | | `OneShot` 跑完即销毁 |
| `Wasm` wasmtime+WASI(无网/无 FS) | | `OnDemand` 调起短用回收 |
| `Docker` 容器(无网/只读挂载/限额) | | `Persistent` 常驻服务(运行中服务表,不自动重启,绑进程生命周期) |

**6 点诉求映射:** ①`generate_skill`+`use_skill` ②`generate_capability`+`run_capability` ③联网工具+`memory` 数据索引持久化 ④`search_github`+`Shell`/`Docker` Capability ⑤三 Environment ⑥三 Lifecycle。

## 一等公民清单

codecoder 的「文件系统即自我」覆盖了三层身份与工作/推理层:

```
身份层「agent 是谁」(已落地):
  - AGENTS.md / CONTEXT.md / skills/ / prompts/ / capabilities/
  - memory/ 持久 KV · sessions/ 事后记录树(树状,Phase A)
  - trust 门禁(0028) · SourceInfo 溯源(#6)

工作/推理层「在做什么 / 在想什么」(已落地):
  - Work Graph(事前构造之图,Plan #2):workgraph.json + milestone 工具 + drive_workgraph 自动驱动
  - 推理树(事后诊断之树,#3):causal_tree.json + reason 工具 + debug-causal skill
  - 验收裁决(Review Verdict, #4):review.rs 结构化裁决 + 四信号 rubric
  - 统一节点模型(NodeStatus: Hypothesis/Locked)
```

## 权限与安全

- **PermissionKey** 细到命令类/环境(`run_command:git`、`run_capability:foo@shell`),而非整工具名。
- **天花板规则**(0005/0022):`@shell` 能力上限 `AlwaysThisSession`,永不 `AlwaysThisProject`——它是唯一自修改逃逸口;自撰生效须经一次可见 `/reload`(不热注册)。
- **闸门在执行侧**:`generate_*` 仅 `write_file` 级;真正危险在 `run_capability`。Capability 进程拿不到 agent 工具集,不能递归自撰。
- **子 agent 无用户通道** → 只能用 `Permission::None` 工具(没人能答它的权限提问),这是 read-only 的可强制定义。
- **隔离不静默降级**:Docker 缺失/Wasm 未编译 → 明确报错,绝不偷偷落到宿主。

## Client-Server UI(0032)

- **`ccd` daemon**:长驻服务,监听 Unix socket,管理多 client、session 复用、Registry 共享。
- **`cc` 客户端**:无状态 CLI,经 stdin/stdout 交互,permission/ask/confirm/plan/trust 弹窗行内 `y/n`。
- **已移除 ratatui TUI**:原全屏托管视口(Mode/Dialog/Popup/alternate screen)已删除,仅 daemon+cc 两路。

## ADR 索引

`0001` TUI 键位与 Mode 语义(已废弃)· `0002` slash 本地派发 · `0003` 中心 Theme(已废弃)· `0004` Session 持久化与迁移 · `0005` 权限 scope 与 allowlist · `0007` prompt 注入 slash · `0015` 统一消息模型 · `0016` channel 拓扑与事件模型 · `0017` provider 中立消息模型 · `0018` Tool trait 与 PermissionKey · `0019` 子 agent 能力边界 · `0020` Skills/Capabilities Registry · `0021` Capability 环境与生命周期 · `0022` 自撰安全回路 · `0023` 上下文压缩 · `0024` TUI 视口与渲染循环(已废弃)· `0025` Prompt 作为 Skill 草稿层 · `0026` Background Agent · `0027` pi 对照分析 · `0028` 项目信任加载门禁 · `0029` turn steering 与 follow-up · `0031` 拦截中间件边界论证(不做)· `0032` client-server 架构 ·

## 测试与验证边界

- **202 个离线单元/集成测试**(默认套件,hermetic)+ **3 个 `#[ignore]`**(2 Docker e2e + L3 真实 LLM 冒烟;`cargo test -- --ignored`,部分另需门控 env)。Wasm e2e 在默认套件内(纯进程内)。
- `tests/` 下为**黑盒行为验证分层**:只编译于 `src/lib.rs` 公共 API,驱动真实 `AgentLoop`+真实工具,断言只落三面(`AgentEvent` 流 / 文件系统+git / `ScriptedProvider` 记录的 `CompletionRequest`)。分层与门控开关见 `docs/testing/behavioral-validation.md`。
- **L2 pty 冒烟测试已删除**(原 TUI 交互验证);client-server 交互由 daemon+`cc` 的手动验收覆盖。
- token 计数用 tiktoken(准确);`run_command`/`commit` 走真实 `git`/shell(运行期生效)。

## 交互能力(全部实现)

五种 Dialog 均有触发:`ToolPermission`(权限)· `PlanApproval`(`plan` 工具)· `AskQuestion`(`ask_user`)· `Confirm`(`confirm` 工具,yes/no)· `TrustQuestion`(`trust` 工具)。均经 daemon wire protocol 往返,`cc` 在终端行内显示 `y/n` 提示。grep AST 支持 rust/python/javascript/go/c。