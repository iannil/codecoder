---
name: engineer-poc
description: >
  【强触发 / Strong trigger】"做个高保真原型 / POC / 可点击 demo" + 已有需求或前端设计 + 纯前端演示 —— 把需求变成全功能、全状态、贴合行业风格的可运行纯前端演进式原型。
  ROUTING RULE: 位于 engineer-job 流程的需求分析(Phase 1)+前端设计(Phase 3)之后、正式实现(Phase 4)之前。工程可停在 POC、可跳过 POC、也可接着 POC 演进为完整功能。
  AI 高保真 POC 生成引擎 — 读取 REQUIREMENTS.md + FRONTEND-DESIGN.md（+ CONTEXT.md），
  产出真实前端工程骨架 + 可替换 mock 数据层 + POC-MANIFEST.md / POC-FIDELITY.md，
  再交棒 engineer-job Phase 4 把 mock 演进为真实实现。
  TRIGGERS: "做个原型""高保真原型""POC""样机""可点击 demo""交互原型""先出个能演示的前端"
  "prototype""high-fidelity prototype""clickable demo""interactive mockup"
  也触发于：engineer-job Phase 3 完成后（除非 skip_poc），或用户在需求/前端设计后说"先做 POC"。
compatibility: "agent, bash, write, edit, read"
---

# engineer-poc — AI 高保真 POC 生成引擎 / AI High-Fidelity POC Engine

> **来源声明**: 本 skill 的方法论来源于《基于实现规划的 AI 辅助编程实战》。更多内容请访问 [zhurongshuo.com]。
>
> **Source**: The methodology of this skill originates from "AI-Assisted Programming Practice Based on Implementation Planning". Visit [zhurongshuo.com] for more context.
>
> **参考架构**: 覆盖账本范式复用 `engineer-cloner` / `engineer-legacy-recon`；状态覆盖范式复用 `engineer-frontend-architect`；构建链交棒 `engineer-job`。

---

## 🎯 核心理念 / Core Philosophy

POC 的职责是：在写下第一个真实后端接口之前，让**整个产品先"像真的、像做完了"**——每个页面、每个状态、每条流程都能在 mock 数据上点得动、走得通、贴合行业风格——同时诚实标注数据层是 mock 且**为演进而设计**。

不是一次性抛弃式样机，而是一副**真实前端骨架，其 mock 层被刻意设计成可被真实 API 替换的单一接缝**。

**A POC makes the whole product feel real and complete before a single real endpoint exists — every screen, every state, every flow clickable on mock data, styled to industry conventions — while staying honest that the data layer is mocked and built to evolve.**

### 四条核心原则

#### 原则一：全功能靠账本，不靠记忆 / Coverage via Ledger, Not Memory

"全功能覆盖"不能靠对话记忆——长会话必然遗漏页面、状态、边角交互。覆盖靠一份持久化账本：

- `.agents/poc.ledger.json` —— 由 `REQUIREMENTS.md` + `FRONTEND-DESIGN.md` 驱动，记录每个页面/组件/状态/流程及其覆盖状态与保真度层级（schema 见 `references/poc-ledger.schema.json`）。
- **loop-until-dry** —— 反复实现，直到账本不再新增未覆盖节点。
- **coverage critic** —— 每轮复查账本，追问"还有哪些页面/状态/流程/角色视图没覆盖"。

**Coverage is a file, not a memory.**

#### 原则二：全生命周期即全状态 / Full Lifecycle Means Every State

**Happy path 不是唯一路径。** 每个页面必须覆盖：

- **loading**: 首次加载、刷新、部分加载
- **empty**: 无数据、清零、搜索无结果
- **error**: 网络错误、权限不足、服务端异常
- **normal**: 数据正常展示
- **edge**: 分页结尾、最大长度、频控

外加完整 mock CRUD 生命周期、跨页流程、mock 登录/角色切换。只画 happy path 不算 POC。

#### 原则三：行业保真优于通用模板 / Industry Fidelity Over Generic Templates

POC 要像"这个行业里的真实产品"，不是通用模板。

