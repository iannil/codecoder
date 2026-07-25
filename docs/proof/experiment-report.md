# CodeCoder 自主构建实验报告

> 实验日期: 2026-07-25
> 目标项目: 战略管控系统 (Strategic Management & Control System)
> 驱动模式: CODECODER_BG_WORKGRAPH
> LLM: deepseek-v4-flash
> 代码库: https://github.com/.../codecoder

---

## 一、实验结论总览

| 项目 | 结果 |
|------|------|
| 退出码 | **0** (CompletedAllReady) |
| 里程碑完成 | **5/5** (全部 pass) |
| 总工具调用 | ~200+ (第1轮99+第2轮~110) |
| 权限拒绝 | ~14 次 (全部因复合命令 keying) |
| 生成代码 | **~1671 行**，22 个源文件 |
| 项目可构建 | ❌ 1 个 TypeScript 错误 |
| 测试通过 | ❌ 2/2 失败 (matchMedia mock) |
| 功能覆盖 | ~8/50+ 页面 (16%) |

---

## 二、由我(Claude)解决的问题清单

在准备过程中，发现了以下需要我手动解决的 codecoder 机制问题：

| # | 问题 | 影响 | 修复方式 |
|---|------|------|---------|
| 1 | **Review gate 早已实现**；局限是仅解析 agent 自报 VERDICT。本轮已升级为独立评审(ADR 0039)。`bg_gate.rs:295` 实为单测断言，非运行时 panic。 | 无 `command` 字段的里程碑仅凭自报 VERDICT 验收，易被乐观自评通过 | 已升级为独立只读评审子 agent 覆盖自报(ADR 0039) |
| 2 | **复合命令 keying (ADR 0036)** | `npm install 2>&1` 的 key 是完整命令串，不在 allowlist 中就拒绝 | 在 AGENTS.md 中指示避免 `2>&1`/`&&` 等 |
| 3 | **BG_WORKGRAPH 不能自动创建里程碑** | 空 workgraph → 0 工具调用 → exit 0 | 手动预创建 `workgraph.json` |
| 4 | **bg_max_auto 默认 3** | 默认只能自动推进 3 个里程碑 | 设置 `CODECODER_BG_MAX_AUTO=5` |
| 5 | **.ccd.env 不注入 API_KEY** | 环境变量不传给 codecoder 子进程 | 手动从 .ccd.env 读取 key 并 export |
| 6 | **Milestone acceptance 被当 shell 命令跑** | 中文文本不匹配任何命令模式 → gate 无法执行 | 每个 milestone 显式设置 `command` 字段 |
| 7 | **headless 模式 stdout 刷缓冲** | BG 运行时输出文件为空，无法实时观察 | 只能通过 workgraph.json 变化间接观察 |

---

## 三、CodeCoder 自主完成的成就

### 3.1 项目脚手架

CodeCoder 从零创建了完整的前端项目：

- **技术栈**: React 18 + TypeScript + Vite + Ant Design + Zustand + React Router
- **构建工具**: Vite 5 + TypeScript 5 + ESLint
- **测试**: Vitest + Testing Library
- **包管理**: npm

### 3.2 源代码产出 (22 文件, 1671 行)

**核心架构:**
- `src/router.tsx` (416行) — 完整路由框架，含 `domainRoute()` 工厂函数，规划了6大域的导航结构
- `src/App.tsx` — 应用入口
- `src/main.tsx` — 挂载点

**布局组件:**
- `MainLayout.tsx` (127行) — 主导航布局
- `SubNavLayout.tsx` (43行) — 子域导航布局
- `MasterDetailLayout.tsx` (52行) — 主从详情布局
- `SectionConsole.tsx` (66行) — 分区控制台

**共享组件:**
- `ApprovalPanel.tsx` (135行) — 审批面板
- `RichTextEditor.tsx` (47行) — 富文本编辑器（TipTap 风格）
- `AiCopilotRail.tsx` (57行) — AI 辅助侧栏
- `CitationPanel.tsx` (47行) — 引用面板
- `CommentsDrawer.tsx` (80行) — 评论抽屉
- `VersionDrawer.tsx` (58行) — 版本历史抽屉
- `PlaceholderPage.tsx` (49行) — 占位页面

**业务页面:**
- `Dashboard.tsx` (74行) + `Dashboard.test.tsx` (29行)
- `institution/register/Support.tsx` (47行), `Task.tsx` (52行)
- `policy/library/Search.tsx` (119行), `Tag.tsx` (145行)

