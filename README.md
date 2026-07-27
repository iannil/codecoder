# CodeCoder

> 自主 AI agent 系统 — 事件驱动，文件系统即自我

基于 AI 大模型、用 Rust 编写的高度自主 agent。它能读/写文件、搜索网络、搜索 GitHub、逆向 API、在沙箱中运行代码，还能用工具来扩展自身能力。

## 快速开始

```bash
# 1. 设置 API key
export CODECODER_API_KEY=sk-your-key-here
export CODECODER_MODEL=gpt-4o  # 默认 gpt-4o

# 2. 启动 daemon + 连接客户端
CODECODER_DAEMON=1 cargo run  # 启动 ccd daemon
cargo run --bin cc            # 连接 daemon (cc 客户端)

# 3. 在客户端中试试
cc> 列出当前目录的文件
cc> 搜索 GitHub 上的 Rust Web 框架
cc> /help      # 查看所有命令
cc> /exit      # 退出
```

## 无需 API key 运行

不设置 API key 时使用 StubClient（模拟 LLM 响应），可用于测试：

```bash
CODECODER_DAEMON=1 cargo run  # 启动 daemon
cargo run --bin cc            # 连接 (cc 客户端)
```

## 内置工具（27 个）

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
| `milestone` | 工作图管理（list/add/start/done/needs_fix/next/remove，持久依赖有序；`add` 可带结构化 `command` 作客观验收门，见 ADR 0030 修订） |
| `memory` | 持久化 key-value 记忆读写（**跨 session 共享**） |
| `ask_user` | 用户交互 |
| `confirm` | yes/no 确认对话 |
| `agent` | 子代理调用 |
| `reason` | 推理树管理（root-cause 分析：add/status/margin/list/trace，持久化到 `causal_tree.json`，**跨 session 检索 meta 节点**） |

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

## 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CODECODER_API_KEY` | — | LLM API key（必需） |
| `CODECODER_MODEL` | `gpt-4o` | 模型名称 |
| `CODECODER_API_BASE` | `https://api.openai.com/v1` | API 端点 |
| `CODECODER_MAX_TOKENS` | `8192` | 单次生成的 max_tokens。命中截断时按 `CODECODER_MAX_TOKENS_CEILING` 自适应上调（ADR 0038） |
| `CODECODER_MAX_TOKENS_CEILING` | `32768` | 截断自适应上调的封顶值：命中 `StopReason::Length` 时该 turn 有效 max_tokens 翻倍直至此上限（ADR 0038） |
| `CODECODER_TEMPERATURE` | `0.7` | 温度参数 |
| `CODECODER_ROOT` | 当前目录 | 项目根目录 |
| `CODECODER_DAEMON` | — | 设置后以 daemon 模式启动长驻服务（client-server 架构，无 TUI；见 ADR 0032） |
| `CODECODER_BG_TASK` | — | 设置后以 Background Agent headless 模式跑完该 task 即退出（无 daemon、无用户在场；权限走 `codecoder.json` 预授权，见 ADR 0026） |
| `CODECODER_BG_WORKGRAPH` | — | 设为 `1` 时以 headless **workgraph 模式**跑（无显式 task，逐里程碑推进 workgraph；产出 mission_state→退出码 0/2/3/4/5=EmptyGraph（空图，需先 seed），见 ADR 0033 修订） |
| `CODECODER_BG_MAX_AUTO` | `10` | BG workgraph 模式下，单次调用最多推进的里程碑数（ADR 0030；默认 3→10 见 ADR 0039，熔断 `bg_circuit_k` 仍兜底） |
| `CODECODER_BG_CIRCUIT_K` | `2` | BG 连续失败里程碑的熔断阈值：连续 K 个 fail 即停止（ADR 0030） |
| `CODECODER_BG_MILESTONE_TOOL_CAP` | `8` | BG 单里程碑 turn 的工具迭代上限（< 全局 12，防固着；ADR 0030） |
| `CODECODER_BG_MAX_FIX_ATTEMPTS` | `3` | headless workgraph 中单个 milestone 验收 `needs_fix` 后最多自动重试次数（0 = 禁用自恢复；预算耗尽才落 `StuckNeedsFix`，退出码 2；ADR 0026/0033 修订） |
| `CODECODER_SUPERVISOR_CRASH_BUDGET` | `3` | Persistent Capability 跨重启崩溃预算：累计崩溃达此值后 daemon 重启不再 spawn（会话内仍守 ADR 0021 不自动重启；manifest 变更自动重置；ADR 0034） |
| `CODECODER_DEFAULT_TRUST` | `never` | 无用户在场（headless）时的默认信任策略：`never`/`always`/`once`（见 ADR 0028）。未信任则不加载 `AGENTS.md`/skills/capabilities 与 `codecoder.json` allowlist |
| `CODECODER_TRUST_FILE` | `~/.codecoder/trust.json` | 全局信任决策存储路径（就近祖先匹配，见 ADR 0028） |
| `CODECODER_MAX_TOOL_OUTPUT` | `262144` | `read_file` / `run_command` 单次输出字节上限，超长截断带 marker（ADR 0037） |
| `CODECODER_NOOP_NUDGE_THRESHOLD` | `3` | 单 turn 连续多少「纯探索」步（read_file/glob/grep/diff）后注入一次 steering nudge，推动动手或声明阻塞（0 = 禁用；每 turn 至多一次；迭代 4，ADR 0029 修订） |
| `CODECODER_COMPACTION_TIER2` | `true` | 是否启用 tier-2 compaction（LLM 摘要），设为 `false` 则仅执行 tier-1 压缩（ADR 0023） |
| `CODECODER_PROVIDER_RETRY_MAX` | `3` | LLM provider 调用重试次数（0 = 禁用重试） |
| `CODECODER_PROVIDER_RETRY_INITIAL_MS` | `1000` | LLM provider 重试初始等待毫秒数（指数退避基准） |
| `CODECODER_FALLBACK_API_BASE` | — | 可选，主 provider 不可用时回退的 API 端点 |
| `CODECODER_FALLBACK_MODEL` | — | 可选，回退时使用的模型名称 |
| `CODECODER_WG_TICK_SECS` | `30` | daemon 后台 workgraph 自动推进间隔秒数 |
| `CODECODER_DAEMON_AUTO_RESTART` | `false` | daemon 崩溃后自动重启并恢复 session（stamp 文件追踪 workgraph 进度，最多 5 次重启尝试） |
| `CODECODER_MAX_SESSIONS` | `100` | `sessions/` 目录最大文件数，超限时删除最旧的。`0` = 不限制 |
| `CODECODER_MAX_LEDGER_LINES` | `10000` | `bg_ledger.jsonl` 最大行数，超限时截断。`0` = 不限制 |
| `CODECODER_AUTOTASK_INTERVAL_SECS` | `300` | 自动任务发现轮询间隔秒数（0 = 禁用） |
| `CODECODER_AUTOTASK_SOURCE` | `github_issues` | 自动任务来源（当前仅支持 `github_issues`） |
| `CODECODER_SUPERVISOR_TICK_SECS` | `1` | daemon Persistent 服务监督线程检查间隔秒数 |
| `CODECODER_ALERT_WEBHOOK` | — | BG 失败告警 webhook URL（非空时 BG 非 0 退出即 POST JSON 至该 URL） |
| `CODECODER_ALERT_ON_FAILURE_ONLY` | `true` | 设为 `false` 时，BG 每次退出（含成功）都触发告警；默认仅非 0 退出时告警 |
| `CODECODER_ONDEMAND_REAPER_SECS` | `5` | OnDemand capability 自动回收延迟秒数 |
| `GITHUB_TOKEN` | — | GitHub API token（提升 rate limit） |

