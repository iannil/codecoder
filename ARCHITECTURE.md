# CodeCoder 架构

自主 AI agent,Rust 编写,**事件驱动、文件系统即自我**。本文串起 23 个源模块 ↔ 16 个 ADR ↔ 24 个内置工具 ↔ 6 点自我进化诉求,供人与未来 agent 导航。术语以 `CONTEXT.md` 为准,决策依据见 `docs/adr/`。

## 一句话

一个跑在 OS 线程 + channel 上的 agent 内核,用 provider 中立的消息模型对话,用一套自报权限的工具行动,把「技能/能力」当文件存在磁盘上自我扩展,并能在 shell/wasm/docker 三种环境里执行自撰的代码。

## 运行时形状(ADR 0016)

```
        cmd_tx (AgentCommand: ProcessMessage/Resume/Reload/Cancel/Shutdown)
  ┌──────────────┐ ───────────────────────────────▶ ┌──────────────┐
  │  TUI 线程     │                                    │  agent 线程   │
  │ (main thread)│ ◀─────────────────────────────── │  AgentLoop   │
  └──────────────┘  event_rx (AgentEvent: 流式增量/    └──────────────┘
      阻塞往返         工具状态/权限请求/ask/通知)              │
   (permission/ask)  经 AgentEvent 内嵌 reply_tx oneshot ◀────┘
```

- **无 async 运行时**:阻塞式,HTTP 用 ureq,子 agent/服务用 OS 线程/子进程。
- **权限/ask 应答走 oneshot**,不走 cmd_tx——待决请求的答复不会被新消息错序。
- **取消**是协作式:`Cancel` + cancellation token + 子进程 kill(不硬杀线程)。

## 模块地图

| 模块 | 职责 | ADR |
|---|---|---|
| `main.rs` | 入口:选 provider、起 agent 线程、跑 TUI、退出时杀常驻服务 | 0016 0024 |
| `config.rs` | `CODECODER_*` 环境变量 | — |
| `message.rs` | `Message`/`MessageItem`/`MessageId`/`Role`(provider 中立) | 0015 0017 |
| `provider/{mod,openai,stub}` | `Provider` trait;`OpenAiClient`(chat-completions)/`StubClient` | 0017 |
| `agent.rs` | `AgentLoop`、turn 循环、工具分派、子 agent、ask_user、reload | 0016 0019 |
| `permission.rs` | `PermScope`/`Permission`/`PermissionKey`/`SessionAllowlist` | 0005 0018 |
| `tool/mod.rs` | `Tool` trait、`Toolbox`(父/子 read-only)、wire schema | 0018 0019 |
| `tool/builtin.rs` | 文件/执行/自我进化/委派工具 | 0018 0020 0022 |
| `tool/{net,dev,search,wasm}.rs` | 联网 / 开发 / glob·grep(含 AST)/ wasm 运行器 | 0018 0021 |
| `session.rs` | `Session`、自动落盘、前向迁移链、`/resume` | 0004 |
| `compaction.rs` | 派生的 Context Working Set(不毁持久记录)。**v1 存根:原样返回全量历史;分层混合压缩见 0023,尚未实现** | 0023 |
| `tokenizer.rs` | tiktoken 精确计数 + 模型→窗口表 | 0023 |
| `registry.rs` | 扫 `skills/`+`capabilities/` → 常驻目录 | 0020 |
| `capability.rs` | `Environment`/`Lifecycle`/manifest/`RunningServiceTable` | 0021 |
| `memory.rs` | `memory/<key>` 文件级 KV + 数据索引 | — |
| `tui/{mod,render,run}` | `TuiApp`/派生 `Mode`/`Theme`/`Dialog`/`Popup`、渲染、主循环 | 0001 0003 0024 |

## 一个 turn 的生命周期

1. 用户在 TUI 输入 → `cmd_tx` 送 `AgentCommand::ProcessMessage`。
2. `AgentLoop::process_turn`:追加 user `Message` → Session **自动落盘**(0004)。
3. 取 **Context Working Set**(0023,派生自全量 messages)+ 前置 **System prompt**(AGENTS.md + 目录,0020)。
4. `tokenizer` 精确计 token → `AgentEvent::Context{pct}` 驱动状态栏 `ctx%`。
5. `Provider::complete`:中立消息 ↔ OpenAI 线格式翻译(0017);`Reasoning` 不回灌(0004)。
6. 回复含 `ToolCall` → 逐个**串行**分派(0016):
   - 权限闸门(0018):`None` 直跑;`Ask{key}` 查 allowlist,否则发 `PermissionRequest` **阻塞等 oneshot**。
   - `agent`/`review`/`ask_user` 被 `AgentLoop` **拦截**(需 provider/事件通道)。
   - 执行 → `ToolResult` 回灌 → 再问 LLM,直到无工具调用或触及 `MAX_TOOL_ITERATIONS`。
7. 无工具调用 → `TurnComplete`。

## 工具体系(24 内置)

`Tool` 自报 `Permission`(0018):`None`(只读免问)/ `Ask{key}`(细粒度 key,命令类/前缀甜点区)。**子 agent 只能用 `Permission::None` 的一个只读子集(9 个),且无 `agent`——深度锁 1(0019)。**

