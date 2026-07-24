# Pebble —— 迷你语言 + 常驻内核（用 codecoder 自主建成 + 不可替代性证明）

- **日期**：2026-07-24
- **状态**：设计已批准，待写 implementation plan
- **类型**：元实验（demo 项目 + 证明协议）
- **编排者**：Claude Code 本会话（外部编排）；**被测系统**：codecoder（独立进程）

---

## 0. 目标与非目标

**目标**：设计一个与 codecoder 无关、有真难度的项目，**用 codecoder 的内置能力自主建成**，并**诚实地量化"实现在多大程度上依赖 codecoder 框架而非模型自身"**——即：换成 Claude Code 等产品会缺什么。

要证明的四条差异线（用户确认全要）：
1. 无人值守自主性（headless workgraph 逐里程碑 + `needs_fix` 预算内自恢复）
2. 自我进化（agent 自撰可复用的 Skill + Capability，并在后续里程碑复用）
3. 常驻服务韧性（Persistent Capability 由 daemon 监管，跨重启保留 supervisor 状态）
4. 跨会话记忆（`memory/` 持久化事实、跨 session 召回）

**非目标**：
- 不冒领"codecoder 让代码更好"——parser/闭包等代码质量归因于模型，两边一样。
- 不做语言的花哨特性（无模块系统、无静态类型、无宏）。
- 不上分布式/多机；内核先走单进程 + stdin/stdout 行 JSON 协议。

---

## 1. Demo 项目：Pebble

一门动态类型小语言（文件后缀 `.peb`），**用 Rust 实现**，最终以**常驻内核 daemon** 形式对外服务。难度来自算法本身（Pratt 解析、闭包环境链、无 panic 错误传播、跨请求状态），有明确对错、可用 golden 测试验收。

*名字/细节可调；YAGNI：协议先 stdin/stdout 行 JSON，不上 Unix socket。*

### 1.1 里程碑切分（约 12 个，供 workgraph 播种）

| # | 里程碑 | 硬点 | 验收 |
|---|--------|------|------|
| M1 | Lexer：数字(int/float)/字符串(转义)/标识符/关键字/运算符/`#`注释/行号 | — | 单元测试：token 流正确、行号正确 |
| M2 | **Pratt 优先级解析器**：`\|\| && == != < <= > >= + - * / %`、一元 `- !`、分组 `( )` | 优先级+结合性 | 单元测试：AST 结构、结合性用例 |
| M3 | 求值核心 + 值（number/string/bool/nil）、算术/逻辑/比较、真值性、**带行号的运行时错误（绝不 panic）** | 无 panic 错误传播 | conformance：表达式求值、错误带行号 |
| M4 | 变量与作用域：`let`、赋值、块作用域 `{ }`、环境链 | 环境链 | conformance：遮蔽/作用域用例 |
| M5 | 控制流：`if/else`、`while` | — | conformance：分支/循环用例 |
| M6 | **函数与闭包**：`fn`、一等函数、词法闭包捕获 env、递归、`return` | 闭包捕获环境 | conformance：fib 递归、闭包计数器 |
| M7 | 数据结构：列表 `[...]`、索引、`len/push/get` | — | conformance：列表操作 |
| M8 | 标准库内建：`print/len/type/str/num/range/math…` | — | conformance：内建用例 |
| M9 | 错误处理加固：全错误路径结构化、模糊输入不崩 | 健壮性 | 模糊/坏输入不 panic |
| M10 | **常驻内核 daemon**：长驻进程持有全局 env 跨 eval 请求；行分隔 JSON 协议（`eval`/`:reset`/`:vars`/`:ping`）；优雅关停 | 跨请求状态 + 协议 | 协议往返测试、状态保留 |
| M11 | **内核注册为 codecoder Persistent Capability**：manifest(Shell×Persistent)、`run_capability` 权限门、daemon 监管、崩溃/重启韧性 | 跨重启 supervisor | kill→respawn、supervisor_state 保留 |
| M12 | 一致性测试套件：golden `.peb`/`.expected`（fib 递归、闭包计数器、优先级、错误带行号、列表操作） | — | 全套 golden 绿 |

