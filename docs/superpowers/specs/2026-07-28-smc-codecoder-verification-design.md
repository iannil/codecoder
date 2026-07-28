# 战略管控系统 CodeCoder 自主构建验证实验 — 设计方案

> 实验目的：验证 codecoder 最新修复（ADR 0039 等合并后）在 headless BG_WORKGRAPH 模式下
> 自主从零构建 Vite + React + TypeScript 前端项目的完整能力。
>
> 对比基准：2026-07-25 实验（deepseek-v4-flash, 5/5 里程碑, 1 TS 错误, 测试全挂, 16% 覆盖）

---

## 一、项目概述

### 1.1 目标项目

战略管控系统（Strategic Management & Control System）前端 Demo，基于：
- Vite 5 + React 18 + TypeScript
- Ant Design 5 + @ant-design/icons
- Zustand（状态管理）
- React Router v6（createBrowserRouter）
- Vitest + @testing-library/react（测试）
- @tiptap/react + @tiptap/starter-kit（富文本）
- npm（包管理）

### 1.2 驱动方式

headless BG_WORKGRAPH 模式，由 seed_workgraph_from_mission() 自动从 AGENTS.md
生成里程碑，然后逐一自动推进。

### 1.3 观察模式

GLM-5.2（观察者）：
1. 准备：编译 codecoder、复制二进制、写 AGENTS.md 和 codecoder.json
2. 启动：设置环境变量后运行 headless
3. 监控：tail -f .ccd.bg.ndjson 实时观察 + 等待退出
4. 报告：输出验收报告，含退出码、里程碑完成情况、构建状态、测试结果

---

## 二、项目布局

```
~/Code/smc-demo2/
├── AGENTS.md            # 使命描述（codecoder 自动生成里程碑）
├── codecoder.json       # 权限 allowlist
├── codecoder            # 主二进制
├── cc                   # 客户端二进制（赛后查询 ledger）
└── docs/proof/
    ├── 功能清单.md       # 来源：codecoder/docs/proof/
    └── experiment-report.md
```

---

## 三、运行配置

### 3.1 环境变量

| 变量 | 值 | 说明 |
|------|-----|------|
| `CODECODER_DEFAULT_TRUST` | `always` | 绕过 trust 门 |
| `CODECODER_BG_WORKGRAPH` | `1` | 启用 workgraph 模式 |
| `CODECODER_MAX_TOKENS` | `16384` | 生成上限 |
| `CODECODER_API_KEY` | 从 .ccd.env 读取 | 来自 codecoder 项目 |
| `CODECODER_MODEL` | `deepseek-v4-flash` | 与上次实验一致 |
| `CODECODER_BG_MAX_AUTO` | `10` | 最多推进 10 个里程碑 |
| `CODECODER_BG_MAX_FIX_ATTEMPTS` | `3` | needs_fix 自动重试次数 |

### 3.2 权限预授权

```json
{
  "allowlist": [
    "write_file",
    "edit_file",
    "commit",
    "run_command:npm",
    "run_command:node",
    "generate_skill",
    "generate_milestones"
  ]
}
```

---

## 四、AGENTS.md 设计

详见 docs/proof/ 下的文档。核心要点：
- 明确六域覆盖范围（驾驶舱、规划、机构、绩效、制度、智能与系统）
- 技术栈硬约束（Ant Design、Zustand、TipTap 等）
- 验收标准（`npm run build` 零错误、`npm test` 通过、代码提交）
- 目录结构约定

---

## 五、实验流程

### 5.1 准备阶段（观察者执行）

1. `cargo build --release` 编译静态连接二进制（避免运行时找动态库）
2. 将 `target/release/codecoder` 和 `target/release/cc` 复制到 `~/Code/smc-demo2/`
3. 将 `docs/proof/` 下的文档复制到目标项目的 `docs/proof/`
4. 创建 `AGENTS.md` 使命描述
5. 创建 `codecoder.json` 权限配置

### 5.2 运行阶段（codecoder 自主执行）

1. 从目标目录启动 headless
2. codecoder 读取 AGENTS.md → 自动生成里程碑 → 逐一推进
3. 观察者实时监控 `.ccd.bg.ndjson`

### 5.3 验收阶段（观察者执行）

1. 等待退出码确定终态
2. 查询 `./cc ledger --detail` 和 `./cc ledger --last`
3. 检查 `npm run build` 是否通过
4. 检查 `npm test` 是否通过
5. 检查 git log 确认代码已提交
6. 生成实验报告

---

## 六、成功标准

| 标准 | 对比上次实验 |
|------|-------------|
| 退出码 0 (CompletedAllReady) | ✅ 上次已达成 |
| GitHub 风格里程碑数为 5-8 | 上次手动 seed 5 个 |
| `npm run build` 通过 | ❌ 上次 1 个 TS 错误 |
| `npm test` 全部通过 | ❌ 上次 2/2 失败 |
| 功能覆盖 > 16%（> 10 个页面有真实组件） | ❌ 上次 ~8 个 |
| 代码已 git commit | ❌ 上次未提交 |

---

## 七、风险与应对

| 风险 | 概率 | 应对 |
|------|------|------|
| 复合命令权限拒绝 | 中 | 指导 agent 避免 `&&`/`2>&1` |
| 测试框架适配问题 | 高 | AGENTS.md 中预埋 jsdom mock 提示 |
| 模型弱导致过度探索 | 低 | deepseek-v4-flash 上次表现出色 |
| 功能覆盖不足 | 中 | 每次完成加大功能覆盖预期 |