| 类 | 工具 | 权限 | 子 agent |
|---|---|---|---|
| 文件 | read_file · list_directory | None | ✓✓ |
| | write_file · edit_file | Ask | |
| 搜索 | glob · grep(text+AST) | None | ✓✓ |
| 执行 | run_command | Ask(`run_command:<类>`) | |
| 自我进化 | use_skill | None | ✓ |
| | run_capability | Ask(`run_capability:<名>@<env>`) | |
| | generate_skill · generate_prompt · generate_capability | Ask | |
| 委派/交互 | agent(子 agent)· review(只读 review 子 agent)· ask_user | 拦截 | |
| 联网 | search_web · search_github · reverse_api | None | ✓✓✓ |
| 开发 | diff | None | ✓ |
| | commit | Ask(`commit`) | |
| | plan · todo · memory | None(本地 scratch) | |

## 自我进化闭环

三分(`CONTEXT.md`):**Tool 天生(编译进二进制)· Skill 学来的想法(`.md` 知识,只改怎么想)· Capability 造出来的手脚(可执行物,真行动)。**

```
agent 深思 → generate_skill / generate_capability   (写文件到 skills//capabilities/)
           → /reload                                 (Registry 重扫 → 常驻目录 + System prompt)
           → use_skill(注入全文) / run_capability(执行)  (后续按需自主选择)
```

**Capability 执行**(0021)= `Environment` × `Lifecycle`:

| Environment | | Lifecycle |
|---|---|---|
| `Shell` 宿主(信任,过权限) | | `OneShot` 跑完即销毁 |
| `Wasm` wasmtime+WASI(无网/无 FS) | | `OnDemand` 调起短用回收 |
| `Docker` 容器(无网/只读挂载/限额) | | `Persistent` 常驻服务(运行中服务表,不自动重启,绑进程生命周期) |

**6 点诉求映射:** ①`generate_skill`+`use_skill` ②`generate_capability`+`run_capability` ③联网工具+`memory` 数据索引持久化 ④`search_github`+`Shell`/`Docker` Capability ⑤三 Environment ⑥三 Lifecycle。

## 权限与安全

- **PermissionKey** 细到命令类/环境(`run_command:git`、`run_capability:foo@shell`),而非整工具名。
- **天花板规则**(0005/0022):`@shell` 能力上限 `AlwaysThisSession`,永不 `AlwaysThisProject`——它是唯一自修改逃逸口;自撰生效须经一次可见 `/reload`(不热注册)。
- **闸门在执行侧**:`generate_*` 仅 `write_file` 级;真正危险在 `run_capability`。Capability 进程拿不到 agent 工具集,不能递归自撰。
- **子 agent 无用户通道** → 只能用 `Permission::None` 工具(没人能答它的权限提问),这是 read-only 的可强制定义。
- **隔离不静默降级**:Docker 缺失/Wasm 未编译 → 明确报错,绝不偷偷落到宿主。

## TUI(0001/0003/0024)

- **全屏托管视口**(alternate screen):放弃原生 scrollback,换任意行可重绘(Reasoning 折叠/Dialog/BROWSE)。
- **`Mode` 派生**、每帧从子状态算(dialog→help→popup→search→browse→insert),不存字段、不失同步。
- **事件/动画驱动渲染**:统一 channel 阻塞 recv,空闲零 CPU;动画期 ~20fps tick。
- 3 区(messages/input/status)+ 活动行;permission/ask **Dialog**、slash/@file **Popup**、search、help、完整 readline;`Enter` 提交 / `Shift+Enter`(Kitty 协议,`Ctrl+J` 兜底)换行。

## ADR 索引

`0001` TUI 键位与 Mode 语义 · `0002` slash 本地派发 · `0003` 中心 Theme · `0004` Session 持久化与迁移 · `0005` 权限 scope 与 allowlist · `0007` prompt 注入 slash · `0015` 统一消息模型 · `0016` channel 拓扑与事件模型 · `0017` provider 中立消息模型 · `0018` Tool trait 与 PermissionKey · `0019` 子 agent 能力边界 · `0020` Skills/Capabilities Registry · `0021` Capability 环境与生命周期 · `0022` 自撰安全回路 · `0023` 上下文压缩 · `0024` TUI 视口与渲染循环。

## 测试与验证边界

- **46 个离线单元/集成测试**(默认套件,hermetic)+ **2 个 Docker e2e**(`#[ignore]`,`cargo test -- --ignored`)。Wasm e2e 在默认套件内(纯进程内)。
- **无法在无 TTY 环境验证 TUI 交互**——需真终端。TUI 观感由人工真机验收。
- token 计数用 tiktoken(准确);`run_command`/`commit` 走真实 `git`/shell(运行期生效)。

## 交互能力(全部实现)

四种 Dialog 均有触发:`ToolPermission`(权限)· `PlanApproval`(`plan` 工具)· `AskQuestion`(`ask_user`)· `Confirm`(`confirm` 工具,yes/no)。鼠标滚轮滚动 transcript;`F2` 切换鼠标捕获以启用终端原生选择/复制。grep AST 支持 rust/python/javascript/go/c。

自绘选区 copy-mode 有意不做——全屏 TUI 下工程量大、价值低,`F2` 放行原生复制已覆盖需求。