### 1.2 内核协议（M10）

行分隔 JSON（每行一条）：
- 请求：`{"op":"eval","code":"let x = 1 + 2"}` → 响应：`{"ok":true,"value":"3","stdout":""}`
- 控制：`{"op":"reset"}`（清空全局 env）、`{"op":"vars"}`（列出绑定）、`{"op":"ping"}`
- 错误：`{"ok":false,"error":"...","line":N}`
- 全局 env 跨 `eval` 请求保留，直到 `reset` 或进程退出。

---

## 2. 四条差异线如何被"逼出来"

- **无人值守自主性** ← 12 里程碑由 `CODECODER_BG_WORKGRAPH=1` headless runner 逐个推进；Rust 编译/borrow-checker 失败 → `needs_fix` → 预算内自动把失败原因注入重试（重试不计入 max_auto）。Rust 编译门恰好成为自恢复压力测试。
- **自我进化**（关键，两处）：
  - **Capability「conformance-runner」**：agent 在 M3 后自撰一个 Shell/OneShot 能力，跑 `tests/conformance/*.peb` 对拍 `*.expected`、不符退非零。此后每个里程碑（M4–M9）结尾用 `run_capability` **调用自己造的工具自我把关**。→ 一个被复用 ≥6 次、受权限门控、agent"拥有"的产物。
  - **Skill「add-a-language-feature」**：agent 走过 2–3 遍"token→parse→AST→eval→test"后，把套路蒸馏成 `.md` skill，M7–M9 用 `use_skill` 注入、按自己的流程干。
- **常驻服务韧性** ← 内核作为 Persistent Capability 由 daemon 监管；kill 后走 respawn / give_up，`supervisor_state.json` 跨重启保留 crash_count/gave_up/manifest mtime。
- **跨会话记忆** ← 语法决策（优先级表、关键字表、值语义、协议 schema/版本）写入 `memory/`，多次建造会话间保持一致。

**只播种身份 + 里程碑，不预写 skill/capability**——那是要证明的自我进化，必须由 agent 运行时自己长出来。

---

## 3. 执行协议（混合模式）

### Phase 0 —— Bootstrap（Claude Code 本会话做）
1. `cd ~/Code/codecoder && cargo build --release`（确保二进制最新）。
2. 新建 `~/Code/pebble/`，`git init`。
3. 拷 `target/release/codecoder` 与 `target/release/cc` 进 `~/Code/pebble/`（按需求"编译并复制二进制到目标项目文件夹下"）。
4. 写 `~/Code/pebble/codecoder.json`：预授权 allowlist —— `write_file`、`run_command`(cargo build/test/run)、`git commit`、以及 conformance-runner 的 `run_capability`。（遵守 memory `driving-codecoder-headless`：allowlist key 口径、trust、acceptance-as-command、max_tokens。）
5. 写 `~/Code/pebble/AGENTS.md`（身份："你在建造 Pebble……"）+ `CONTEXT.md`（术语）。
6. 用 workgraph 播种 M1–M12 里程碑（依赖顺序）。

### Phase 1 —— Headless 磨制（codecoder 无人值守）
- `CODECODER_API_KEY=<真实key> CODECODER_BG_WORKGRAPH=1 CODECODER_BG_MAX_FIX_ATTEMPTS=3` 启动 headless runner，逐 `pending` 就绪里程碑推进。
- 我只监控 `mission_state` + 日志；仅在 `StuckNeedsFix`(退出码 2) / give_up 时介入。
- 纪律：**不并发向同一 daemon 发消息**（session 历史无并发写保护）；headless BG runner 是安全路径。

### Phase 2 —— 常驻内核演示
- 注册内核 Persistent Capability，daemon 下起内核。
- 做 kill→respawn 演示，抓 `supervisor_state.json` 重启前后对比。

