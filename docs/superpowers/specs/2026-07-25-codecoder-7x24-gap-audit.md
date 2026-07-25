# CodeCoder 7×24 高度自主开发差距审计报告

> 实验依据: 2026-07-25 strategic-control 项目两轮 BG_WORKGRAPH 自主构建
> 实验数据: BgObserver 396 条事件、workgraph 11 里程碑、5657 行产出
> 审计日期: 2026-07-25

---

## 概要

CodeCoder 已证明**能按预设里程碑图自主完成完整的前端项目构建**（11/11 milestone pass，build clean，test pass）。但要实现 7×24 无人值守的"给个空白目录就自己做完"的能力，还有 4 个 P0 + 5 个 P1 差距。

---

## P0 — 必须修复（阻塞 7×24 的基础设施）

### P0-1: EmptyGraph 不自建里程碑

**模块**: `background.rs` / `workgraph.rs`

**现象**: 空 `workgraph.json` 时 BG_WORKGRAPH 立即退出码 5 (EmptyGraph)。不读 AGENTS.md，不解析任务，不创建任何里程碑。

**实验证据**: R1 首次启动 → 0 tools, 0 denied → exit 0（当时还没 EmptyGraph 码）。必须手动写 workgraph.json。

**成功标准**: 当 workgraph 为空时，codecoder 应:
1. 读取 AGENTS.md 理解使命
2. 自主拆解为 3-10 个里程碑（含依赖关系）
3. 写入 workgraph.json
4. 开始推进第一个里程碑

### P0-2: plan 工具 headless 模式下不可用

**模块**: `src/tool/dev.rs` — `PlanTool::run()` 

**现象**: `plan` 工具返回 `Permission::None`（只读），但 `AgentLoop` 处理 plan 时通过 `handle_ask_user` 要求用户确认 → headless 模式下被拒绝。

**实验证据**: R2 BgObserver 中 2 次 `plan: denied: 'plan' requires a user, none present (headless)`

**成功标准**: headless 模式下：
1. `plan` 工具直接返回规划结果，不需要用户确认
2. 或：BG_WORKGRAPH 在 milestone turn 中自动跳过 plan 类工具

### P0-3: write_file 因 max_tokens 截断

**模块**: `agent.rs` — tool call 参数提取

**现象**: 模型在同一 turn 中先做 reasoning 再输出工具参数。当 reasoning 过长时，write_file 的 `content` 参数被截断 → 文件写不完整。

**实验证据**: R2 BgObserver 中 **5 次** `write_file: tool call truncated: the response hit max_tokens before the arguments finished`

**成功标准**:
1. 自动检测截断 → 用剩余 budget 续写缺失部分
2. 或：当文件超过阈值（如 200 行）时自动分多次 write_file
3. 或：让 write_file 支持 append 模式，分批写入

### P0-4: 复合命令权限过细 (ADR 0036)

**模块**: `src/tool/builtin.rs` — `RunCommand::key_for()`

**现象**: `is_compound()` 检测到 `2>&1`/`|`/`&&` 等符号后，key 变成完整命令串。`run_command:npm` 不能覆盖 `npm run build 2>&1`。

**实验证据**: R1 14 次拒绝，R2 仍有 6 次（包括 `npm run build 2>&1`、`find ... | sort` 等开发常用命令）

**成功标准**:
1. 为 headless 模式提供更宽松的权限模式（如 `AllRunCommandsAllowed`）
2. 或：支持 allowlist 通配符前缀（`run_command:npm*`）
3. 安全考量：宽松模式仅在 `CODECODER_DEFAULT_TRUST=always` 时生效

---

## P1 — 应该修复（显著影响效率和可靠性）

### P1-1: 跨 milestone bug 修复延迟

**模块**: `background.rs` — `build_repair_prompt()`

**现象**: 当 milestone A 引入 bug → build fail → A needs_fix。但 A 的修复 prompt 只注入"本轮 build 失败原因"，不追溯"这个文件是哪个 milestone 写的"。下游 milestone (B/C) 继续触发同一 bug → 浪费 3 轮重试机会才修好。

**实验证据**: M8 引入 `Loop.tsx:145 Modal` 标签未闭合 → M9/M10 继续 build fail → M8 在第 4 次才修复。

**成功标准**: 修复 prompt 应包含：
1. 失败文件的归属里程碑（`workgraph.json` 的 `touched` 字段）
2. 跨 milestone 的搜索策略（"这个语法错误在所有 .tsx 文件中是否存在"）

### P1-2: bg_max_auto 默认 10 仍非 0

