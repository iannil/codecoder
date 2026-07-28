# 战略管控系统 CodeCoder 自主构建验证 — 实施计划

> **特别说明：** 本计划的"实施"主体是 codecoder 自身（headless BG_WORKGRAPH 模式）。
> GLM-5.2 作为观察者负责：环境准备 → 启动驱动 → 实时监控 → 赛后验收。
> codecoder 自主完成：使命解析 → 里程碑生成 → 项目构建 → 自验收。

**目标：** 验证 codecoder 在最新修复（ADR 0039 等）合并后，headless BG_WORKGRAPH 模式下
从零构建 Vite+React+TS 前端项目的能力，并与 2026-07-25 实验（退出码 0、1 TS 错误、测试全挂、16% 覆盖）对比。

**架构：** codecoder headless 进程读取 AGENTS.md → 调用 `generate_milestones` 工具自动分解为 5-8 个里程碑 →
逐里程碑推进 agent turn（写代码→自测→验收→下一个）→ 所有里程碑完成或卡住时退出。

**技术栈：**
- host: codecoder（Rust 二进制），LLM: deepseek-v4-flash
- target: Vite 5 + React 18 + TypeScript + Ant Design 5 + Zustand + React Router v6 + Vitest + TipTap
- 观察工具: tail -f .ccd.bg.ndjson, ./cc ledger

---

## 全局约束

1. 目标文件夹: `~/Code/strategic-management-system`
2. 二进制来源: codecoder 项目 `cargo build --release` 产物
3. AGENTS.md 必须清晰描述六域覆盖范围和技术栈约束
4. allowlist 预授权 headless 所需的写/编辑/提交/命令工具
5. 环境变量通过 export 注入（不从 .ccd.env 读取 API key）
6. 等待 headless 进程自然退出（不 SIGINT 打断）
7. 赛后独立验证构建和测试结果（不信任 agent 自报）

---

### 准备任务 1：编译 codecoder 二进制

**说明：** 以 release 模式编译 codecoder 项目，产出最小依赖的静态二进制。

- [ ] **Step 1: 在 codecoder 项目根目录执行 release 编译**

```bash
cd /Users/rong.zhu/Code/codecoder
cargo build --release
```

- [ ] **Step 2: 确认二进制文件存在**

```bash
ls -la /Users/rong.zhu/Code/codecoder/target/release/codecoder
ls -la /Users/rong.zhu/Code/codecoder/target/release/cc
```

预期：两个文件存在且可执行。

---

### 准备任务 2：创建目标项目目录

- [ ] **Step 1: 创建目标文件夹结构**

```bash
mkdir -p ~/Code/strategic-management-system/docs/proof
```

- [ ] **Step 2: 复制二进制文件**

```bash
cp /Users/rong.zhu/Code/codecoder/target/release/codecoder ~/Code/strategic-management-system/codecoder
cp /Users/rong.zhu/Code/codecoder/target/release/cc ~/Code/strategic-management-system/cc
```

- [ ] **Step 3: 复制功能文档**

```bash
cp /Users/rong.zhu/Code/codecoder/docs/proof/功能清单.md ~/Code/strategic-management-system/docs/proof/
cp /Users/rong.zhu/Code/codecoder/docs/proof/experiment-report.md ~/Code/strategic-management-system/docs/proof/
```

- [ ] **Step 4: 验证复制结果**

```bash
ls -la ~/Code/strategic-management-system/
ls -la ~/Code/strategic-management-system/docs/proof/
```

---

### 准备任务 3：配置 AGENTS.md

- [ ] **Step 1: 创建 AGENTS.md**

写入以下使命描述：

```markdown
# 战略管控系统前端 — 自主构建

## 使命

基于 Vite + React 18 + TypeScript + Ant Design + Zustand + React Router + Vitest 技术栈，
从零创建一个战略管控系统的前端 Demo 项目。

## 技术栈约束

- 构建工具: Vite 5
- UI 框架: Ant Design 5 (antd)
- 状态管理: Zustand
- 路由: React Router v6 (react-router-dom)
- 测试: Vitest + @testing-library/react + jsdom
- 包管理: npm
- 富文本: @tiptap/react + @tiptap/starter-kit
- 图标: @ant-design/icons

## 系统覆盖范围

完整实现六大业务域的核心页面：

1. **战略驾驶舱**: Dashboard（仪表盘页面，系统落地页）
2. **战略规划**: 规划档案、产业研究、规划制定（含富文本编辑器）、审批、分解、投资计划、评估、仿真
3. **机构管理**: 法人注册/变更/注销、机构台账
4. **组织绩效**: 考核方案、过程考核、指标管理、年度考核
5. **制度管理**: 分类标签、制度检索
6. **智能与数据中枢**: AI 能力中心、数据集成、通知预警
7. **系统管理**: 用户权限、流程引擎、统一门户

未实现具体内容的叶子页面使用 PlaceholderPage 占位，但必须注册路由。

## 验收要求

1. `npm run build` 通过（零 TypeScript 错误）
2. `npm test` 通过（需适配 jsdom，在 setupFiles 中添加 window.matchMedia mock）
3. 所有页面均有路由注册
4. 代码通过 git commit 提交
5. 遵循 Ant Design 最佳实践，组件按需导入

## 目录结构

```
src/
├── router.tsx           # 完整路由（含 pageMap）
├── App.tsx
├── main.tsx
├── setupTests.ts        # 测试全局配置（含 matchMedia mock）
├── layouts/
│   ├── MainLayout.tsx
│   ├── SubNavLayout.tsx
│   └── SectionConsole.tsx
├── components/
│   ├── layout/
│   │   ├── MasterDetailLayout.tsx
│   │   └── PlaceholderPage.tsx
│   ├── planning/
│   │   ├── RichTextEditor.tsx
│   │   ├── AiCopilotRail.tsx
│   │   ├── VersionDrawer.tsx
│   │   ├── CommentsDrawer.tsx
│   │   └── CitationPanel.tsx
│   └── approval/
│       └── ApprovalPanel.tsx
├── pages/
│   ├── dashboard/
│   ├── planning/
│   │   ├── index.tsx
│   │   ├── research/
│   │   ├── formulation/
│   │   ├── approval/
│   │   ├── decomposition/
│   │   ├── investment/
│   │   ├── evaluation/
│   │   └── simulation/
│   ├── institution/
│   ├── performance/
│   ├── policy/
│   ├── intelligence/
│   └── system/
└── lib/
    └── collaborationCursor.ts