### Phase 3 —— 取证 + 报告
- 收集 5 项证据产物，写 `~/Code/pebble/PROOF.md`。

---

## 4. 证明设计 —— 诚实的"模型 vs 框架"拆分

**核心论点**：最强的诚实主张**不是**"codecoder 写的代码更好"（同一模型在 Claude Code 也能写出同样 parser），而是：**codecoder 把 Claude Code 甩给"人在环 + 外置 harness + 用户自己搭持久化"的那部分，内核化成一等原语**，于是同一个模型能**无人值守跑完 12 里程碑、用自己造的工具自我把关、并在重启后存活**——这三件事 Claude Code 没有人或定制外置脚本就做不到。

| 维度 | A. 模型可归因（CC 一样） | B. codecoder 可归因（框架 delta）＋证据＋CC 缺口 |
|---|---|---|
| 代码质量 | parser/闭包/求值正确性——LLM 两边都能写 | —（**不冒领**） |
| 无人值守完成 | | `mission_state.json`：N 里程碑自动推进、M 次自恢复重试、磨制期 0 人工 turn。**CC 缺口**：每 turn 需人或自建外置 harness；无内置 workgraph/needs_fix 重试 |
| 可复用能力 | | `capabilities/conformance-runner/` + `run_capability` 调用日志（≥6 次、权限门控）。**CC 缺口**：无一等、持久、可复用、受权限门控的 capability；只能每次临时 bash，agent 不"拥有"任何东西 |
| 常驻监管 | | `supervisor_state.json` + kill/respawn 日志跨重启保留。**CC 缺口**：无 daemon 托管服务生命周期/跨重启状态 |
| 自撰 Skill | | `skills/add-a-language-feature.md` + M7–M9 的 `use_skill`。**CC 缺口**：无 agent 运行时自撰并再激活的 skill 注册表（subagent/CLAUDE.md 是人写的，非运行时进化） |
| 跨会话记忆 | | `memory/` 事实跨 session 召回。**CC 缺口**：只重读文件，无内核召回原语 |

**A/B 小对照**：同一个"加一个语言特性"的循环，在 Claude Code 本会话也真跑一遍，展示它每次重新推导套路、无可自我把关的持久能力——形成具体反差，而非空口。

---

## 5. 测试与验收

- **语言**：conformance 套件全绿（fib 递归、闭包计数器、优先级、错误带行号、列表操作）。
- **内核**：协议往返测试、跨 eval 请求状态保留、优雅关停、被 kill 后经 supervisor 存活。
- **证明**：5 项证据产物齐备并映射到 CC 缺口；`PROOF.md` 完成。
- **Definition of Done**：`~/Code/pebble` 内 `cargo test` 全绿 + conformance-runner 绿 + 内核演示 transcript + 证据包 + `PROOF.md`。

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 无 `CODECODER_API_KEY` → StubClient 无法真正建造 | **前提已确认：有真实 key**；Phase 1 显式带 key 启动 |
| Headless Rust 失败循环 | `CODECODER_BG_MAX_FIX_ATTEMPTS=3` 预算封顶；`StuckNeedsFix` 我介入 |
| 并发写 session 历史竞争 | 单 daemon、串行；headless BG runner 路径 |
| 二进制过期 | Phase 0 先 `cargo build --release` 再拷 |
| 里程碑过大跑不动 | 每里程碑可独立 `cargo test`/conformance 验；过大再拆 |
| 过度归因于 codecoder | 第 4 节 A 列显式列出模型可归因项，不冒领 |

---

## 7. 相关

- 相邻实验：`2026-07-24-sentinel-design.md`（同类"codecoder 自主建成 + 不可替代性证明"，不同 demo 载体）。
- 契约参考：ADR 0021（Persistent 生命周期 / 会话内崩溃即 give_up）、0026（BG headless runner / 外置调度 / SIGINT）、0033（workgraph 逐里程碑 + needs_fix 自恢复）、0034（supervisor_state 跨重启）、0035（workgraph 并发写保护）。
- 纪律 memory：`driving-codecoder-headless`。
