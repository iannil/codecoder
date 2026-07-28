---
name: engineer-legacy-recon
description: >
  【强触发 / Strong trigger】用户**粘贴了历史遗留系统的页面内容 + 导航菜单**（或截图 / 导出的 HTML），并想理解、复刻或重建它 —— 把这些既有材料当作唯一真相源，静态推断功能，产出三文档，交棒 engineer-job 构建。
  ROUTING RULE: 当用户**已经提供了页面文本/菜单/截图/HTML**时，本技能优先触发；**优先根据这些既有材料推测功能，不要动用 agent-browser 去线上抓取**。只有在用户明确要求"对照线上核实"且拥有授权账号时，才改调 engineer-cloner（它用 agent-browser 逆向线上站点）。
  AI 遗留系统静态侦察前置引擎 — 仅凭用户提供的页面内容与导航菜单，
  离线推断出模块地图、实体字段、操作、状态机、角色权限，
  产出 REQUIREMENTS.md / CONTEXT.md / FRONTEND-DESIGN.md / RECON-FIDELITY.md，
  再交棒 engineer-job 完成全功能重建。观测不联网，只读既有材料。
  TRIGGERS: "这是遗留系统的页面内容""照着这些菜单/页面做一个""帮我把这个老系统复刻/重写"
  "根据这些截图推测功能""我把后台各页面贴给你，重建它""分析这个系统能做什么"
  "legacy system pages""rebuild from these screenshots""reverse this admin panel from pasted content"
  也触发于：用户在对话里粘贴了成体系的页面文本/表格/菜单树并表达"理解/复刻/重建"意图。
  不触发：用户只给线上 URL + 账号要"精确克隆" → 交给 engineer-cloner（agent-browser 观测）。
compatibility: "bash, write, read, edit"
---

# engineer-legacy-recon — AI 遗留系统静态侦察前置引擎 / AI Legacy-System Static-Recon Front-End

> **来源声明**: 本 skill 的方法论来源于《基于实现规划的 AI 辅助编程实战》，是 `engineer-cloner` 的离线兄弟技能。更多内容请访问 [zhurongshuo.com]。
>
> **Source**: Methodology from "AI-Assisted Programming Practice Based on Implementation Planning". This is the offline sibling of `engineer-cloner`.
>
> **参考架构**: 观测层**只读用户提供的既有材料**（粘贴文本 / 导航菜单 / 截图 / 导出 HTML），不联网、不动用 `agent-browser`；构建层复用 `engineer-job` 链（脚手架 → 架构 → 编排开发 → 验收 → 部署）。

---

## 🎯 核心理念 / Core Philosophy

有时你根本进不去那个线上系统——没账号、没授权、服务器在内网、或者它早就下线了，你手上只有**别人贴给你的一堆页面文字和菜单树**。

这时不该去联网爬。**用户既然把材料贴出来了，那堆材料就是你的真相源。** 你的工作是把它读穿：从一列表头推断出实体字段，从一组标签页推断出状态机，从一排按钮推断出操作与权限，从"共 227202 条"推断出数据规模。然后诚实地把"白纸黑字写着的"和"我据此猜的"和"材料里根本没提的"分清楚。

**Read what you were handed to the bone; infer the rest to behavioral equivalence; never browse live to fill the gaps the user could just paste.**

这个 skill 存在的唯一理由：**在写第一行重建代码之前，仅凭既有材料把"看得见的"抽干、把"看不见的"标为缺口，产出三份可执行文档，再交棒 engineer-job。**

### 四条核心原则

#### 原则一：既有材料即真相源，不联网 / Given Material Is Ground Truth, Don't Browse

用户提供的页面内容与导航菜单是本技能**唯一**的观测输入。不启动浏览器、不发网络请求、不去线上核实——因为遗留系统往往无法访问，而**盲目联网会用臆造的"线上现状"污染用户明确给出的材料**。

若某处信息在材料里缺失且确实关键，正确做法是**标为缺口并向用户追问**（"这个页面的表单字段你能补贴一下吗？"），而不是自己去猜一个线上版本。

> **唯一例外**：用户**明确说**"去线上核实一下"且拥有合法授权账号 —— 这时这已经不是本技能的活儿了，改调 `engineer-cloner`（它专做 agent-browser 线上逆向）。本技能的边界就是"只读既有材料"。

#### 原则二：三层可信度，诚实分级 / Three Confidence Tiers, Honest Grading

材料是静态文字，比"浏览器实时可观测"更弱。每一条推断资产必须落入三层可信度之一：

