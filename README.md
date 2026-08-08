# CodeCoder

> 自主 AI agent 系统 — 事件驱动，文件系统即自我

基于 AI 大模型、用 Rust 编写的高度自主 agent。它能读/写文件、搜索网络、搜索 GitHub、逆向 API、在沙箱中运行代码，还能用工具来扩展自身能力。

## 快速开始

```bash
# 1. 设置 API key（或通过 codecoder.json 配置，见下文）
export CODECODER_API_KEY=sk-your-key-here

# 2. 编译
cargo build

# 3. 启动 daemon + 连接客户端
cargo run --bin ccda      # 启动 ccd daemon
cargo run --bin ccli      # 连接 daemon (ccli 客户端)

# 4. 在客户端中试试
ccli> 列出当前目录的文件
ccli> 搜索 GitHub 上的 Rust Web 框架
ccli> /help      # 查看所有命令
ccli> /exit      # 退出
```

## 可执行文件

CodeCoder 提供三个独立二进制（通过 `cargo run --bin <name>` 运行）：

| 二进制 | 路径 | 用途 |
|--------|------|------|
| `ccda` | `src/bin/ccda.rs` | 守护进程（daemon），长驻后台，监听 Unix socket |
| `ccli` | `src/bin/ccli.rs` | 交互式 CLI 客户端，连接 daemon 进行对话 |
| `ccweb` | `src/bin/ccweb.rs` | Web 界面客户端，通过浏览器连接 daemon |

**启动 daemon 后，ccli（或 ccweb）才能连接。**

## 无需 API key 运行

不设置 API key 时使用 StubClient（模拟 LLM 响应），可用于测试：

```bash
cargo run --bin ccda       # 启动 daemon
cargo run --bin ccli       # 连接 (ccli 客户端)
```

## 内置工具（40 个）

> **Tool / Skill / Capability 三分**:**Tool** 是编译进二进制的原生原语(下表);**Skill** 是 agent 自撰的 `.md` 程序性知识(改变怎么想);**Capability** 是 agent 自撰的可执行产物(长出新手脚)。**Skill** 另有一个草稿前身 **Prompt**(`prompts/`,经 `promote_prompt` 转正,见 `docs/adr/0025`)。详见 `CONTEXT.md` 与 `docs/adr/0020`–`0022`、`0025`。

| 工具 | 功能 |
|------|------|
| `read_file` | 读取文件内容 |
| `write_file` | 写入文件（自动创建父目录） |
| `run_command` | 执行 shell 命令（复合命令按整串 keying,不可经前缀预授权,ADR 0036） |
| `list_directory` | 列出目录内容 |
| `search_web` | 抓取 URL 内容 |
| `search_github` | 搜索 GitHub 仓库 (`repos:`) 或代码 (`code:`) |
| `reverse_api` | 抓取文档页面，提取 API endpoint 签名 |
| `generate_skill` | 撰写 Skill(`.md` 程序性知识)到 `skills/` |
| `generate_prompt` | 撰写 Prompt 草稿(Skill 草稿态)到 `prompts/` |
| `promote_prompt` | 把 `prompts/` 草稿转正为 `skills/` 里的 Skill(删草稿,撞名报错) |
| `generate_capability` | 撰写 Capability(自撰可执行产物)到 `capabilities/` |
| `generate_milestones` | 将目标分解为 workgraph 里程碑（`goal`, 可选 `context`） |
| `use_skill` | 激活某个 Skill,把全文注入后续 context |
| `run_capability` | 在声明的 Environment/Lifecycle 中执行某个 Capability |
| `glob` | 文件 glob 搜索（支持 ** 递归） |
| `grep` | 文本搜索 + AST 查询（tree-sitter） |
| `diff` | 文件差异比较 |
| `edit_file` | 精确文本替换编辑 |
| `commit` | Git 提交 |
| `review` | 架构审查(只读子 agent → 结构化 Verdict + 漂移 rubric) |
| `plan` | 任务计划 |
| `milestone` | 工作图管理（list/add/start/done/needs_fix/next/remove，持久依赖有序） |
| `memory` | 持久化 key-value 记忆读写（**跨 session 共享**） |
| `ask_user` | 用户交互 |
| `confirm` | yes/no 确认对话 |
| `agent` | 子代理调用 |
| `reason` | 推理树管理（root-cause 分析：add/status/margin/list/trace，持久化到 `causal_tree.json`，**跨 session 检索 meta 节点**） |
| `mcp_call_tool` | 调用 `codecoder.json` 中 `mcp_servers` 配置的 MCP 服务器上的工具（JSON-RPC 2.0 over stdio，ADR 0040） |
| `mcp_list_resources` | 列出 MCP 服务器暴露的资源（ADR 0040） |
| `mcp_read_resource` | 按 URI 读取 MCP 服务器上的资源内容（ADR 0040） |
| `lsp` | 代码智能查询（go_to_definition / find_references / hover / document_symbol / workspace_symbol / go_to_implementation，服务器按扩展名自动探测，ADR 0040） |
| `task_create` | 创建内存任务（返回 id） |
| `task_get` | 按 id 获取任务详情 |
| `task_list` | 列出任务（可按 status 过滤） |
| `task_update` | 更新任务字段/依赖 |
| `task_stop` | 停止任务（标记为 deleted） |
| `cron_create` | 注册 cron 定时任务（到点注入 prompt） |
| `cron_delete` | 删除 cron 任务 |
| `cron_list` | 列出 cron 任务 |
| `send_message` | 向子代理/父代理发送消息 |