> **`.ccd.env` 自动加载**：`ccd`/`cc`/headless BG 启动时自动加载项目根（`CODECODER_ROOT` 或 CWD）的 `.ccd.env`（`KEY=VALUE` 每行一条，支持 `#` 注释与引号值；已 gitignore）。**只注入一个安全调参白名单**（`MODEL`、`MAX_TOKENS`、`MAX_TOKENS_CEILING`、`TEMPERATURE`、`MAX_TOOL_OUTPUT`、`BG_MAX_AUTO`、`BG_CIRCUIT_K`、`BG_MILESTONE_TOOL_CAP`、`BG_MAX_FIX_ATTEMPTS`、`NOOP_NUDGE_THRESHOLD`、`SUPERVISOR_CRASH_BUDGET`，均带 `CODECODER_` 前缀），且**不覆盖**已设置的进程 env。密钥/端点（`CODECODER_API_KEY`/`CODECODER_API_BASE`/`GITHUB_TOKEN`）、trust 门（`CODECODER_DEFAULT_TRUST`）与 loader/shell 变量（`LD_*`/`PATH` 等）**一律拒绝注入**并 stderr 告警——`.ccd.env` 是仓库本地文件、可能不可信，这些必须来自你的真实 shell。

> **`.ccd.bg.ndjson` 实时 BG 日志**：headless BG（`CODECODER_BG_TASK`/`CODECODER_BG_WORKGRAPH`）运行期间，`BgObserver` 把每个 BG 事件同时写 stderr 与项目根的 `<root>/.ccd.bg.ndjson`（每行一条 JSON，一次 truncate 开轮 + 逐事件 append 累积整条流；已 gitignore），可 `tail -f` 实时观察进展（ADR 0039）。

## Background Agent 账本与告警（ADR 0033）

每次 BG 调用（独立进程、跑完即退）追加一条 JSONL 记录到 `CODECODER_ROOT/bg_ledger.jsonl`（ts / mission_state / 每 milestone 的 subgoal 结论 / counts）。`mission_state` 映射成进程退出码，外部调度器据此告警：

| mission_state | exit code |
|---|---|
| `CompletedAllReady` / `Running`（正常） | 0 |
| `BlockedAt(_)`（硬依赖断裂） | 2 |
| `StuckNeedsFix(_)`（needs_fix 重试预算耗尽，见 `CODECODER_BG_MAX_FIX_ATTEMPTS`） | 2 |
| `CircuitBreaker`（连续失败熔断） | 3 |
| `Error(_)`（turn/provider 出错） | 4 |
| `EmptyGraph`（空图，需先 seed） | 5 |
| SIGINT 取消 | 0（操作者主动，非故障） |

> **退出码可达性**：`CODECODER_BG_TASK=<显式 task>` 产 `Running`→0（provider 错误经 `AgentLoop.last_error` 现也→`Error`→4）；`CODECODER_BG_WORKGRAPH=1` 经 workgraph 分支可达 `CompletedAllReady`/`BlockedAt(2)`/`StuckNeedsFix(2)`/`CircuitBreaker(3)`/`EmptyGraph(5)`（空图，需先 seed）（ADR 0033 修订）。

查账本（直读文件，不经 daemon——BG 运行时 daemon 常不在场）：

```bash
cc ledger                 # 最近 10 次（单行摘要：ts / state / counts）
cc ledger --last 50       # 最近 N 次
cc ledger --failed        # 仅需关注（state ≠ CompletedAllReady）
cc ledger --detail        # 最近一次的完整 subgoals 明细
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
CODECODER_DAEMON=1 cargo run  # 启动 ccd daemon
cargo run --bin cc            # 启动 cc 客户端
```

## 架构

参考 `docs/` 目录下的 ADR 和设计文档。