| 层级 / Tier | 标签 | 含义 |
|:----------|:-----|:-----|
| 明示 / Stated | `明示` | 材料里白纸黑字写着：菜单名、表头列、按钮文案、筛选项、统计数字、表单字段、状态标签。可直接采信其**存在**。 |
| 推断 / Inferred | `推断` | 从明示证据合理反推：一列"编辑/恢复/禁用"按钮 → CRUD + 软删除状态机；一组"待审核/已通过/已驳回"标签页 → 审批状态机；身份证号+手机号列 → 学员实体。行为等价，非确证。 |
| 缺口 / Gap | `缺口` | 材料里根本看不到：后端算法、精确校验规则、分页/排序内部、完整权限矩阵、支付内部、字段类型细节。**标为缺口 + 向用户追问**，构建时 mock/占位，禁止臆造。 |

**明示的写"写着"，推断的写"猜的"，缺口的写"没提"。诚实分级本身就是交付物。** 任何把 `推断` 冒充 `明示` 的标注都会在重建验收时暴露。

#### 原则三：覆盖靠账本，导航树即种子 / Coverage via Ledger, Nav-Tree Is the Seed

遗留系统的导航菜单**本身就是一张路由地图**——这是静态侦察相对线上爬取的最大优势：cloner 要靠遍历发现路由，而这里路由树已经端到用户手里了。但"读完每个页面"不能靠记忆——长材料必然遗漏叶子节点、遗漏表头列、遗漏按钮。覆盖靠一份持久化账本：

- `.agents/recon.ledger.json` —— 把导航树每个叶子作为节点，记录其明示要素（实体/字段/操作/状态/统计）与提取状态（schema 见 `references/recon-ledger.schema.json`）。
- **loop-until-dry** —— 反复遍历账本，直到不再新增未提取节点、也不再从已提取页面里挖出新要素。
- **coverage critic** —— 每轮复查：哪些菜单叶子还没有对应的页面材料？哪些明示的表头列/按钮还没映射到实体/操作/状态？哪些页面之间的跳转关系还没连起来？

**覆盖是一份文件，不是记忆。导航树作种子 + loop-until-dry + coverage critic，保证全功能覆盖。**

#### 原则四：状态即文件 / 交棒复用 / State Is Files, Reuse the Chain

本 skill **只做前置静态侦察与文档合成**，不重造轮子。产出三文档（+ 一份可信度报告）后，交棒给 `engineer-job` 负责脚手架、架构编排、里程碑开发、集成验收、部署。

- `REQUIREMENTS.md` / `CONTEXT.md` / `FRONTEND-DESIGN.md` —— engineer-job 链原生消费的三文档。
- `RECON-FIDELITY.md` —— 逐资产可信度报告（明示/推断/缺口）+ 待用户补全的缺口清单。

即使会话上下文全部丢失，从账本 + 四份文件也能完全恢复侦察进度。**产出文档，交棒 `engineer-job`；不重实现脚手架/编排/验收。**

---

## 🚦 触发条件 / When to Trigger

**必须触发**此 skill 当用户表现出以下信号：

- 在对话里**粘贴了成体系的页面文本 / 表格 / 导航菜单树**（像本仓库 `engineer-requirements` 里的 AOPA 示例那样），并说"照着做/复刻/重建/理解它"。
- 提供了后台**截图 / 导出的 HTML / 前端代码片段**，要从中还原功能。
- "这是老系统的各个页面""我把菜单和页面贴给你""根据这些推测功能""帮我把这个后台重写一遍"。

**不触发**（交给别的技能）：

| 场景 | 交给谁 |
|------|--------|
| 只给**线上 URL + 账号**要精确克隆，需实时观测 | `engineer-cloner`（agent-browser 线上逆向） |
| 从零做一个**全新**项目、没有任何遗留材料 | `engineer-job`（全链路构建） |
| 分析**本地已有代码库**的架构（读源码，不是读页面材料） | `engineer-architect` 逆向分析模式 |

**优先级规则**：

1. **既有页面材料/菜单树已提供** → **优先触发 engineer-legacy-recon**，静态推断，**不动用 agent-browser**（原则一）。
2. 三文档产出后，Phase 6 **调用 engineer-job** 完成实际构建。
3. 用户只想要蓝图不想构建 → 停在交棒点，或改调 `engineer-orchestrator`（仅蓝图驱动开发）。
4. 用户明确要"对照线上核实"且有授权账号 → 越界了，改调 `engineer-cloner`。