```

## 约束

- 禁止使用 tailwindcss
- 使用 react-router-dom 的 createBrowserRouter
- 状态管理统一用 Zustand
- 所有 Ant Design 组件按需导入（import { Button } from 'antd'）
```

---

### 准备任务 4：配置 codecoder.json（权限）

- [ ] **Step 1: 创建 codecoder.json**

```json
{
  "allowlist": [
    "write_file",
    "edit_file",
    "commit",
    "generate_skill",
    "generate_milestones",
    "run_command:npm",
    "run_command:node",
    "run_command:git",
    "run_command:cargo"
  ]
}
```

---

### 准备任务 5：启动 codecoder headless 构建

- [ ] **Step 1: 读取 API key 并注入环境变量**

```bash
source /Users/rong.zhu/Code/codecoder/.ccd.env
export CODECODER_DEFAULT_TRUST=always
export CODECODER_BG_WORKGRAPH=1
export CODECODER_MAX_TOKENS=16384
export CODECODER_BG_MAX_AUTO=10
export CODECODER_BG_MAX_FIX_ATTEMPTS=3
```

- [ ] **Step 2: 启动 headless（后台运行，方便观察）**

```bash
cd ~/Code/strategic-management-system
./codecoder &
```

或（实时观察前台输出）：

```bash
cd ~/Code/strategic-management-system
CODECODER_DEFAULT_TRUST=always \
CODECODER_BG_WORKGRAPH=1 \
CODECODER_MAX_TOKENS=16384 \
CODECODER_BG_MAX_AUTO=10 \
CODECODER_API_KEY=$CODECODER_API_KEY \
CODECODER_MODEL=$CODECODER_MODEL \
./codecoder 2>&1
```

- [ ] **Step 3: 观察初始化阶段**

```bash
tail -f .ccd.bg.ndjson
```

预期观察到：`seed: empty workgraph — attempting to seed from AGENTS.md...` →
`seed: workgraph seeded successfully — entering milestone loop` →
然后逐里程碑推进事件输出。

---

### 观察任务 6：实时监控里程碑推进

- [ ] **Step 1: 观察 milestone turn 事件**

使用 `tail -f .ccd.bg.ndjson` 观察字段：
- `"event":"milestone_start"` / `"event":"milestone_done"`
- `"phase":"milestone:N"` 指示当前里程碑

- [ ] **Step 2: 记录每个里程碑结果**

| 期望观察到的里程碑 | 状态 |
|-------------------|------|
| M1: 项目初始化（Vite + 依赖安装） | |
| M2: 路由框架 + 布局组件 | |
| M3: 战略驾驶舱 + 共享组件 | |
| M4: 业务页面模块 | |
| M5: 测试适配 + 构建验证 + git提交 | |
| ...（codecoder 自动生成） | |

---

### 验收任务 7：赛后结果检查

- [ ] **Step 1: 检查退出码**

```bash
echo $?
```

- [ ] **Step 2: 查询账本**

```bash
./cc ledger --detail
./cc ledger --last
```

- [ ] **Step 3: 检查构建**

```bash
cd ~/Code/strategic-management-system
npm run build 2>&1
```

- [ ] **Step 4: 检查测试**

```bash
npm test 2>&1
```

- [ ] **Step 5: 检查 git 提交**

```bash
cd ~/Code/strategic-management-system
git log --oneline -10
```

- [ ] **Step 6: 统计功能覆盖**

```bash
find src/pages -name "*.tsx" | sort
find src/components -name "*.tsx" | sort
```

- [ ] **Step 7: 生成对比报告**

对照 2026-07-25 实验数据，输出对比表格。

---

### 应急任务 8：问题诊断与终止

仅在 codecoder 无法完成时执行：

- [ ] **Step 1: 判断是否需终止**
  - 连续 5 分钟无输出
  - 退出码 2(StuckNeedsFix) 且重试耗尽
  - 退出码 3(CircuitBreaker)
  - 退出码 4(Error)

- [ ] **Step 2: 收集诊断信息**

```bash
cat .ccd.bg.ndjson | tail -50
./cc ledger --detail
cat workgraph.json 2>/dev/null | python3 -m json.tool
```

- [ ] **Step 3: 梳理发现的问题**

按以下格式输出：
| 问题分类 | 问题描述 | 影响 | 建议修复方向 |
|----------|---------|------|-------------|
| codecoder 缺陷 | ... | ... | ... |
| AGENTS.md 问题 | ... | ... | ... |
| 模型行为 | ... | ... | ... |

- [ ] **Step 4: 终止**

```bash
kill %1 2>/dev/null
```
