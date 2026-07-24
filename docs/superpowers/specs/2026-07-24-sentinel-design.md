# Sentinel — 用 codecoder 自主建成、并证明其不可替代性

- 日期：2026-07-24
- 状态：设计已获批（待 spec 复核）
- 目标项目：`~/Code/sentinel`（与 codecoder 无关；Python 可观测 / 异常检测平台）
- 编排器：本会话（Claude Code），负责搭环境、投放二进制、播种蓝图、驱动 codecoder、埋点证据

## 1. 目的与两层结构

本任务的产物有**两层**，全程不可混淆：

- **产品层 = Sentinel**：被 codecoder *建出来* 的东西。
- **编排层 = 本会话 + 证明**：搭环境、编译并投放 codecoder 二进制、给 codecoder 播种身份/蓝图/workgraph、驱动它无人值守建 Sentinel、全程埋点写 `PROOF.md`。

**codecoder = 开发者；本会话 = 操作员 + 记录员；Sentinel = 产物。**

证明的核心命题：*Sentinel 的每一块都能追溯到某个 codecoder 独有动作；换成 Claude Code 会在明确的点卡住。* 归因不成立的部分，如实归给"模型自身能力"。

## 2. Sentinel 产品规格（codecoder 要建的东西）

自包含、纯 Python 标准库（`http.server`/`sqlite3`/`json`/`threading`），**无外部 key、无第三方依赖** → 演示可复现，"是不是 codecoder 干的"信号干净。

四个进程角色：

| 角色 | 形态 | codecoder 能力映射 |
|---|---|---|
| **target** | 合成业务应用；HTTP 暴露 `/metrics`（计数/延迟/心跳），带**故障注入**开关（`POST /fault`） | Persistent Capability（shell + persistent，daemon 托管） |
| **collector** | 周期拉 target `/metrics` → 落 SQLite 时序表；每周期扫描 `detectors/` 目录**热加载**并执行检测器 | Persistent Capability |
| **detector(s)** | 单条异常规则（心跳缺失 / 阈值越界 / 突变 / 多信号）；读近窗数据 → 产 alert 行 | 运行时 `generate_capability` **自撰**的 on_demand Capability，collector 扫描目录加载 |
| **dashboard** | 只读看板：渲染最新指标 + 告警流（轮询 SQLite 出 HTML/文本） | on_demand Capability |

数据流：`target ──HTTP──▶ collector ──▶ SQLite ──▶ detectors(周期) ──▶ alerts 表 ──▶ dashboard`

存储：单一 `sentinel.db`（SQLite）。表：`metrics(ts, name, value)`、`alerts(ts, detector, severity, msg)`。

## 3. Workgraph（codecoder 逐里程碑自主建）

播种为 `~/Code/sentinel/workgraph.json`（`schema_version:1`）。每个里程碑带 `deps`、`acceptance`（人读）、`command`（客观验收门，独占裸命令；ADR 0030）。验收门统一用标准库 `unittest`（避免第三方依赖）：`python3 -m unittest -q tests.test_mN`。

| # | 里程碑 | deps | command（验收门） | 主要能力 |
|---|---|---|---|---|
| M1 | 协议与骨架：`/metrics` JSON 契约 + `sentinel.db` schema + 目录约定 | — | `python3 -m unittest -q tests.test_m1` | write_file |
| M2 | target 常驻服务：合成负载 + 故障注入 | 1 | `python3 -m unittest -q tests.test_m2` | Persistent Capability |
| M3 | collector 常驻服务：拉取 + 落库 + 检测器热加载目录 | 1,2 | `python3 -m unittest -q tests.test_m3` | Persistent Capability |
| M4 | 首个检测器（心跳缺失），由 generate_capability 自撰 | 3 | `python3 -m unittest -q tests.test_m4` | 自我进化 |
| M5 | 检测器族 + 告警（阈值/突变/多信号） | 4 | `python3 -m unittest -q tests.test_m5` | 自我进化 + 子 agent 并行设计 |
| M6 | dashboard 只读看板 + 告警流 | 3,5 | `python3 -m unittest -q tests.test_m6` | Capability |
| M7 | 韧性验收：杀 target/collector→supervisor 恢复；重启 ccd→supervisor_state 存活 | 2,3,6 | `python3 -m unittest -q tests.test_m7` | 常驻托管 + 崩溃恢复 |

