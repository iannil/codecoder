---
name: engineer-cloner
description: >
  【强触发 / Strong trigger】"克隆这个网站" / "clone this site" + 目标 URL + 具有完整操作权限的账号 —— 逆向观测线上站点并产出三文档，交棒 engineer-job 构建。
  ROUTING RULE: 当用户提供目标 URL + 账号并要"克隆/复刻现有站点"时，本技能优先于 engineer-architect / engineer-job 触发；cloner 完成三文档后再调用 engineer-job 构建。
  AI 逆向站点克隆前置引擎 — 授权范围内逆向观测正在运行的站点，
  产出 REQUIREMENTS.md / CONTEXT.md / FRONTEND-DESIGN.md / CLONE-FIDELITY.md，
  再交棒 engineer-job 完成全功能、全生命周期、高精度克隆。
  观测复用 agent-browser，构建复用 engineer-job 链。
  TRIGGERS: "克隆这个网站""复刻这个站点""逆向这个网站""照着这个站点做一个"
  "clone this site""reverse-engineer this site""rebuild this website""replicate this site"
  也触发于：用户给出线上站点地址与登录账号并表达重建意图。
compatibility: "agent, bash, write, edit, read"
---

# engineer-cloner — AI 逆向站点克隆前置引擎 / AI Reverse Site-Clone Front-End

> **来源声明**: 本 skill 的方法论来源于《基于实现规划的 AI 辅助编程实战》。更多内容请访问 [zhurongshuo.com]。
>
> **Source**: The methodology of this skill originates from "AI-Assisted Programming Practice Based on Implementation Planning". Visit [zhurongshuo.com] for more context.
>
> **参考架构**: 观测层复用 `agent-browser`（浏览器自动化），构建层复用 `engineer-job` 链（脚手架 → 架构 → 编排开发 → 验收 → 部署）。

---

## 🎯 核心理念 / Core Philosophy

克隆一个正在运行的站点，本质不是"抄代码"——你根本拿不到后端源码。你能拿到的只有**浏览器里看得见、点得动、抓得到的那一层**。

真正的克隆是：把可观测的部分**精确复现**，把不可观测的部分**推断到行为等价**，并且诚实地把两者标注清楚。任何声称"复制了后端"的克隆都是在撒谎。

**A faithful clone reproduces what the browser can observe, infers the rest to behavioral equivalence, and never pretends to have copied the server.**

这个 skill 存在的唯一理由：**在你写第一行克隆代码之前，先把"看得见的"观测穷尽、把"看不见的"推断标注，产出三份可执行文档，再交棒给 engineer-job 构建。**

### 四条核心原则

#### 原则一：观测即黑盒，推断即架构 / Observe the Black Box, Infer the Architecture

站点对你而言是一个黑盒。只有**客户端可观测**的部分——UI、交互、客户端可见的 API 契约——才能被精确复现；后端逻辑、数据库结构、服务端算法只能被**推断到行为等价**，永远不能被"复制"。

因此每一条克隆资产都要落入三个保真度层级之一：

| 层级 / Tier | 标签 | 含义 |
|:----------|:-----|:-----|
| 可观测精确 / Observable-exact | `可观测精确` | 浏览器直接可见/可抓：DOM、样式、交互、2xx 响应体、客户端可见的 API 契约。可 1:1 复现。 |
| 推断 / Inferred | `推断` | 从可观测行为反推的服务端逻辑：校验规则、错误状态、排序算法、状态机。复现到行为等价，不是源码等价。 |
| 不可观测 / Unobservable | `不可观测` | 无法从客户端观测：支付网关内部、第三方 SaaS 内部、密钥、纯服务端算法。用 mock/占位替代，禁止臆造。 |

**Every cloned asset is labeled `可观测精确 / Observable-exact`, `推断 / Inferred`, or `不可观测 / Unobservable`. Honesty about the tier is the deliverable.**

#### 原则二：覆盖靠账本，不靠记忆 / Coverage via Ledger, Not Memory

"全功能覆盖"不能靠对话记忆——长会话必然遗漏页面、状态、边角交互。覆盖靠一份持久化账本：

