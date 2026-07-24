# CLAUDE.md

本文件为 Claude Code (claude.ai/code) 在此仓库中工作时提供指导。

## 项目状态

CodeCoder 是一个**已落地**的自主 AI agent，使用 Rust 编写。仓库已有 Cargo 项目、43 个源文件(`src/`,含 `background.rs`、`trust.rs`、`retry.rs`、`review.rs`、`workgraph.rs`、`daemon/`、`client/`)、26 个内置工具、339 个测试(336 通过 + 3 个 `#[ignore]`:2 Docker e2e + L3 真实 LLM 冒烟),以及 30 份 ADR(`docs/adr/`)。`tests/` 下为黑盒行为验证分层(L1 默认;L2/L3 门控),见 `docs/testing/behavioral-validation.md`。架构总览见 `ARCHITECTURE.md`;领域术语以 `CONTEXT.md` 为准。

**已知未实现的部分(文档中已标注,勿误以为已就绪):**

- **Wasm Capability 源码→wasm 编译未实现**:`run_capability` 的 Wasm 环境只接受预编译 `.wasm`/`.wat`,源码跨编译单独立项(ADR 0021)。
- **Persistent 服务跨重启韧性已实现(ADR 0034)**:`supervisor_state.json` 持久化 gave_up/crash_count/manifest mtime;daemon 重启后跳过超预算/gave_up 的服务、manifest 变更自动重置。**会话内仍守 ADR 0021**(崩溃即 give_up、不自动重启;预算只管"重启后是否再 spawn")。live handles(PID)仍绑进程生命周期,不持久化。
- **内置调度器 / 多 runner 资源上限**:调度外置,并发由外置调度器限制(ADR 0026)。
- **margin/leverage/terminal 仅为字符串元数据**:"关键节点 = 余量 × 杠杆"的排序未内核化,靠 agent/skill 判断(reason 工具;rc 纪律 skill 待写)。

> **Background Agent 已落地 headless one-shot runner**(见 ADR 0026):由 `CODECODER_BG_TASK=<task>` 触发,无用户在场地跑完一个 task 即退出;另有 `CODECODER_BG_WORKGRAPH=1` 走 **workgraph 逐里程碑模式**(无显式 task,产出 mission_state→退出码 0/2/3/4,见 ADR 0033 修订)——优先推进 `pending` 就绪里程碑;里程碑验收 `needs_fix` 时 runner 在预算内(`CODECODER_BG_MAX_FIX_ATTEMPTS`,默认 3,0=禁用)自动把上轮失败原因注入修复 prompt 重试(重试不计入 max_auto),预算耗尽仍 `needs_fix` 才报 `StuckNeedsFix`(退出码 2,此时需人工/上层介入)——**自恢复仅限该 headless runner**(`run_background_cfg`),交互式 `drive_workgraph` 与 daemon 空闲推进线程仍只标记 `needs_fix`、需人工重置 `pending`;权限走 `codecoder.json` 预授权,任何未授权的 Ask 工具被自动拒绝(记入 `BgOutcome.denied`、发 `ToolFinished{is_error}` 事件),从不弹 prompt。调度外置;**SIGINT 优雅取消已实现**(`Ctrl+C`/`kill -INT` 经 signal-hook 接 `CancelToken`,见 ADR 0026);内置调度器/多 runner 资源上限仍属延后项。**编排纪律**:切忌向同一常驻 daemon **并发**发消息(共享 session 历史 + 异步写 → 文件版本竞争);并发工作请用独立 root/daemon 或串行化(ADR 0035 已护 workgraph 并发写,session 历史无此保护)。

> **Compaction 已全量实现**(tier-1 + tier-2,见 ADR 0023):tier-1 超阈值时丢 `Reasoning` + 占位化旧 `ToolResult` 正文,保护 anchor 与近端 tail;tier-1 后仍超阈值时,`AgentLoop::context_working_set` 用一次带缓存的 LLM 调用把最旧跨度摘要为合成 `System` 消息,摘要失败/为空则降级回 tier-1。tier-2 摘要采用结构化模板、迭代式合并(span 增长只摘增量并带入上一版摘要),并累积追踪 read/modified 文件路径附于摘要末尾(见 docs/adr/0023 增强说明)。

改动代码时依据 `CONTEXT.md` 的术语与 `docs/adr/` 的决策契约;新增/修改功能后请同步更新 `ARCHITECTURE.md`、`README.md` 中的相关数字与描述,使文档与代码保持一致。

## 命令

```bash
cargo build          # 编译
cargo test           # 运行测试套件
cargo test <name>    # 按名称子串运行单个测试
CODECODER_DAEMON=1 cargo run  # 启动 ccd daemon
cargo run --bin cc            # 启动 cc 客户端
```

未设置 `CODECODER_API_KEY` 时会回退到 `StubClient`（模拟 LLM 响应）——这是无需真实 key 进行测试的预期方式。

关键环境变量：`CODECODER_API_KEY`（真实 LLM 必需）、`CODECODER_MODEL`（默认 `gpt-4o`）、`CODECODER_API_BASE`、`CODECODER_ROOT`（项目根目录，默认为当前工作目录）、`GITHUB_TOKEN`（提升 GitHub rate limit）。完整列表见 `README.md`。

## 架构