初始 `status`：全 `pending`（headless BG_WORKGRAPH 逐个推进就绪节点）。

## 4. 四大能力如何被"压满"

- **常驻服务托管**：target + collector 以 `lifecycle:persistent, environment:shell` 的 Capability 起，daemon 启动即 spawn 并 supervise。M7 主动杀进程验证自恢复 + 跨 `ccd` 重启 `supervisor_state.json`（gave_up/crash_count/manifest mtime）存活。
- **自我进化**：检测器一律 `generate_capability` 运行时自撰（on_demand），collector 热加载目录；"如何加一个检测器"用 `generate_skill` / `promote_prompt` 沉淀为常驻 Skill。
- **无人值守 workgraph**：M1–M7 用 `CODECODER_BG_WORKGRAPH=1` headless 跑；验收门（`command`）+ `needs_fix` 自动重试（`CODECODER_BG_MAX_FIX_ATTEMPTS`）+ 连续失败熔断（`CODECODER_BG_CIRCUIT_K`）。里程碑间我只读 `bg_ledger.jsonl`。
- **子 agent 编排**：M5 派 read-only sub-agent 并行侦察"哪些信号值得检测"、并行验证检测器产出。

## 5. 编排流程（本会话执行）

1. **编译**：`cargo build --release` → 得 `target/release/codecoder`、`target/release/cc`。
2. **投放**：`mkdir ~/Code/sentinel`；复制两个二进制到该目录；`git init`。
3. **播种身份/蓝图**：写 `AGENTS.md`（Sentinel 身份 + 纪律：纯 stdlib、小步写、别谎报测试）、`CONTEXT.md`（术语/数据契约/表结构）、`workgraph.json`（M1–M7）。
4. **预授权 + 调参**：
   - `codecoder.json`：`{"allowlist":[...]}` 预授权 headless 需要的工具键（见 §7 键格式）。
   - `.ccd.env`：`CODECODER_MODEL=...`、`CODECODER_MAX_TOKENS=8192`（安全白名单，自动加载）。
   - 真实 shell env：`CODECODER_API_KEY`、`CODECODER_DEFAULT_TRUST=always`、`CODECODER_ROOT=~/Code/sentinel`（**不能**走 `.ccd.env`）。
5. **驱动**：`cd ~/Code/sentinel && CODECODER_BG_WORKGRAPH=1 ./codecoder`，逐里程碑推进；每轮后读 `bg_ledger.jsonl` + 独立跑该里程碑 `unittest` 核验；`StuckNeedsFix` 时介入（重置 `pending` 或切交互式 `cc`）。
6. **收官演示**：起 `ccd` daemon → 托管 target/collector；`cc` 触发故障注入 → 自撰检测器捕获 → dashboard 显示告警；杀进程 + 重启 ccd 验证韧性。
7. **埋点归档**：全程写 `PROOF.md`（§6）。

## 6. 证明：`PROOF.md` + 对照分析

建项目**同时**埋点。每次 codecoder 独有动作追加一条：
`{ts, 能力类别, 动作, 产物路径, 证据(bg_ledger 行 / supervisor_state 快照 / capability manifest 路径)}`。

收官产出对照表，逐条写"换 Claude Code 卡在哪"：

| Sentinel 组件 | codecoder 怎么做 | Claude Code 的问题 |
|---|---|---|
| target/collector 常驻 | Persistent Capability + supervisor 跨重启 | 无法把长驻进程作为 agent 拥有的一等产物托管；会话结束进程即失管 |
| 运行时加检测器 | `generate_capability` + collector 热加载 | 只能写文件；无能力注册表/生命周期/权限闸门；活着的采集器不会"长出新手" |
| M1–M7 自主建 | BG_WORKGRAPH + 验收门/重试/熔断 | 无自主里程碑闭环；需人逐步 prompt |
| 崩溃恢复 | `supervisor_state.json` 持久化 | 无 supervisor 概念 |