- `.agents/clone.ledger.json` —— 记录每一个已发现的路由/视图/交互/契约及其保真度层级与观测状态（schema 见 `references/coverage-ledger.schema.json`）。
- **loop-until-dry** —— 反复遍历，直到账本中不再新增未观测节点。
- **coverage critic** —— 每轮遍历后由覆盖批判者复查账本，追问"还有哪些入口/状态没走到"。

**Coverage is a file, not a memory. The ledger + loop-until-dry + a coverage critic guarantee 全功能 coverage.**

#### 原则三：状态即文件 / 交棒复用 / State Is Files, Reuse the Chain

本 skill **只做前置观测与文档合成**，不重造轮子。产出三份（+ 一份保真度报告）文档后，交棒给 `engineer-job`，由它负责脚手架、架构编排、里程碑开发、集成验收、部署配置。

- `REQUIREMENTS.md` / `CONTEXT.md` / `FRONTEND-DESIGN.md` —— engineer-job 链原生消费的三文档。
- `CLONE-FIDELITY.md` —— 逐资产的保真度报告，供验收阶段对照。

即使会话上下文全部丢失，从账本 + 四份文件也能完全恢复克隆进度。**Produce the docs, hand off to `engineer-job`; do not re-implement scaffolding / orchestration / acceptance.**

#### 原则四：授权先行 / Authorization First

在任何**带凭据的访问**发生之前，必须先确认合法授权，并划定"禁止克隆边界"。

- **授权确认**：用户是否对该站点拥有合法授权（本人所有 / 书面授权 / 明确许可）？无授权 → 停在 Phase 0，不做任何登录访问。
- **禁止克隆边界（do-not-clone）**：支付网关、第三方 SaaS、密钥/凭据、受版权保护的媒体资产，一律标 `不可观测` 并 mock/占位，绝不镜像原始资产。

本 skill 做的是**功能重建**（设计语言重建 + 现代技术栈重写），**不是原始资产拷贝**。**This skill does functional reconstruction — design-language reconstruction + modern-stack rebuild — never raw-asset copying.**

---

## 🚦 触发条件 / When to Trigger

**必须触发**此 skill 当用户表现出以下信号：

- "克隆这个网站"、"复刻这个站点"、"逆向这个网站"、"照着这个站点做一个"
- "clone this site"、"reverse-engineer this site"、"rebuild this website"、"replicate this site"
- 用户给出**线上站点地址 + 登录账号**并表达"重建/复刻/照做一个"的意图

**不触发**（这些交给别的技能）：

- **从零做一个新项目、没有克隆目标** → 交给 `engineer-job`（全链路构建），不属于逆向克隆。
- **分析本地已有代码库的架构** → 交给 `engineer-architect` 的逆向分析模式（读代码，不是读线上站点）。

**优先级规则**：

1. **目标 URL + 克隆意图** → **优先触发 engineer-cloner**（先于 engineer-architect / engineer-job）。
2. cloner 在 Phase 6 内部**调用 engineer-job**完成实际构建——cloner 是前置引擎，job 是构建引擎。
3. 若用户只想要"蓝图不想构建"，cloner 产出三文档后可停在交棒点，或改调 `engineer-orchestrator`（仅蓝图驱动开发）。

---

## ⚙️ 模式选择 / Mode Selection

通过 `--mode` 参数控制自动确认程度（默认 normal）：

| 模式 | 行为 |
|:----:|------|
| normal | 每个阶段展示观测结果后等待用户确认，账本变更逐轮报告 |
| auto | 使用默认策略自动推进遍历与提取，仅在重大异常（反爬阻断、契约冲突）时暂停 |
| silent | 全部自动，静默执行遍历与合成，仅在终止级失败时暂停，末尾输出交棒摘要 |

> **⚠️ Phase 0 永不跳过 / Phase 0 is never skipped**：
> **auto 和 silent 模式都不会跳过 Phase 0 的授权确认与"禁止克隆边界"确认。**
> 无论何种模式，带凭据访问之前必须先确认合法授权 + do-not-clone 边界。这是不可协商的安全门禁，`--auto` / `--silent` 只影响其后各阶段的自动化程度，绝不豁免 Phase 0。