**「文件系统即自我」** 是核心设计原则：agent 的身份与能力由磁盘上的文件定义，而非硬编码，并在运行时加载/重载：

- `AGENTS.md` — 系统身份声明，自动注入 LLM system prompt
- `CONTEXT.md` — 项目术语表（领域术语的权威来源）
- `skills/` — Skill:`.md` 程序性知识,`Registry` 扫描入常驻目录,`use_skill` 激活时注入全文
- `capabilities/` — Capability:agent 自撰的可执行产物 + manifest,`run_capability` 按声明的 Environment/Lifecycle 执行
- `memory/` — 持久化的 key-value 记忆
- `sessions/` — 以带版本号的 JSON 保存的对话历史（通过 `/resume` 加载）
- `docs/adr/` — 架构决策记录；`docs/design/`、`docs/audit/`

**事件驱动、多线程内核(OS 线程 + channel,非 tokio/lunatic)。** daemon 长驻,监听 Unix socket;`cc` 客户端经 stdin/stdout 交互,permission/ask/confirm/plan/trust 弹窗行内 `y/n`。daemon 线程经 `cmd_tx` 发 `AgentCommand`(仅用户主动意图:ProcessMessage、Shutdown、Cancel);agent 线程经 `event_rx` 回传 `AgentEvent`(流式增量 + 结构化状态)。权限/`ask_user` 应答走 `AgentEvent` 内嵌的 `reply_tx` oneshot,**不走** `cmd_tx`——故 `PermissionResponse` 不是 `AgentCommand`。一个 turn 内工具**串行**执行;取消是协作式:`cc` 的 `Ctrl+C`(有 turn 在跑时)直接翻转共享 `CancelToken`(不走 `cmd_tx`,因 agent 线程 turn 内阻塞在 `process_turn`),`run_command` 与 shell Capability 轮询该 token 并 kill 子进程。只有顶层 agent 拥有面向用户的通道;`agent` 工具派生的 sub-agent 汇报回父 agent,其 read-only 的精确含义 = 工具集仅限 `Permission::None` 那批,且深度锁定为 1。详见 `docs/adr/0016`、`0019`、`0032`。

**三分自我进化:Tool / Skill / Capability。** **Tool** 是编译进二进制的原生原语(26 个:读/列/写/编辑文件、运行命令、带 tree-sitter AST 查询的 glob/grep、`diff`、web 与 GitHub 搜索、`reverse_api`、git `commit`、`plan`、`milestone`、`memory`、`ask_user`、`confirm`、`agent`、`review`、`generate_skill`、`generate_prompt`、`promote_prompt`、`generate_capability`、`use_skill`、`run_capability`、`reason`;完整清单见 `README.md` 表)。**Skill** 是 agent 自撰的 `.md` 程序性知识(只改变「怎么想」,不执行);**Prompt** 是 Skill 的草稿态(`prompts/`,经 `promote_prompt` 晋升为 Skill,见 ADR 0025);**Capability** 是 agent 自撰的可执行产物(长出新手脚)。三者由 `Registry` 扫描 `skills/`、`prompts/`、`capabilities/` 成常驻目录、按需激活。Capability 在自己 manifest 里声明 **Environment**(`Shell`/`Wasm`/`Docker`)× **Lifecycle**(`OneShot`/`OnDemand`/`Persistent`);权限闸门在 `run_capability`(按 `能力名+环境` keying),`generate_*` 仅 `write_file` 级。消息模型 provider 中立,OpenAI 为规范协议。详见 `CONTEXT.md` 与 `docs/adr/0017`、`0018`、`0020`–`0022`。

## 使用领域术语——重要

`CONTEXT.md` 是一份术语表，其存在正是为了防止命名错误。每个术语都列出一行 `_Avoid_:`，标明**不得**使用的近义词。请在代码、注释和文档中精确遵守这些约定。该术语表强制的重要区分：

- **Mode** vs. Dialog vs. Popup vs. Overlay——TUI 每帧恰有一个活跃的 Mode（INSERT、SEARCH、DIALOG 等）；Dialog 是*阻塞式*模态，Popup 是*非阻塞式*，Overlay 泛指两者。
- **Session**（持久化的 JSON 对话）vs. **History**（用于 Up/Down 的内存输入缓冲区）——切勿混淆。
- **MessageId**（每个 session 内的 `u64`，是整条消息的 UI/持久化标识）vs. **ToolCall.id**（面向 provider 的 tool_use/tool_result 关联 id）。
- **Slash Command**（本地拦截，从不发给 LLM）vs. **Prompt-Injecting Slash Command**（展开为 prompt 并转发给 LLM，仅当轮有效）vs. **Agent Command**（TUI→agent 通道消息）。
- **Permission Scope**（`Once` / `AlwaysThisSession` / `AlwaysThisProject`）与内存中的 **Session Allowlist** vs. 持久化在 `codecoder.json` 中的项目级 allowlist。

当架构决策以 ID 引用时（例如 `[[0015-unified-message-model]]`），它们对应应存放于 `docs/adr/` 下的 ADR。

## 关于 `archived/`

`archived/`（已被 gitignore）存放第三方参考项目——`claude-code`、`lunatic`、`nocobase`。它们仅供参考，不属于本代码库；请勿编辑或构建它们。