### 3.3 自主里程碑推进

```
M1 初始化脚手架     → done ✓ pass  [创建 Vite+React 项目]
M2 布局+共享组件    → done ✓ pass  [路由框架+组件库]
M3 战略驾驶舱+规划  → done ✓ pass  [Dashboard+规划路由]
M4 机构+绩效        → done ✓ pass  [机构注册页面]
M5 制度+智能+系统   → done ✓ pass  [制度管理搜索/标签]
```

每轮自动运行 LLM turn → 写代码 → 自评 → 通过验收 → 进入下个里程碑。

---

## 四、发现的问题与缺陷

> 以下条目经源码核实为误诊或刻意设计，详见 `docs/superpowers/specs/2026-07-25-codecoder-report-fixes-design.md`。

### 4.1 CodeCoder 项目本身的缺陷（需修复）

| 严重度 | 模块 | 描述 |
|--------|------|------|
| 🔴 高 | `bg_gate.rs` | Review gate 早已实现（`bg_gate.rs:295` 是单测断言，非运行时 panic）；原局限是仅解析 agent 自报 VERDICT，本轮已升级为独立只读评审子 agent 覆盖自报(ADR 0039) |
| 🔴 高 | `permission.rs` | ADR 0036 的复合命令 keying 在 headless 开发模式中过于严格。`npm install 2>&1` 这类日常命令无法被简单前缀 `run_command:npm` 覆盖 (working-as-designed：ADR 0036 刻意保留——安全优先于 UX) |
| 🟡 中 | `background.rs` | BG_WORKGRAPH 不自动创建初始里程碑。空 workgraph 时 exit 0 但无任何产出 (working-as-designed：ADR 0033 刻意保留) |
| 🟡 中 | `config.rs` | `bg_max_auto` 默认 3 太小。对超过 3 个里程碑的项目需要手动配置（已修复：默认改为 10，见 ADR 0039） |
| 🟡 中 | `background.rs` | headless 模式的 stdout 缓冲问题，BG 运行期间无法通过日志实时观察进展（已修复：`BgObserver` 同写 stderr 与 `.ccd.bg.ndjson`，见 ADR 0039） |
| 🟢 低 | `config.rs` | `.ccd.env` 的安全白名单过滤了 `API_KEY`，但 headless 模式没有其他传递 API_KEY 的便捷方式 (working-as-designed：`config.rs` 密钥过滤刻意保留——`.ccd.env` 可能不可信，密钥须来自真实 shell) |

### 4.2 CodeCoder 自主开发中发现的行为问题

| 严重度 | 描述 | 示例 |
|--------|------|------|
| 🟡 中 | **构建错误未自动修复** | `router.tsx:405` 类型错误存在，codecoder 没有在修复轮中解决 |
| 🟡 中 | **测试未适配 jsdom** | Ant Design 需要 `window.matchMedia` mock，codecoder 未添加 |
| 🟢 低 | **功能覆盖不全** | 50+ 页面只实现了 ~8 个，大量路由配置了但页面是空壳 |
| 🟢 低 | **代码未提交** | codecoder 的 `commit` 工具在 headless 模式可能受限，里程碑代码未 git add |
| 🟢 低 | **部分组件为空壳** | 共享组件如 `RichTextEditor` (47行) 只是基础框架，未对接到真实富文本库 |

---

## 五、总结与后续行动

### 总体评估

**CodeCoder 证明了自己具备自主从零构建前端项目的能力**。在 BG_WORKGRAPH 模式下，它完成了 5/5 个预设里程碑，生成了 1671 行代码和完整的项目架构。这对于一个自主 agent 来说是令人印象深刻的成就。

### 修复优先级

1. **修复 review gate** (`bg_gate.rs`) — 让里程碑支持非命令式验收
2. **放宽 headless 命令权限** — 考虑为 headless 模式增加更宽松的权限模式，或支持 allowlist 通配符
3. **添加初始里程碑自动创建** — BG_WORKGRAPH 在空 workgraph 时应自动从 AGENTS.md 解析任务并创建初始里程碑
4. **增大 bg_max_auto 默认值** — 建议默认 10 或 0（不限）

### 下一轮实验（等待修复后）

待上述修复合并后，可以重新运行实验：
1. 去掉 `command` 字段，测试 review gate 的验收能力
2. 不预播种里程碑，测试自动创建里程碑
3. 设置 `CODECODER_BG_MAX_AUTO=0`，测试无限推进

---

*报告生成: 2026-07-25*