---

## 🏗️ 逆向流水线 / Reverse Pipeline

七个固定阶段，账本贯穿始终。

### 阶段总览 / Phase Overview

| 阶段 | 名称 | 输入 → 输出 | 关键动作 |
|:----:|:-----|:-----------|:---------|
| 0 | **授权与范围** | 目标 URL + 账号 → 授权确认 + 范围清单 | 确认合法授权；划定 do-not-clone 边界；界定站点范围与账号权限 |
| 1 | **侦察** | 授权范围 → 站点指纹 | 匿名踩点：技术栈指纹、路由发现、公开页面结构，写入账本初始节点 |
| 2 | **登录全站遍历** | 站点指纹 + 账号 → 覆盖账本 | 登录后 loop-until-dry 遍历所有路由/视图/交互/状态，截图取证，账本跑干 |
| 3 | **契约与数据模型提取** | 覆盖账本 → API 契约 + 数据模型 | 抓取网络请求/响应，提取客户端可见契约，反推数据模型，标注保真度层级 |
| 4 | **设计语言提取** | 覆盖账本 + 截图 → 设计令牌 | 提取色彩/排版/间距/组件/布局/动效，重建设计语言（非资产拷贝） |
| 5 | **三文档合成** | 以上全部 → 三文档 + 保真度报告 | 合成 REQUIREMENTS.md / CONTEXT.md / FRONTEND-DESIGN.md + CLONE-FIDELITY.md |
| 6 | **交棒** | 三文档 → 构建 + 截图验收 | 调用 engineer-job 构建；重建视图与 Phase 2 截图逐一比对，产出迭代清单 |

### 阶段间数据流 / Data Flow

```
Phase 0  scope           目标 URL + 账号 + 授权 + do-not-clone 边界
   │
   ▼
Phase 1  fingerprint     技术栈指纹 + 路由种子  ─┐
   │                                            │ 写入
   ▼                                            ▼
Phase 2  ledger          .agents/clone.ledger.json（loop-until-dry 跑干）
   │                     .agents/clone.observations/（截图/HAR/DOM 取证）
   ▼
Phase 3  contracts       客户端可见 API 契约 + 推断数据模型（标层级）
   │
   ▼
Phase 4  design          设计令牌（色彩/排版/间距/组件/布局）
   │
   ▼
Phase 5  three docs      REQUIREMENTS.md ─┐
                         CONTEXT.md       ├─→ engineer-job 消费
                         FRONTEND-DESIGN.md ┘
                         CLONE-FIDELITY.md（逐资产保真度）
   │
   ▼
Phase 6  build + accept  engineer-job 构建 → 重建视图 ⇄ Phase 2 截图 → 迭代清单
```

### loop-until-dry 遍历（阶段 2 核心）

```
# Phase 2 的核心循环：反复遍历，直到账本"跑干"（不再新增未观测节点）
seed_frontier ← Phase 1 发现的路由种子
将 seed_frontier 全部写入 ledger（coverage_status = "discovered"）

repeat:
    new_nodes ← 0
    for each node in ledger where coverage_status == "discovered":
        用 agent-browser 打开该 node（登录态）
        截图 + 抓 HAR + 记录 DOM 关键结构 → .agents/clone.observations/
        标注保真度层级（可观测精确 / 推断 / 不可观测）
        发现的子路由 / 交互 / 状态 / 表单 / 空态与错误态:
            if 未在 ledger 中:
                加入 ledger（coverage_status = "discovered"）
                new_nodes += 1
        将该 node 置为 coverage_status = "explored"

    # coverage critic 复查
    critic 追问：还有哪些入口 / 状态机分支 / 分页 / 权限视图 / 空态没走到？
    critic 发现的遗漏节点写入 ledger（coverage_status = "discovered"）
    new_nodes += critic 新增数

until new_nodes == 0        # 账本跑干：无新增未观测节点

assert 每个 node 都有 coverage_status == "explored" 且带保真度层级
```