**诚实条款**：纯算法/写代码部分（协议、SQL、检测逻辑本身）主要是*模型能力*，非 codecoder 独有——`PROOF.md` 必须如实标注，不夸大归因。

## 7. 目录布局 & 关键契约

```
~/Code/sentinel/
  codecoder  cc                 # 投放的二进制
  AGENTS.md  CONTEXT.md          # 播种的身份/蓝图
  codecoder.json  .ccd.env       # 预授权 allowlist + 调参
  workgraph.json                 # M1–M7
  capabilities/<name>/manifest.json + entry  # codecoder 自撰（target/collector/detectors/dashboard）
  skills/                        # codecoder 自撰程序性知识
  src/  tests/  sentinel.db      # Sentinel 代码 / 测试 / 存储
  PROOF.md  bg_ledger.jsonl  supervisor_state.json  # 证据
```

契约（源码核对得来）：
- **workgraph.json**：`{schema_version:1, nodes:[{id,title,acceptance,deps,status,command}]}`。
- **capability manifest**：`capabilities/<name>/manifest.json`：`{name,description,environment:shell|wasm|docker,lifecycle:one_shot|on_demand|persistent,entry,address?}`；daemon 启动时扫描并 spawn `persistent+shell` 条目。
- **codecoder.json**：`{"allowlist":["<key>"]}`。键格式：`write_file`/`edit_file`/`commit`/`generate_skill`/`generate_capability`；`run_command` 简单命令→`run_command:<head>`（如 `run_command:python3`），复合命令（含 `&&`/`|`/`2>&1`）按**整串**→`run_command:<整串>`（ADR 0036）；`run_capability` 按 `能力名+环境` keying。只读工具（search/grep/reason）是 `Permission::None`，无需授权。

## 8. 成功判据

1. Sentinel 端到端可跑：注入故障 → 自撰检测器捕获 → dashboard 显示告警。
2. M7 韧性验收通过：杀进程 supervisor 恢复；重启 ccd `supervisor_state.json` 存活。
3. 全部里程碑 `command` 门在**我独立执行**下通过（不采信 agent 自报）。
4. `PROOF.md` 每个组件都有 codecoder 归因 + Claude Code 对照，且诚实标注模型能力部分。

## 9. 风险与缓解（含真实 gotcha）

- **需真实 `CODECODER_API_KEY` 在 shell env**（headless 真跑必需；`.ccd.env` 拒绝注入 key）→ 落地前先确认可用。
- **trust 门**：`codecoder.json` allowlist 只有 root 被 trust 时才加载 → headless 设 `CODECODER_DEFAULT_TRUST=always`。
- **acceptance 会被当 shell 命令跑** → 用 `command` 字段放独占裸命令（`python3 -m unittest ...`），`acceptance` 只放人读说明。
- **max_tokens 默认 4096 → 大文件写被截断** → `.ccd.env` 设 `CODECODER_MAX_TOKENS=8192`，并令 agent 小步分模块写。
- **单 turn 12 工具上限 / 弱模型过度探索** → AGENTS.md 内联精确指令；里程碑粒度已切小。
- **切忌向同一常驻 daemon 并发发消息**（共享 session 历史 + 异步写 → 文件版本竞争）→ 串行 `cc`，等每 turn 完再发。
- **StuckNeedsFix（重试预算耗尽，退出码 2）** → 介入：改 `workgraph.json` 该节点回 `pending` 重跑，或切交互式 `cc`。
- **验证纪律**：弱模型会谎报测试通过 → 每里程碑我独立跑 `unittest`，别信 agent 的 "all pass"。

## 10. 非目标（YAGNI）

- 不做认证/多租户/分布式；单机单 `sentinel.db`。
- 不接真实外部数据源（合成 target 足以演示，且保持可复现）。
- 不做 Wasm/Docker Capability（shell 足够；Wasm 源码编译本就未实现）。
- 不改动 codecoder 本体（Sentinel 与 codecoder 无关）。