## 文件系统即自我

```
project/
├── AGENTS.md     ← 系统身份声明 → 自动注入 LLM system prompt
├── CONTEXT.md    ← 项目术语表
├── skills/       ← Skill:.md 程序性知识,Registry 扫描入目录
├── capabilities/ ← Capability:agent 自撰的可执行产物 + manifest
├── memory/       ← 系统持久化的 key-value 记忆
├── causal_tree.json ← 推理树（根因分析节点，`reason` 工具管理）
├── sessions/     ← 对话历史 JSON 文件
├── docs/         ← 设计文档、ADR、审计报告
│   ├── adr/      ← 架构决策记录
│   ├── audit/    ← TUI 保真度审计
│   └── design/   ← 设计规格
├── archived/     ← 参考项目存档
└── target/       ← 编译产物
```

## REPL 命令

```
/exit, /quit    退出 REPL
/help           显示帮助
/reload         重载 context 和 skills
/clear          清除对话历史
/history        显示历史消息数
/tools          列出可用工具
/skills         列出已加载技能
/memory         列出持久化记忆
/autotask on    启用自动任务发现（从 `CODECODER_AUTOTASK_SOURCE` 轮询）
/autotask off   禁用自动任务发现
/autotask status 查看自动任务状态（运行中/已停止/间隔/来源）
```

## Capability 执行:Environment × Lifecycle

每个 Capability 在 manifest 里声明两个正交属性——在哪跑(Environment)、活多久(Lifecycle)。这取代了旧的 L0/L1/L2 分级。

**Environment(在哪跑,由 Capability 声明):**

| 环境 | 隔离方式 | 说明 |
|------|---------|------|
| `Shell` | 宿主直跑(信任域,每次调用过权限) | 逃逸口:写 `capabilities/` 须经 `/reload` 生效,权限上限 `AlwaysThisSession` |
| `Wasm` | wasmtime + WASI(无网络、限 FS) | v1 仅接受 `.wasm`/`.wat`;源码→wasm 编译单独立项 |
| `Docker` | 容器(无网络、只读挂载工作区、CPU/内存 limit) | daemon 缺失时**显式报错**,不偷偷降级到宿主 |

**Lifecycle(活多久):**

| 生命周期 | 语义 |
|---------|------|
| `OneShot` | 跑一次、抓 stdout、销毁 |
| `OnDemand` | 调起才启动,短暂可复用,随后回收 |
| `Persistent` | 后台常驻服务,跨 turn 存活,经网络/IPC 调用;崩溃不自动重启(标记 `Failed`),绑 CodeCoder 进程生命周期 |

详见 `docs/adr/0021-capability-environments-and-lifecycle.md`。

## 配置：三层 JSON 配置

所有配置不再使用环境变量表，改为三层 JSON 文件覆盖：

```
层级 1 (内置默认)  ── 编译期默认值，无需任何配置文件
层级 2 (用户级)    ── ~/.codecoder/codecoder.json，全局生效
层级 3 (项目级)    ── <project_root>/.codecoder/codecoder.json，项目专属
```

后一层覆盖前一层：项目级 > 用户级 > 内置默认。每层只需提供要覆盖的字段，缺失则使用下层值。

### 完整模板

以下是一个完整的 `codecoder.json`，包含所有可配置字段及其默认值：

```json
{
  "model": "gpt-4o",
  "api_base": "https://api.openai.com/v1",
  "api_key": null,
  "max_tokens": 8192,
  "max_tokens_ceiling": 32768,
  "temperature": 0.7,
  "github_token": null,
  "bg_max_auto": 0,
  "bg_circuit_k": 2,
  "bg_milestone_tool_cap": 15,
  "bg_max_fix_attempts": 3,
  "supervisor_crash_budget": 3,
  "max_tool_output": 262144,
  "command_timeout_secs": 0,
  "compaction_tier2": true,
  "wg_tick_secs": 30,
  "supervisor_tick_secs": 1,
  "ondemand_reaper_secs": 5,
  "auto_task_interval_secs": 300,
  "auto_task_source": "github_issues",
  "provider_retry_max": 3,
  "provider_retry_initial_ms": 1000,
  "fallback_api_base": null,
  "fallback_model": null,
  "alert_webhook": null,
  "alert_on_failure_only": true,
  "daemon_auto_restart": false,
  "probe_failure_threshold": 5,
  "wg_auto_renew": true,
  "max_sessions": 100,
  "max_ledger_lines": 10000,
  "self_observe": false,
  "noop_nudge_threshold": 3
}
```