---

## ⚙️ 模式选择 / Mode Selection

通过 `--mode` 控制自动确认程度（默认 normal）：

| 模式 | 行为 |
|:----:|------|
| normal | 每阶段展示提取结果后等待用户确认；缺口清单逐轮报告，主动追问关键缺失 |
| auto | 自动推进解析与提取，仅在重大歧义（如同名菜单/矛盾字段）或关键缺口时暂停追问 |
| silent | 全自动静默执行，仅在终止级问题时暂停，末尾一次性输出缺口清单 + 交棒摘要 |

> **⚠️ 缺口不静默臆造 / Gaps are never silently invented**：任何模式下，材料缺失的关键信息（表单字段、校验规则、权限矩阵）都必须落到 `RECON-FIDELITY.md` 的缺口清单，而不是自动编一个填上。auto/silent 只是减少确认交互，**不豁免"缺口如实标注"**。

---

## 🏗️ 静态侦察流水线 / Static-Recon Pipeline

六个固定阶段，账本贯穿始终。

### 阶段总览 / Phase Overview

| 阶段 | 名称 | 输入 → 输出 | 关键动作 |
|:----:|:-----|:-----------|:---------|
| 0 | **材料归集与端界定** | 粘贴内容/截图/HTML → 规范化材料库 + 端划分 | 把散落材料收拢；分清管理端/机构端/多角色端；识别材料形态与缺失 |
| 1 | **导航树解析** | 导航菜单 → 路由/模块账本种子 | 把菜单层级解析为账本节点（每个叶子=一个视图），建立模块→页面骨架 |
| 2 | **逐页要素提取** | 账本 + 各页材料 → 每页明示要素 | loop-until-dry：从表头/筛选/按钮/标签页/统计/表单抽取实体·字段·操作·状态·聚合，coverage critic 复查 |
| 3 | **实体与状态机推断** | 明示要素 → 数据模型 + 状态机 + 角色权限 | 跨页归并实体，反推关系与状态机，划角色可见性，逐项标可信度层级 |
| 4 | **设计与信息架构线索** | 截图/HTML/布局 → 设计令牌线索 + 页面树 | 从可得材料提取布局/组件/列表-详情模式；材料不足处标缺口（多为占位） |
| 5 | **三文档合成** | 以上全部 → 三文档 + 可信度报告 | 合成 REQUIREMENTS.md / CONTEXT.md / FRONTEND-DESIGN.md + RECON-FIDELITY.md |
| 6 | **交棒** | 三文档 → 构建 | 按规模自动路由：大型多模块系统（端≥2 或 模块≥5）先交 engineer-requirements 再顺链；否则直接交 engineer-job。缺口清单作为待补全项贯穿构建 |

### 阶段间数据流 / Data Flow

```
Phase 0  intake      规范化材料库（按端/角色分区）+ 材料形态清单
   │
   ▼
Phase 1  nav-tree    .agents/recon.ledger.json（导航树 → 节点种子）
   │                                   │ 写入
   ▼                                   ▼
Phase 2  ledger      loop-until-dry 逐页提取明示要素（跑干）
   │                 .agents/recon.material/（归档的页面材料/截图）
   ▼
Phase 3  model       实体 + 关系 + 状态机 + 角色权限（标 明示/推断/缺口）
   │
   ▼
Phase 4  design      设计令牌线索 + 页面树（列表/详情/表单/审批模式）
   │
   ▼
Phase 5  three docs  REQUIREMENTS.md ─┐
                     CONTEXT.md       ├─→ engineer-job 消费
                     FRONTEND-DESIGN.md ┘
                     RECON-FIDELITY.md（逐资产可信度 + 缺口清单）
   │
   ▼
Phase 6  handoff     engineer-job 构建（缺口作为待补全项）
```

### loop-until-dry 提取（阶段 2 核心）

