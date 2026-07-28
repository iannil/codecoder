# 战略管控系统 — CodeCoder 自主构建设计

> 基于 docs/proof 中的功能清单与实验报告
> 驱动模式: CODECODER_BG_WORKGRAPH（单次启动，全播种+动态调整）
> 设计日期: 2026-07-28

---

## 一、项目概况

| 项目 | 值 |
|------|------|
| 项目名 | `smcs` |
| 路径 | `~/Code/smcs/` |
| 目标 | 基于「功能清单.md」全量覆盖 6 大业务域、50+ 页面 |
| 技术栈 | CodeCoder 自主选择（现代 React SPA 方向） |
| 构建策略 | 全播种 workgraph + 允许 codecoder 动态调整里程碑 |
| 我的角色 | 仅观察监管，不参与开发 |

## 二、驱动方案: C2（全播种+动态调整）

### 核心原则

1. **workgraph.json 预播种所有里程碑**，覆盖全部 6 个业务域
2. 前几个里程碑详细、后几个提纲式，codecoder 沿路径推进
3. 如果里程碑过重需要拆分，允许 codecoder 用 `write_file` 动态调整后续里程碑
4. 单次启动 `CODECODER_BG_WORKGRAPH=1`，只观察不参与
5. 如果 codecoder 无法完成，终止项目并梳理问题

### 启动参数

| 环境变量 | 值 | 说明 |
|---------|-----|------|
| `CODECODER_BG_WORKGRAPH` | `1` | 启用 workgraph 逐里程碑模式 |
| `CODECODER_BG_MAX_AUTO` | `0` | 不限自动推进次数 |
| `CODECODER_BG_CIRCUIT_K` | `20` | 熔断阈值为 20 |
| `CODECODER_BG_MAX_FIX_ATTEMPTS` | `3` | 每个里程碑最多 3 次自动修复 |
| `CODECODER_ROOT` | `~/Code/smcs` | 项目根目录 |
| `CODECODER_MODEL` | 待定（由用户决定） | 底层 LLM |

### 关键文件

| 文件 | 说明 |
|------|------|
| `workgraph.json` | 预播种的里程碑列表（~10 个） |
| `AGENTS.md` | 顶层指令，含动态调整指示 |
| `.ccd.env` | 环境变量配置 |

---

## 三、里程碑规划

### 初始播种 ~10 个里程碑

| # | 里程碑名 | 描述 | 预期产出 |
|---|---------|------|---------|
| M1 | 项目脚手架 | 初始化前端项目，安装依赖，配置构建工具 | `package.json`, `vite.config.ts`, `tsconfig.json`, `index.html` |
| M2 | 路由框架与布局 | 搭建 router.tsx 路由树、MainLayout、SubNavLayout、占位页体系 | `router.tsx`, `layouts/`, `PlaceholderPage.tsx` |
| M3 | 公共组件库 | 实现所有共享组件：RichTextEditor、ApprovalPanel、MasterDetailLayout、SectionConsole、AiCopilotRail、CitationPanel、CommentsDrawer、VersionDrawer | `components/` 下共享组件 |
| M4 | 战略驾驶舱 | 顶层 Dashboard + 战略总览驾驶舱页面 | `Dashboard.tsx` + 首页 |
| M5 | 战略规划管理 (PLG-01) | 8 个二级分组全部页面：规划档案、产业研究、规划制定、审批、分解、投资计划、评估、仿真推演 | `pages/planning/` 下所有页面 |
| M6 | 机构管理 (INST-02) | 4 个二级分组：注册、变更、注销、台账 | `pages/institution/` 下所有页面 |
| M7 | 组织绩效 (PERF-03) | 6 个二级分组：考核方案、过程考核、指标管理、年度考核、半年度、个性化设置 | `pages/performance/` 下所有页面 |
| M8 | 制度管理 (POL-04) + 智能中枢 (INT-05) | 制度库 + AI 能力中心 + 数据集成 + 通知预警 | `pages/policy/`, `pages/intelligence/` 下页面 |
| M9 | 系统管理 (SYS-06) | 用户权限 + 流程引擎 + 统一门户 | `pages/system/` 下页面 |
| M10 | 集成验收与修复 | 修复构建错误、测试失败，确保 `npm run build` 通过 | 最终可构建状态 |