### 阶段 0: 授权与范围 / Authorization & Scope

**动作**：确认用户对目标站点拥有合法授权；明确 do-not-clone 边界（支付网关、第三方 SaaS、密钥、版权媒体）；界定克隆范围（哪些子域/路径在内）与账号权限层级。
**输出**：授权确认记录 + 范围清单（写入 `.agents/clone.ledger.json` 的 meta 段）。
**门禁**：无授权 → 立即停止，不做任何登录访问。**此阶段 auto/silent 均不跳过。**

### 阶段 1: 侦察 / Reconnaissance

**动作**：匿名（未登录）踩点——技术栈指纹识别、公开路由发现、站点结构初探，生成路由种子。
**输出**：站点指纹 + 路由种子，作为账本初始节点（coverage_status = "discovered"）。
**参考**：观测方法见 `references/observation-playbook.md`；账本结构见 `references/coverage-ledger.schema.json`。

### 阶段 2: 登录全站遍历 / Authenticated Full-Site Traversal

**动作**：用 `agent-browser` 以授权账号登录，执行上文 loop-until-dry 遍历，逐节点截图 / 抓 HAR / 记录 DOM，coverage critic 复查遗漏，直到账本跑干。
**输出**：完整覆盖账本 + `.agents/clone.observations/` 取证目录，每节点带保真度层级。
**参考**：`references/observation-playbook.md`（遍历策略、SPA 路由处理、状态取证）；`references/coverage-ledger.schema.json`（账本 schema）。

### 阶段 3: 契约与数据模型提取 / Contract & Data-Model Extraction

**动作**：从遍历抓取的网络流量中提取**客户端可见的 API 契约**（请求/响应形状、状态码、分页、错误体），反推数据模型与状态机，逐项标注 `可观测精确 / 推断 / 不可观测`。
**输出**：API 契约清单 + 推断数据模型（写入账本，供 CONTEXT.md 合成）。
**参考**：`references/contract-extraction.md`（契约提取、错误态推断、mock 边界）。

### 阶段 4: 设计语言提取 / Design-Language Extraction

**动作**：从截图与计算样式中提取设计令牌——色彩、排版、间距节奏、组件形态、布局栅格、交互动效——**重建设计语言**，而非镜像原始资产（受版权媒体用占位替代）。
**输出**：设计令牌集，供 FRONTEND-DESIGN.md 合成。
**参考**：`references/observation-playbook.md`（样式取证）；受版权资产处理见原则四。

### 阶段 5: 三文档合成 / Three-Document Synthesis

**动作**：将账本 + 契约 + 数据模型 + 设计令牌合成 engineer-job 链原生消费的三文档，并逐资产产出保真度报告。
**输出**：`REQUIREMENTS.md`（要做什么）、`CONTEXT.md`（怎么做/技术栈/数据模型/契约/里程碑）、`FRONTEND-DESIGN.md`（页面树/组件树/设计令牌）、`CLONE-FIDELITY.md`（逐资产保真度）。
**参考**：`references/fidelity-report-template.md`（CLONE-FIDELITY.md 模板与三层级标注规范）。

### 阶段 6: 交棒 / Handoff

**动作**：调用 `engineer-job` 用三文档构建克隆项目；构建完成后，将重建视图与 Phase 2 截图逐一比对，产出截图差异迭代清单。
**输出**：可运行的克隆项目 + 视觉差异迭代清单。
**详情**：见下文「🤝 交棒 / Handoff」。

---

## 📁 产物与文件结构 / Artifacts