```
# Phase 2 的核心循环：反复读页，直到账本"跑干"
frontier ← Phase 1 从导航树解析出的所有页面节点（extraction_status = "seeded"）

repeat:
    new_elements ← 0
    for each node in ledger where extraction_status in {"seeded","partial"}:
        取该 node 对应的页面材料（用户已提供的文本/截图/HTML）
        从材料中抽取明示要素并写入 node：
            - 表头列       → 实体候选字段
            - 筛选/搜索项   → 查询参数 + 枚举取值
            - 按钮/操作列   → 操作动词 + 潜在状态转移
            - 标签页/分段   → 状态机的状态集
            - 统计/汇总头   → 聚合指标/报表
            - 表单字段      → 输入模型 + 必填/校验线索
            - "共 N 条"/分页 → 列表规模 + 分页存在性
        逐要素标可信度层级（明示 / 推断 / 缺口）
        if 该 node 无对应材料:
            标 extraction_status = "no_material" 并加入缺口清单
        else 若仍有未消化的材料片段: extraction_status = "partial"; new_elements += n
        else: extraction_status = "extracted"

    # coverage critic 复查
    critic 追问：
        - 哪些菜单叶子还没有对应页面材料？（→ 缺口清单，向用户追问）
        - 哪些明示的表头列/按钮/标签还没映射到实体/操作/状态？
        - 页面间的跳转（列表→详情→编辑、审批流转）是否连成图？
        - 同一实体在多页出现，字段是否已归并？
    critic 发现的遗漏写回 ledger（相应 node 置回 "partial"）
    new_elements += critic 新增数

until new_elements == 0        # 账本跑干

assert 每个 node 或为 "extracted" 或为 "no_material"（后者已进缺口清单）
```

### 各阶段要点 / Phase Notes

- **阶段 0 — 材料归集与端界定**：遗留系统常有多个端（如管理端 / 机构端）和多角色。先把材料按端/角色分区归档到 `.agents/recon.material/`，识别每份材料的形态（纯文本 / 表格 / 菜单树 / 截图 / HTML）与明显缺失。**门禁**：材料过于稀薄（只有零星菜单名、无任何页面内容）→ 明确告知用户"当前材料只够画模块骨架，页面级功能需要你补贴内容"，不硬编。
- **阶段 1 — 导航树解析**：菜单层级直接映射为 `模块 → 子模块 → 页面` 的账本骨架。这是本技能最确定的一步——路由地图是白给的。参考 `references/extraction-playbook.md`「导航树解析」。
- **阶段 2 — 逐页要素提取**：执行上文 loop-until-dry。**这是"推测功能"的引擎**，全部启发式（表头→字段、按钮→操作、标签→状态、统计→报表）见 `references/extraction-playbook.md`。
- **阶段 3 — 实体与状态机推断**：跨页归并同一实体（如"学员"在学员列表、成绩、缴费、证件多页出现），反推关系基数与状态机，按端划分角色可见性。逐项标层级。参考 `references/extraction-playbook.md`「实体归并与状态机推断」。
- **阶段 4 — 设计与信息架构线索**：有截图/HTML 时提取色彩/排版/组件/布局；纯文本时至少还原**信息架构模式**（列表页 + 筛选栏 + 分页 + 行内操作 + 详情/编辑抽屉是遗留后台的通用范式）。材料不足处标缺口，构建时用现代栈默认设计语言占位。
- **阶段 5 — 三文档合成**：合成 engineer-job 原生消费的三文档 + 逐资产可信度报告。模板见 `references/fidelity-report-template.md`。
- **阶段 6 — 交棒**：按规模自动路由（大型多模块系统先交 engineer-requirements，否则直接交 engineer-job），缺口清单作为待补全项。见下文「交棒」。

---

## 📁 产物与文件结构 / Artifacts

| 产物 | 用途 |
|:-----|:-----|
| `.agents/recon.ledger.json` | 覆盖账本——导航树节点 + 每页明示要素 + 可信度层级 + 提取状态；loop-until-dry 与恢复的唯一真相源（schema 见 `references/recon-ledger.schema.json`） |
| `.agents/recon.material/` | 材料归档——按端/角色分区存放用户提供的页面文本/截图/HTML，供逐页提取与追溯 |
| `REQUIREMENTS.md` | 需求文档——角色旅程、功能清单、状态机、验收条件（engineer-job Phase 1 产物位，此处由本技能生成） |
| `CONTEXT.md` | 架构蓝图——技术栈、数据模型、推断的 API 契约、架构模式、里程碑（engineer-job 架构师读取） |
| `FRONTEND-DESIGN.md` | 前端设计——页面树、组件树、设计令牌线索、UI 状态机（engineer-job 前端架构师读取） |
| `RECON-FIDELITY.md` | 可信度报告——逐资产标注 `明示 / 推断 / 缺口` + 待用户补全的缺口清单 |

---

## 🤝 交棒 / Handoff

Phase 6 按**规模自动路由**，不再让用户手动决定走哪条链。判据取自 Phase 0 的端界定与账本模块数：