- 先从 `REQUIREMENTS.md` **识别行业/形态**，套用 `references/industry-patterns.md` 的典型信息架构、组件、交互范式与最佳实践。
- 设计 Token（色彩/排版/间距）取自 `FRONTEND-DESIGN.md`，不手写魔法值。
- 必要时用 WebSearch 对特定行业惯例做增强（可选、联网）。

#### 原则四：Mock 层可演进，诚实标注 / Evolvable Mock Layer, Honest Labeling

全部数据走一个可替换的 mock adapter（**单一接缝**，见 `references/mock-layer-guide.md`），使 engineer-job Phase 4 只需替换这一层即可接真实 API。每条资产落入三个保真度层级之一：

| 层级 / Tier | 标签 | 含义 |
|:--|:--|:--|
| 真实交互 / Real | `真实交互` | 前端逻辑完整、状态齐全、可点可走，仅数据来自 mock。 |
| mock 数据 / Mocked | `mock 数据` | 数据来自 mock adapter，契约按 CONTEXT.md 或推断，可演进为真实 API。 |
| 占位未实现 / Placeholder | `占位未实现` | 支付网关 / 第三方 SaaS / 纯服务端算法 → 占位，留待正式实现阶段接入。 |

**Honesty about the tier is part of the deliverable.**

---

## 🚦 触发条件 / When to Trigger

**必须触发**：

- "做个原型""高保真原型""POC""样机""可点击 demo""交互原型""先出个能演示的前端"
- "prototype""high-fidelity prototype""clickable demo""interactive mockup"
- 用户已有需求或前端设计，想先出一个能演示、能点通的纯前端产品

**链式触发**：

- engineer-job Phase 3（frontend）完成后自动进入 Phase 3.5（除非 `skip_poc`）
- 用户在需求/前端设计完成后说"先出个原型""先做 POC"

**不触发**：

- 纯后端 / CLI / 库项目（无前端）→ 自动跳过
- 用户明确要求直接进入正式实现（`skip_poc`）

**优先级**：

1. 用户表达"要一个可演示的高保真前端原型" → 优先触发 engineer-poc。
2. engineer-poc 完成后可停在交棒点（`stop_at_poc`），或交棒 engineer-job Phase 4 演进。

---

## ⚙️ 模式选择 / Mode Selection

通过 `--mode` 参数控制自动确认程度（默认 normal）：

| 模式 | 行为 |
|:----:|------|
| normal | 每阶段展示产出后等待确认；行业范式与设计 Token 需用户验证。 |
| auto | 用默认策略自动推进遍历与实现，仅在重大异常时暂停。 |
| silent | 全自动，仅记录日志，末尾输出交棒摘要。 |

---

## 🏗️ 六阶段流水线 / Pipeline

| 阶段 | 名称 | 输入 → 输出 | 关键动作 | 失败处理 |
|:--:|:--|:--|:--|:--|
| 0 | 输入校验与行业识别 | 三文档 → POC 范围 + 行业匹配 | 读 REQUIREMENTS / FRONTEND-DESIGN（/CONTEXT）；缺则降级（从用户需求生成最小集）；识别行业匹配 `industry-patterns`；划定 do-not-fake 边界（支付/第三方→占位）。 | 三文档全缺 → 降级：从用户 prompt 生成最小需求+页面清单 |
| 1 | POC 脚手架 | 范围 → 演进式前端工程 | 按 FRONTEND-DESIGN 技术栈（默认 Vite + React/Vue）搭骨架，接入设计 Token、路由、**mock adapter 接缝**。 | 技术栈缺失 → 用默认栈，记录 |
| 2 | 覆盖账本构建 | FRONTEND-DESIGN → `poc.ledger.json` | 把每个页面/组件/状态/流程枚举进账本（coverage_status=planned）。 | — |
| 3 | 逐页高保真实现（loop-until-dry） | 账本 → 高保真页面 | 逐页实现全 UI 状态 + mock 数据 + 交互，套用行业范式；账本跑干；coverage critic 复查遗漏。 | 单页失败 → 降级为占位页，标 `占位未实现` |
| 4 | 全生命周期贯通 | 页面 → 可点通的产品 | 打通跨页流程、mock 登录/角色、内存持久化、灌真实感 seed 数据；run gate（build + 启动）。 | build 失败 → 强制修复循环；修不动标 DOES_NOT_RUN |
| 5 | 产物合成与交棒 | 全部 → POC 文档 + 归宿 | 写 `POC-MANIFEST.md` + `POC-FIDELITY.md` + 报告；然后 stop（stop_at_poc）或交棒 engineer-job Phase 4。 | — |

