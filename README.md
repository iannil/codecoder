# CodeCoder

> 自主 AI agent 系统 — 事件驱动，文件系统即自我

基于 AI 大模型、用 Rust 编写的高度自主 agent。它能读/写文件、搜索网络、搜索 GitHub、逆向 API、在沙箱中运行代码，还能用工具来扩展自身能力。

## 快速开始

```bash
# 1. 设置 API key
export CODECODER_API_KEY=sk-your-key-here
export CODECODER_MODEL=gpt-4o  # 默认 gpt-4o

# 2. 启动
cargo run

# 3. 在 REPL 中试试
cc> 列出当前目录的文件
cc> 搜索 GitHub 上的 Rust Web 框架
cc> /help      # 查看所有命令
cc> /exit      # 退出
```

## 无需 API key 运行

不设置 API key 时使用 StubClient（模拟 LLM 响应），可用于测试：

```bash
cargo run
```

## 内置工具（25 个）

> **Tool / Skill / Capability 三分**:**Tool** 是编译进二进制的原生原语(下表);**Skill** 是 agent 自撰的 `.md` 程序性知识(改变怎么想);**Capability** 是 agent 自撰的可执行产物(长出新手脚)。**Skill** 另有一个草稿前身 **Prompt**(`prompts/`,经 `promote_prompt` 转正,见 `docs/adr/0025`)。详见 `CONTEXT.md` 与 `docs/adr/0020`–`0022`、`0025`。

| 工具 | 功能 |
|------|------|
| `read_file` | 读取文件内容 |
| `write_file` | 写入文件（自动创建父目录） |
| `run_command` | 执行 shell 命令 |
| `list_directory` | 列出目录内容 |
| `search_web` | 抓取 URL 内容 |
| `search_github` | 搜索 GitHub 仓库 (`repos:`) 或代码 (`code:`) |
| `reverse_api` | 抓取文档页面，提取 API endpoint 签名 |
| `generate_skill` | 撰写 Skill(`.md` 程序性知识)到 `skills/` |
| `generate_prompt` | 撰写 Prompt 草稿(Skill 草稿态)到 `prompts/` |
| `promote_prompt` | 把 `prompts/` 草稿转正为 `skills/` 里的 Skill(删草稿,撞名报错) |
| `generate_capability` | 撰写 Capability(自撰可执行产物)到 `capabilities/` |
| `use_skill` | 激活某个 Skill,把全文注入后续 context |
| `run_capability` | 在声明的 Environment/Lifecycle 中执行某个 Capability |
| `glob` | 文件 glob 搜索（支持 ** 递归） |
| `grep` | 文本搜索 + AST 查询（tree-sitter） |
| `diff` | 文件差异比较 |
| `edit_file` | 精确文本替换编辑 |
| `commit` | Git 提交 |
| `review` | 代码审查 |
| `plan` | 任务计划 |
| `todo` | Todo 管理（list/create/update/complete/delete） |
| `memory` | 持久化 key-value 记忆读写 |
| `ask_user` | 用户交互 |
| `confirm` | yes/no 确认对话 |
| `agent` | 子代理调用 |

## 文件系统即自我

```
project/
├── AGENTS.md     ← 系统身份声明 → 自动注入 LLM system prompt
├── CONTEXT.md    ← 项目术语表
├── skills/       ← Skill:.md 程序性知识,Registry 扫描入目录
├── capabilities/ ← Capability:agent 自撰的可执行产物 + manifest
├── memory/       ← 系统持久化的 key-value 记忆
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
| `CODECODER_MAX_TOKENS` | `4096` | 最大 token 数 |
| `CODECODER_TEMPERATURE` | `0.7` | 温度参数 |
| `CODECODER_ROOT` | 当前目录 | 项目根目录 |
| `CODECODER_BG_TASK` | — | 设置后以 Background Agent headless 模式跑完该 task 即退出（无 TUI、无用户在场；权限走 `codecoder.json` 预授权，见 ADR 0026） |
| `CODECODER_DEFAULT_TRUST` | `never` | 无用户在场（headless）时的默认信任策略：`never`/`always`/`once`（见 ADR 0028）。未信任则不加载 `AGENTS.md`/skills/capabilities 与 `codecoder.json` allowlist |
| `CODECODER_TRUST_FILE` | `~/.codecoder/trust.json` | 全局信任决策存储路径（就近祖先匹配，见 ADR 0028） |
| `GITHUB_TOKEN` | — | GitHub API token（提升 rate limit） |

## 开发

```bash
cargo build      # 编译
cargo test       # 运行 128 个测试（124 通过 + 4 个 #[ignore]：2 Docker e2e + L2 pty + L3 LLM 冒烟）
                 # tests/ 下为黑盒行为验证分层（L1 默认；L2/L3 门控，见 docs/testing/behavioral-validation.md）
cargo run        # 启动 TUI / REPL
```

## 架构

参考 `docs/` 目录下的 ADR 和设计文档。