### 最小示例

大多数项目只需覆盖少数几个字段：

```json
{
  "model": "gpt-4o",
  "bg_max_auto": 10,
  "bg_max_fix_attempts": 3,
  "max_tokens": 16384
}
```

### 保留的环境变量

以下环境变量仍被识别，用于进程路由和覆写（优先级高于 JSON 配置）：

| 变量 | 说明 |
|------|------|
| `CODECODER_ROOT` | 项目根目录（默认当前工作目录） |
| `CODECODER_DAEMON` | 设置后以 daemon 模式启动长驻服务（无 TUI） |
| `CODECODER_BG_TASK` | 设置后以 headless 模式跑完该 task 即退出 |
| `CODECODER_BG_WORKGRAPH` | 设为 `1` 时以 headless workgraph 模式跑 |
| `CODECODER_SCRIPT` | 脚本模式：从文件路径读取指令并执行 |
| `CODECODER_API_KEY` | LLM API key（也可在 codecoder.json 中配置） |
| `GITHUB_TOKEN` | GitHub API token（提升 rate limit） |

其他所有 `CODECODER_*` 环境变量已废弃，请迁移到 `codecoder.json`。

> **`.ccd.bg.ndjson` 实时 BG 日志**：headless BG（`CODECODER_BG_TASK`/`CODECODER_BG_WORKGRAPH`）运行期间，`BgObserver` 把每个 BG 事件同时写 stderr 与项目根的 `<root>/.ccd.bg.ndjson`（每行一条 JSON，一次 truncate 开轮 + 逐事件 append 累积整条流；已 gitignore），可 `tail -f` 实时观察进展（ADR 0039）。

## Background Agent 账本与告警（ADR 0033）

每次 BG 调用（独立进程、跑完即退）追加一条 JSONL 记录到 `CODECODER_ROOT/bg_ledger.jsonl`（ts / mission_state / 每 milestone 的 subgoal 结论 / counts）。`mission_state` 映射成进程退出码，外部调度器据此告警：

| mission_state | exit code |
|---|---|
| `CompletedAllReady` / `Running`（正常） | 0 |
| `BlockedAt(_)`（硬依赖断裂） | 2 |
| `StuckNeedsFix(_)`（needs_fix 重试预算耗尽，见 `bg_max_fix_attempts`） | 2 |
| `CircuitBreaker`（连续失败熔断） | 3 |
| `Error(_)`（turn/provider 出错） | 4 |
| `EmptyGraph`（空图，需先 seed） | 5 |
| SIGINT 取消 | 0（操作者主动，非故障） |

> **退出码可达性**：`CODECODER_BG_TASK=<显式 task>` 产 `Running`→0（provider 错误经 `AgentLoop.last_error` 现也→`Error`→4）；`CODECODER_BG_WORKGRAPH=1` 经 workgraph 分支可达 `CompletedAllReady`/`BlockedAt(2)`/`StuckNeedsFix(2)`/`CircuitBreaker(3)`/`EmptyGraph(5)`（空图，需先 seed）（ADR 0033 修订）。

查账本（直读文件，不经 daemon——BG 运行时 daemon 常不在场）：

```bash
ccli ledger                 # 最近 10 次（单行摘要：ts / state / counts）
ccli ledger --last 50       # 最近 N 次
ccli ledger --failed        # 仅需关注（state ≠ CompletedAllReady）
ccli ledger --detail        # 最近一次的完整 subgoals 明细
```

systemd `OnFailure=` / cron 邮件可按非 0 退出码触发告警；账本文件 append-only，用外部 logrotate 轮转。

## Auto-Memory Skill（跨 session 知识积累）

`skills/auto-memory.md` 是一个内置 Skill，帮助 agent 在每次里程碑完成后自动把学到的项目知识持久化到 `memory/auto-*.md` 文件中。这些记忆跨 session 共享，agent 在后续对话中可通过 `memory` 工具读取，形成持续的跨 session 学习能力。

详见 `skills/auto-memory.md`。

## 开发

```bash
cargo build      # 编译
cargo test       # 运行 351 个测试（348 通过 + 3 个 #[ignore]：2 Docker e2e + L3 LLM 冒烟）
                 # tests/ 下为黑盒行为验证分层（L1 默认；L2/L3 门控，见 docs/testing/behavioral-validation.md）
cargo run --bin ccda   # 启动 ccd daemon
cargo run --bin ccli   # 启动 ccli 客户端
```

## 架构

参考 `docs/` 目录下的 ADR 和设计文档。