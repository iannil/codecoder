---
name: engineer-qa
description: >
  AI 测试验收引擎 — 功能开发完成后自动触发的测试门禁。执行测试金字塔全生命周期验收：
  单元测试 + 本轮变更(diff)分支覆盖率 ≥90% 硬门禁（全局 ratchet 不回退）、集成测试覆盖
  实体 CRUD 与错误路径、用 agent-browser 驱动关键用户链路 E2E。是 engineer* 系列
  测试/覆盖率/E2E 门禁的单一真源；无 UI 项目优雅降级为黑盒验收。
  TRIGGERS: 用户说"测试验收""跑一下测试""覆盖率够不够""E2E""端到端测试""验收测试"
  "test acceptance""run the tests""coverage""e2e""是不是测够了"。
  ALSO TRIGGER: 功能/里程碑开发完成后（管道自动接入），在提交固化前执行测试门禁。
compatibility: "bash, read, write, agent"
---

# engineer-qa — AI 测试验收引擎 / AI Test Acceptance Engine

> **来源声明**: 本 skill 的方法论来源于《基于实现规划的 AI 辅助编程实战》。更多内容请访问 [zhurongshuo.com]。
>
> **Source**: The methodology of this skill originates from "AI-Assisted Programming Practice Based on Implementation Planning".

## 🎯 核心理念 / Core Philosophy

"能跑通" ≠ "验收通过"。验收通过 = **测试金字塔全层绿灯 + 本轮分支覆盖率 ≥90% +
全局覆盖率不回退 + 关键用户链路 E2E 通过**。无验证不固化——测试必须真跑、真过、真达标。

## 🚦 触发条件 / When to Trigger

**中文触发：**
- "测试验收"、"跑一下测试"、"覆盖率够不够"、"是不是测够了"
- "E2E"、"端到端测试"、"验收测试"

**English triggers:**
- "test acceptance"、"run the tests"、"coverage"、"e2e"

**管道自动接入 / Pipeline auto-hook:**
功能或里程碑开发完成后，由 engineer* 管道在提交固化前自动调用本 skill 执行测试门禁——
不需要用户显式请求。When a feature or milestone completes, the engineer* pipeline invokes
this skill automatically as the test gate before any commit is finalized.

## ⚙️ 模式选择 / Mode Selection

通过 `--mode` 参数控制自动确认程度（默认 normal）：

| 模式 | 行为 |
|:----:|------|
| normal | 出验收报告，等待用户决定下一步 |
| auto | 自动执行三态决策：PASS→继续，NEEDS_FIX→升维修一次复验，REBUILD→git reset 重建(≤2) |
| silent | 全自动静默，仅 🔴 输出，报告落盘 .agents/qa-latest.md |

## 📋 工作前提 / Prerequisites

1. **CONTEXT.md 可读** — 从中读取测试策略、验收标准与关键用户链路；缺失时从对话历史推断并在报告中标注。
2. **变更范围明确** — 通过 `git diff` 确定本轮变更文件集，作为 diff 覆盖率门禁的分母。
3. **工具链探测** — 探测项目的测试与覆盖率工具链（Jest/Vitest/pytest/go test 等），确认能产出分支覆盖数据。

## 🔍 四阶段工作流 / Four-Stage QA Lifecycle

### ① 静态盘点 / Inventory
读 CONTEXT.md（测试策略、验收标准、用户链路、词汇表）；探测项目类型(API/CLI/Web/Library)；
探测测试与覆盖率工具链（见 `references/coverage-tools.md`）；确定本轮变更范围（`git diff`）。

### ② 单元层 / Unit
运行单元测试（全部通过）；计算**本轮变更(diff)分支覆盖率 ≥ 90% 硬门禁**；
每核心函数 1 happy + 2 edge（空输入/异常参数/极限值）。覆盖率不足 → NEEDS_FIX，报告未覆盖分支 `file:line`。

### ③ 集成层 / Integration
运行集成测试（全部通过）；业务实体 **CRUD 全链路 + 错误路径**（400/404/500 或等价异常）全覆盖。

### ④ E2E 层 / End-to-End（功能/项目完成后负载一次）
用 **agent-browser** 驱动关键用户链路（登录→核心操作→登出 / 创建→读取→更新→删除），
截图取证存 `.agents/qa-e2e/`。详见 `references/e2e-playbook.md`。
**无 UI 项目**：优雅**降级**——API→端点黑盒 E2E，CLI→子命令 E2E，Library→跳过并在报告标注。

## 📊 覆盖率门禁 / Coverage Gate
- 本轮硬门禁：diff 分支覆盖率 `< 90%` → NEEDS_FIX。
- 全局 ratchet：读写 `.agents/qa-baseline.json`；全局覆盖率低于基线 → NEEDS_FIX；达标则更新基线（只升不降）。
- 首次无基线：建立基线，仅 diff 门禁。
- 工具不支持分支覆盖：降级为行覆盖 + 报告告警。

## 🧭 三态决策 / Branch Decision

| 结论 | 条件 | normal | auto / silent |
|:----:|------|--------|---------------|
| ✅ PASS | 四层全绿 + diff 分支≥90% + 全局不回退 | 出报告建议提交 | 继续 |
| ⚠️ NEEDS_FIX | 测试失败/覆盖不足/全局回退/E2E 单链路失败 | 出报告等决策 | 升维修一次复验 |
| 🛑 REBUILD | 修一次后仍失败/根本性错误 | 建议重建 | git reset --hard 重建(≤2) |

## 📄 报告模板 / Report Template
见 `references/qa-report-template.md`，输出到 `.agents/qa-latest.md`。

## ⚠️ 边界情况 / Edge Cases

- **无 CONTEXT.md** — 从对话历史推断测试策略与验收标准，并在报告中标注"无蓝图验收"。
- **无 git 仓库** — 无法计算 diff 覆盖率，降级为全量覆盖率门禁并告警。
- **diff 为空** — 回退到 `HEAD~1..HEAD` 作为本轮变更范围。
- **无测试框架** — 直接判 NEEDS_FIX，报告中给出推荐的工具链与最小接入步骤。
- **E2E 起服务失败** — 降级为可执行的最深层验收（集成层），并在报告标注 E2E 未执行原因。
- **首次无 baseline** — 建立 `.agents/qa-baseline.json` 基线，本轮仅执行 diff 门禁。
- **E2E flake** — 单链路失败自动重试 1 次；复现失败才计入 NEEDS_FIX。
- **用户要求跳过测试** — 提醒"无验证不固化"，如坚持跳过则在报告与提交信息中显式标注未验收。