### 变量说明

- M1-M3 为基础层，M4-M9 为业务域层，M10 为验收层
- codecoder 可以在执行过程中将任一里程碑拆分为多个子里程碑
- 里程碑顺序按**依赖关系**排列：先基础设施，后业务域，业务域之间正交可并行

---

## 四、准备工作（由我执行，非开发行为）

在启动 codecoder 之前，由我（观察者）完成的准备工作：

| 步骤 | 说明 |
|------|------|
| 创建目录 | `mkdir -p ~/Code/smcs` |
| 编译 codecoder | `cargo build --release`，将二进制复制到 `~/Code/smcs/` |
| 编写 seed AGENTS.md | 写入顶层指令，约束 codecoder 行为（见下方） |
| 编写 seed workgraph.json | 预播种 10 个里程碑 |
| 设置环境变量 | `CODECODER_ROOT`、`CODECODER_BG_WORKGRAPH` 等 |

这些是 **基础设施准备，不是项目开发**，由我完成。之后的全部开发工作由 codecoder 自主完成。

---

## 五、AGENTS.md 种子文件内容

```markdown
# smcs — 战略管控系统

## 项目说明
基于 docs/proof/功能清单.md 构建一个完整的战略管控系统前端 SPA。
需覆盖 6 大业务域、50+ 页面，所有页面需有真实组件实现。

## 技术栈
自由选择（推荐现代 React + TypeScript + Vite 方向）。

## 行为约束
1. 每个里程碑完成后检查 workgraph.json，必要时用 write_file 调整后续里程碑（拆分/合并/重排序）
2. 每次 git commit 前确保 npm run build 通过
3. 为关键业务逻辑编写基础测试
4. 每个里程碑完成后执行 git add + git commit
5. 避免复合 shell 命令（如 2>&1、&&、|），使用单步命令
6. 如果某里程碑需要拆分，将新里程碑追加到 workgraph.json 的 milestones 数组中并更新 edges
```

---

## 六、上次实验问题的对应对策

| 上次问题 | 本次对策 |
|---------|---------|
| 复合命令 keying 拒绝 | AGENTS.md 中指示避免复合命令（`2>&1`、`&&`），改用单步命令 |
| Review gate 依赖自报 VERDICT | 已升级（ADR 0039 的独立评审），且每个里程碑设置 `command` 字段 |
| 空 workgraph 不自动创建里程碑 | 预播种所有里程碑 |
| `bg_max_auto` 默认 3 太小 | 设为 `0`（不限） |
| `.ccd.env` 不注入 API_KEY | 用户从真实 shell export API_KEY |
| 构建错误未自动修复 | `bg_max_fix_attempts=3`，AGENTS.md 指示修复 |
| 测试未适配 jsdom | AGENTS.md 中明确要求添加测试 mock |
| 代码未提交 | AGENTS.md 明确要求每个里程碑后 commit |

---

## 七、失败终止条件

如果观察中发现以下情况，终止项目并记录问题：

1. **连续 3 个里程碑卡住无法推进**（`StuckNeedsFix` 退出码 2）
2. **构建错误反复出现，修复轮无法解决**
3. **codecoder 偏离核心方向**（如开始编写无关代码）
4. **熔断触发**（`bg_circuit_k` 耗尽）

终止后产出：
- 已完成的里程碑清单与代码量
- 失败原因分析
- 需要修复的 codecoder 自身问题

---

## 七、预期成果

| 指标 | 目标 |
|------|------|
| 里程碑完成 | ≥ 8/10 |
| 源文件数 | ≥ 60 个 |
| 代码行数 | ≥ 5000 行 |
| 功能覆盖 | ≥ 40/50 页面 |
| 构建可通过 | `npm run build` 0 错误 |

---

## 八、后续实验（待本轮修复后）

1. 去掉 `command` 字段，测试 review gate 独立评审能力
2. 不预播种里程碑，测试自动创建里程碑（待 P0-1 修复合并后）
3. 使用真实的 LLM（非 stub）测试端到端效果