详细阶段动作、loop-until-dry 伪码与恢复流程见 `references/pipeline.md`。

---

## 📁 产物与文件结构 / Artifacts

| 产物 | 用途 |
|:--|:--|
| 可运行的纯前端 POC 工程（仓库内） | 演进式骨架，engineer-job Phase 4 在其上继续 |
| `.agents/poc.ledger.json` | 覆盖账本——所有页面/组件/状态/流程 + 保真度层级 + 覆盖状态；loop-until-dry 与恢复的唯一真相源（schema 见 `references/poc-ledger.schema.json`） |
| `src/mocks/`（或框架约定位置） | mock adapter + seed 数据——**可替换接缝**（演进为真实 API 时只改这一层，规范见 `references/mock-layer-guide.md`） |
| `POC-MANIFEST.md` | 已建清单：页面/组件/路由/mock 端点 + **mock→真实演进映射**（engineer-job Phase 4 消费，做演进而非重建） |
| `POC-FIDELITY.md` | 逐资产诚实标注 `真实交互 / mock 数据 / 占位未实现`，供正式实现阶段对照 |

模板见 `references/poc-manifest-template.md`。

---

## 🤝 交棒 / Handoff

Phase 5 产出 POC 工程 + 两份文档后：

- **停留**（`stop_at_poc`）：输出交棒摘要，停在 POC。
- **继续**：交棒 `engineer-job`。orchestrate（Phase 4）读取 `POC-MANIFEST.md`，把 mock 层**演进**为真实实现，而非重建页面。

```javascript
Workflow({
  script: "skills/engineer-job/run.wf.js",
  args: { requirements: "见 REQUIREMENTS.md", mode: "auto", projectName: "<name>", skip_poc: false, stop_at_poc: false }
})
```

---

## ⚠️ 边界情况 / Edge Cases

| 场景 | 处理方式 |
|------|---------|
| **REQUIREMENTS.md / FRONTEND-DESIGN.md 缺失** | 降级：从用户 prompt 生成最小需求 + 页面清单，再继续；在 POC-FIDELITY.md 标注降级 |
| **无前端项目** | 自动跳过 POC（`skip_frontend` → 不生成） |
| **支付 / 第三方 SaaS 页面** | 标 `占位未实现`，用占位/mock 实现，绝不接真实支付/第三方 |
| **纯服务端算法（排序/风控/推荐）** | 前端用合理 mock 结果驱动，标 `mock 数据` 或 `占位未实现`，诚实说明 |
| **会话中断** | 从 `.agents/poc.ledger.json` 恢复：跳过 coverage_status=="implemented" 的节点，从 "planned" 继续 |
| **build/启动失败** | Phase 4 强制修复循环；修不动标 DOES_NOT_RUN，报告头条如实反映 |

---

## 🚫 非目标 / Non-Goals

1. **不做后端** —— 无真实数据库、无真实 API，全走 mock 层。
2. **不做真实鉴权/支付** —— mock 登录/角色仅用于演示；支付/第三方标 `占位未实现`。
3. **非抛弃式** —— 演进式骨架，可被 engineer-job Phase 4 继续，不是一次性 mockup。
4. **不重造构建链** —— 脚手架之外的正式实现（架构编排/里程碑/验收/部署）交回 engineer-job。
5. **不臆造不可观测的服务端逻辑** —— 纯服务端算法用占位/合理 mock，诚实标注。