**模块**: `src/config.rs`

**现象**: ADR 0039 已改为默认 10，但 11+ milestone 的项目需手动设置。

**成功标准**:
1. 默认 0（不限），但增加安全护栏：`bg_circuit_k` 默认 3
2. 或：根据 workgraph 节点数自动计算 `max_auto`

### P1-3: 无 checkpoint/resume

**模块**: 新模块 或 `compaction.rs` 增强

**现象**: BG_WORKGRAPH 一旦退出，session 上下文全部丢失。下次启动时 codecoder 通过读文件重新理解项目，浪费 tokens。

**实验证据**: R1→R2 之间约 30% 的工具调用用于重新探索项目结构（`list_directory` + `read_file` × 40+ 次）

**成功标准**:
1. 每完成 N 个里程碑，持久化 session checkpoint
2. Resume 时从最后一个 checkpoint 恢复上下文
3. `memory/` 中累积架构决策记忆

### P1-4: Token 消耗不可见

**模块**: `src/bg_observer.rs` / `src/provider/`

**现象**: BgObserver 有事件流但无 token 消耗指标。

**实验证据**: R2 跑完不知道总 token 消耗、每轮 LLM 调用成本、哪个 milestone 最贵

**成功标准**: `.ccd.bg.ndjson` 增加：
1. 每轮 LLM 调用的 `prompt_tokens` / `completion_tokens`
2. 累计 token 统计
3. turn 耗时

### P1-5: 验证深度仅到 build

**模块**: `src/bg_gate.rs`

**现象**: 命令门只跑 `npm run build && npm test`，不做运行时验证。

**实验证据**: Dashboard 页面可以通过 build 和 test，但从未启动 dev server 验证过它在浏览器中渲染正确。

**成功标准**: 增加轻量级运行时验证：
1. 启动 dev server → curl 首页 → 确认 HTTP 200
2. 或：`npx vite build --mode production` 之后检查产物体积

---

## P2 — 补充项（7×24 锦上添花）

| # | 问题 | 模块 | 说明 |
|---|------|------|------|
| P2-1 | 动态 milestone 粒度 | `workgraph.rs` | 过大的 milestone 自动拆分，过小的自动合并 |
| P2-2 | 熔断降级 | `background.rs` | exit 3 前先试降级（跳过本 milestone，继续下一个） |
| P2-3 | 进程级 supervisor | 外部 | systemd / supervisord 集成，自动拉起 |
| P2-4 | Rate limit backoff | `provider/` | HTTP 429 时指数退避，而非立即失败 |
| P2-5 | 日志自动轮转 | `bg_observer.rs` | `.ccd.bg.ndjson` 按大小/时间切分 |
| P2-6 | 远程告警 | 新模块 | milestone 连续失败 → webhook / 邮件通知 |
| P2-7 | 运行时验证增强 | `bg_gate.rs` | dev server curl + Puppeteer/Playwright 快照 |

---

## 实验已验证的完善功能

以下功能经本次实验确认良好，无需改动：

- ✅ **Review gate** (ADR 0039): 独立评审子 agent 覆盖自报 VERDICT
- ✅ **BgObserver** (ADR 0039): `.ccd.bg.ndjson` 实时事件流
- ✅ **自恢复重试**: `build_repair_prompt` 驱动 needs_fix 重试（`bg_max_fix_attempts=3`）
- ✅ **bg_max_auto=10**: 足够支撑中小型项目
- ✅ **EmptyGraph 退出码 5**: 区分空图 vs 完成 vs 卡住
- ✅ **Command gate**: `npm run build` / `npm test` 验证有效
- ✅ **with_lock 并发保护** (ADR 0035): workgraph 写操作无竞争
- ✅ **Compaction** (ADR 0023): tier-1 + tier-2 上下文裁剪

---

## 附录: 实验数据

```
R1 (未修 review gate): 99 tools, 14 denied, 0 gate pass → M1 stuck needs_fix
R2 (ADR 0039 修复后):   ~110 tools, 6 denied, 11/11 gate pass → CompletedAllReady

BgObserver: 396 events
  read_file:    65  (16.4%)
  list_directory: 42 (10.6%)
  write_file:   35  (8.8%)
  run_command:  12  (3.0%)
  edit_file:    12  (3.0%)
  milestone:     7  (1.8%)
  glob:          6  (1.5%)
  commit:        1  (0.2%)
  diff:          1  (0.2%)

Source code: 56 files, 5657 lines (from 22 files, 1671 lines)
Build:       npm run build → exit 0 (clean)
Test:        npm test → 2/2 passed
```