| 产物 | 用途 |
|:-----|:-----|
| `.agents/clone.ledger.json` | 覆盖账本——所有路由/视图/交互/契约节点 + 保真度层级 + 观测状态；loop-until-dry 与恢复的唯一真相源（schema 见 `references/coverage-ledger.schema.json`） |
| `.agents/clone.observations/` | 取证目录——Phase 2 逐节点的截图 / HAR / DOM 快照，供 Phase 6 视觉比对 |
| `REQUIREMENTS.md` | 需求文档——克隆目标的角色旅程、功能清单、状态机、验收条件（engineer-job Phase 1 产物位，此处由 cloner 生成） |
| `CONTEXT.md` | 架构蓝图——技术栈、数据模型、API 契约、架构模式、里程碑（engineer-job 架构师读取） |
| `FRONTEND-DESIGN.md` | 前端设计——页面树、组件树、设计令牌、UI 状态机（engineer-job 前端架构师读取） |
| `CLONE-FIDELITY.md` | 保真度报告——逐资产标注 `可观测精确 / 推断 / 不可观测`，验收阶段对照 |

---

## 🤝 交棒 / Handoff

Phase 5 产出三文档后，**Phase 6 调用 `engineer-job`** 用这些文档完成脚手架、架构编排、里程碑开发、集成验收与部署配置——cloner 不重造这条链。若用户只想要蓝图驱动开发（已有脚手架），可改调 `engineer-orchestrator`。

Phase 6 的验收把**重建后的视图**与 **Phase 2 截取的原站截图**逐一比对，把差异写成迭代清单，交给 engineer-job 的自愈循环收敛。

**具体交棒调用**：

```javascript
Workflow({
  script: "skills/engineer-job/run.wf.js",
  args: { requirements: "见 REQUIREMENTS.md（由 cloner 生成）", mode: "auto", projectName: "<clone-name>", skip_requirements: false, skip_frontend: false }
})
```

> engineer-job 会消费 cloner 生成的 `REQUIREMENTS.md` / `CONTEXT.md` / `FRONTEND-DESIGN.md`，自动推进其 8 个 Phase。`CLONE-FIDELITY.md` 作为验收对照，`不可观测` 资产在构建中以 mock/占位实现。

---

## ⚠️ 边界情况 / Edge Cases

| 场景 | 处理方式 |
|------|---------|
| **未获授权 / 授权存疑** | 停在 Phase 0，不做任何带凭据访问。明确告知需要合法授权才能继续 |
| **反爬 / 验证码阻断** | 切换半自动：由用户人工过验证码/登录，agent-browser 复用其会话继续观测；在账本记录该节点为"半自动获取" |
| **支付 / 第三方 SaaS 页面** | 标 `不可观测`，用 mock/占位实现，绝不镜像真实支付流或第三方内部 |
| **会话中断（超时/掉线）** | 从 `.agents/clone.ledger.json` 恢复：跳过 coverage_status == "explored" 的节点，从 "discovered" 继续遍历 |
| **契约只见 2xx（错误态未触发）** | 无法观测的错误状态标 `推断`，在契约中标注"错误体为推断形状"，构建时按推断实现并在 CLONE-FIDELITY.md 说明 |
| **SPA 路由（无独立 URL）** | 用"点击序列键"（click-sequence key）作为账本节点标识，记录到达该视图的交互路径，而非依赖 URL |
| **受版权保护的媒体资产** | 用占位资产替代（尺寸/位置/角色一致），标 `不可观测`；重建设计语言而非拷贝原始媒体 |
| **重建视图与原站截图有差异** | Phase 6 将差异逐条写入迭代清单，交 engineer-job 自愈循环收敛，直至视觉逼近 |

---

## 🚫 非目标 / Non-Goals

1. **不镜像原始资产** —— 不做整站资产下载/镜像；做的是功能与设计语言的重建。
2. **不内置浏览器** —— 观测能力复用 `agent-browser`，本 skill 不实现浏览器自动化。
3. **不重造构建链** —— 脚手架/架构/编排/验收/部署复用 `engineer-job` 链，cloner 只做前置观测与文档合成。
4. **不克隆支付 / 第三方 / 密钥 / 纯服务端逻辑** —— 这些一律标 `不可观测` 并 mock/占位。
5. **绝不声称复制了后端源码** —— 不可观测部分只做行为等价推断，保真度报告如实标注。
6. **不做无授权访问** —— 没有合法授权就不启动带凭据的观测，Phase 0 门禁不可豁免。