### 交棒路由规则 / Handoff Routing Rule

```
令 ends   = 识别出的端数（管理端/机构端/…）
令 modules = 账本一级模块数

if ends >= 2  或  modules >= 5:        # 大型多模块系统（如 AOPA 那种规模）
    自动先交 engineer-requirements 做深度需求拆解（Event Storming + DDD），
    再顺链进 engineer-architect → engineer-job。
    本技能产出的 REQUIREMENTS.md 正是 engineer-requirements 的天然输入——
    它在此基础上补全有界上下文、业务事件、状态机细节，而非从零开始。
else:                                   # 小型/单模块
    直接交 engineer-job（它内部含 requirements/architect 阶段）。

若用户只想蓝图驱动开发（已有脚手架）→ 改调 engineer-orchestrator。
```

> **为什么大型系统要先过 engineer-requirements**：端多、模块多、审批流交织的系统（AOPA 有 5 种证件、双端、机构审核+证件打印+物流+财务多条状态流），直接进 engineer-job 会让需求纹理在一个大 Phase 里被压平。先做一轮 Event Storming + DDD 拆解，能把角色旅程与状态机摊开，显著降低后续返工。小系统没有这个复杂度，多一跳反而是负担，故直接进 job。

**大型系统交棒调用**（ends≥2 或 modules≥5）：

```javascript
Workflow({
  script: "skills/engineer-requirements/run.wf.js",
  args: { requirements: "见 REQUIREMENTS.md（由 legacy-recon 生成，含账本与缺口清单）", mode: "auto" }
})
// engineer-requirements 产出深化后的 REQUIREMENTS.md → 顺链进 engineer-architect → engineer-job
```

**小型系统交棒调用**（否则）：

```javascript
Workflow({
  script: "skills/engineer-job/run.wf.js",
  args: { requirements: "见 REQUIREMENTS.md（由 legacy-recon 生成）", mode: "auto", projectName: "<rebuild-name>", skip_requirements: false, skip_frontend: false }
})
```

> 无论走哪条链，下游都消费本技能生成的 `REQUIREMENTS.md` / `CONTEXT.md` / `FRONTEND-DESIGN.md`。`RECON-FIDELITY.md` 的**缺口清单**在构建中以 mock/占位实现并逐项提示用户补全；`推断`项按行为等价实现并注明"待用户确认"。

---

## ⚠️ 边界情况 / Edge Cases

| 场景 | 处理方式 |
|------|---------|
| **材料只有菜单、没有页面内容** | 只画模块骨架，页面级要素全标缺口，明确告知用户"补贴各页内容才能推断功能"，不硬编 |
| **同名菜单/页面（多端重名）** | 用"端 + 菜单路径"作节点主键区分（如 `管理端/合格证管理/合格证列表` vs `机构端/合格证列表`） |
| **表头列语义不明**（如"操作""状态"泛列） | 从该列的取值/按钮文案反推语义；仍不明则标推断并在缺口清单追问 |
| **材料自相矛盾**（两页字段冲突） | 不擅自取舍，两版都记入账本并标推断，向用户追问以哪版为准 |
| **截图无法完整读取** | 尽力 OCR 可读部分，读不到的区域标缺口，请用户补文本 |
| **用户中途追加材料** | 增量写入账本，把相关 node 置回 "partial" 重跑 loop-until-dry，不推翻已提取部分 |
| **会话中断** | 从 `.agents/recon.ledger.json` 恢复：跳过 "extracted" 节点，从 "seeded"/"partial" 继续 |
| **用户要求联网核实** | 越出本技能边界——告知需改用 `engineer-cloner`（agent-browser + 授权账号），本技能只读既有材料 |

---

## 🚫 非目标 / Non-Goals

1. **不联网、不用 agent-browser** —— 观测输入仅限用户提供的既有材料；要线上逆向请用 `engineer-cloner`。
2. **不臆造缺口** —— 材料没写的关键信息标缺口 + 追问用户，绝不自动编一个填上冒充明示。
3. **不重造构建链** —— 脚手架/架构/编排/验收/部署复用 `engineer-job` 链，本技能只做前置侦察与文档合成。
4. **不声称还原了后端源码** —— 后端逻辑、算法、物理 schema 一律 `推断` 或 `缺口`，行为等价而非源码等价。
5. **不做像素级视觉克隆** —— 纯文本材料下重建的是**功能 + 信息架构 + 设计语言**，不是原站截图的像素复刻（那是 cloner 的活儿）。
