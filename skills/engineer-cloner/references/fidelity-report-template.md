# fidelity-report-template — CLONE-FIDELITY.md 模板 / CLONE-FIDELITY.md Template

> engineer-cloner 的 **Phase 5（三文档合成）** 产物之一 `CLONE-FIDELITY.md` 的填空模板。逐资产诚实标注三层保真度，供 **Phase 6 交棒验收**对照。上游读 `SKILL.md` 的逆向流水线与原则一（三层级定义），保真度标签来源于 `references/contract-extraction.md` 的诚实标注（Phase 3），设计资产保真度来源于 Phase 4 设计语言提取。
>
> Fill-in template for `CLONE-FIDELITY.md`, one of the **Phase 5 (Three-Document Synthesis)** outputs of engineer-cloner. Grades every cloned asset with one of the three honesty tiers, for **Phase 6 handoff acceptance** to check against. Tier labels are defined in `SKILL.md` Principle 1; per-asset tags flow in from `references/contract-extraction.md` (Phase 3 honesty tagging) and Phase 4 design-language extraction.
>
> **使用方式 / How to use**：复制本模板正文（下方分隔线之后的部分）到克隆项目根目录的 `CLONE-FIDELITY.md`，逐一替换 `[ ... ]` 占位为真实观测/推断内容。方括号占位是**必填项**——留空即视为未完成合成。
>
> **一句话原则 / One-line rule**：抓到的写"抓到"，猜到的写"猜到"，看不到的写"看不到"。任何把 `推断` 冒充 `可观测精确` 的标注，都会在 Phase 6 验收时暴露，并违背 `SKILL.md`「绝不声称复制了后端」的红线。

---

<!-- ↓↓↓ 以下为 CLONE-FIDELITY.md 正文模板，复制到克隆项目根目录后逐项填空 / Template body below — copy to clone project root and fill in ↓↓↓ -->

# CLONE-FIDELITY.md — 克隆保真度报告 / Clone Fidelity Report

> 本报告逐资产标注克隆保真度，诚实区分**可观测精确复现**、**推断到行为等价**、**不可观测需 mock/占位**三种情况。它不是营销文档——它是给 Phase 6 验收和后续迭代看的诚实账本。
>
> This report grades every cloned capability by fidelity tier, honestly separating **observable-exact reproduction**, **inference to behavioral equivalence**, and **unobservable mock/placeholder**. It is an honesty ledger for Phase 6 acceptance and later iteration — not a marketing document.

## 头部信息 / Header Block

| 字段 / Field | 值 / Value |
|:------------|:----------|
| 目标 URL / Target URL | `[目标 URL，例如 https://app.example.com]` |
| 观测日期 / Observation date | `[YYYY-MM-DD，例如 2026-07-14]` |
| 账号角色 / Account role | `[登录账号的权限层级，例如 管理员 / admin、普通用户 / member、只读 / viewer]` |
| 观测模式 / Modes | `[--mode 值：normal / auto / silent；如有半自动过验证码请注明]` |
| 覆盖账本 / Coverage ledger | `[.agents/clone.ledger.json 快照标识或 commit hash]` |
| 授权依据 / Authorization | `[本人所有 / 书面授权 / 明确许可 —— 对应 Phase 0 授权确认记录]` |

---

## 保真分级 / Fidelity Tiers

克隆的每一条资产必须落入以下三个层级之一（定义同 `SKILL.md` 原则一）。层级是本报告的**交付本质**：诚实地说清哪些是抓到的、哪些是猜到的、哪些是看不到的。

Every cloned asset falls into exactly one of the three tiers below (defined in `SKILL.md` Principle 1). Honesty about the tier is the deliverable.

| 层级 / Tier | 含义 / Meaning | 克隆处理 / Clone Handling |
|:-----------|:--------------|:------------------------|
| **可观测精确 / Observable-exact** | 浏览器直接可见/可抓：DOM、样式、交互、2xx 响应体、客户端可见的 API 契约。可 1:1 复现。 / Directly visible or captured in the browser: DOM, styles, interactions, 2xx response bodies, client-visible API contracts. Reproducible 1:1. | 精确复现，视觉与行为逐一对齐 Phase 2 取证。 |
| **推断 / Inferred** | 从可观测行为反推的服务端逻辑：校验规则、错误状态、排序算法、状态机、关系基数、字段必填性。复现到行为等价，不是源码等价。 / Server logic reverse-inferred from observable behavior: validation rules, error states, sort algorithms, state machines, relationship cardinality, field requiredness. Reproduced to behavioral equivalence, not source equivalence. | 按推断实现，明确标注"这是猜的"，可升级：后续触发并抓到真实响应后回填并改标 `可观测精确`。 |
| **不可观测 / Unobservable** | 客户端根本看不到：支付网关内部、第三方 SaaS 内部、密钥/凭据、纯服务端算法、数据库物理 schema。 / Invisible from the client: payment-gateway internals, third-party SaaS internals, secrets/credentials, pure server-side algorithms, physical database schema. | 用 mock/占位替代，**禁止臆造**，绝不镜像原始资产或声称复制了后端。 |

> **标注纪律 / Tagging discipline**：一项一签，拿不准就下沉（`推断` 优先于 `可观测精确`，涉及服务端内部一律 `不可观测`）。诚实的下限比乐观的上限安全。

---

## 能力清单 / Capability Inventory

逐功能标注保真度、观测依据与克隆处理方式。**每个层级至少一行示例**（下表首三行为示例格式，替换为真实条目后删除"示例"标记）。

Grade every capability by tier, evidence, and clone handling. Provide at least one row per tier (the first three rows below are format examples — replace with real entries and drop the "example" marker).

| 功能 / Feature | 分级 / Tier | 依据 / Evidence | 克隆处理 / Clone Handling |
|:--------------|:-----------|:---------------|:------------------------|
| `[示例] 登录 → 仪表盘 UI 流程 / Login → dashboard UI flow` | 可观测精确 / Observable-exact | `[.agents/clone.observations/dashboard.png + DOM 快照；交互路径已录]` | `[1:1 复现页面树/组件树/交互，逐屏对齐 Phase 2 截图]` |
| `[示例] 注册表单邮箱格式校验 / Signup email-format validation rule` | 推断 / Inferred | `[提交非法邮箱触发 400，错误文案已抓；跨字段规则未观测]` | `[前端按抓到的文案实现，服务端规则按推断封套实现，标注为推断]` |
| `[示例] 每夜对账定时任务 / 第三方支付集成 / Nightly reconciliation job / third-party payment integration` | 不可观测 / Unobservable | `[客户端无任何可观测信号；仅从 UI 措辞推知其存在]` | `[mock/占位实现（占位 webhook + 假数据），绝不镜像真实支付流]` |
| `[真实条目，替换我 / real entry]` | `[可观测精确 / 推断 / 不可观测]` | `[观测取证引用或推断依据]` | `[精确复现 / 推断实现 / mock 占位]` |

> 契约与数据模型的逐项保真度（method/path/状态码/字段/关系）由 Phase 3 产出，格式与升降级规则见 `references/contract-extraction.md` 的「诚实标注」节，汇总进上表对应功能行。

---

## 不克隆边界 / Do-Not-Clone Boundary

以下类别一律标 `不可观测`，用 mock/占位实现，**绝不镜像原始资产**（对应 `SKILL.md` 原则四的 do-not-clone 边界）。逐项确认本次克隆如何处置。

The categories below are always tagged `不可观测` and implemented as mock/placeholder — **never mirror the raw asset** (per `SKILL.md` Principle 4). Confirm the disposition of each for this clone.

| 边界类别 / Boundary category | 本站是否涉及 / Present here | 占位/Mock 处置 / Placeholder handling |
|:----------------------------|:--------------------------|:------------------------------------|
| 支付网关 / Payment gateways | `[是/否 —— 例如 Stripe Checkout 跳转]` | `[占位支付页 + 假成功回调，标 不可观测，绝不接真实网关]` |
| 第三方 SaaS / Third-party SaaS | `[是/否 —— 例如嵌入的客服/分析/地图组件]` | `[占位组件替代，记录其角色/位置/尺寸，不镜像其内部]` |
| 密钥/凭据 / Secrets & credentials | `[是/否 —— API key、token、私钥]` | `[全部用 .env 占位符，绝不复制任何真实凭据]` |
| 受版权媒体 / Copyrighted media | `[是/否 —— logo、图片、字体、视频]` | `[占位资产替代（尺寸/位置/角色一致），重建设计语言而非拷贝原始媒体]` |
| `[其他不可观测边界 / other boundary]` | `[是/否]` | `[处置方式]` |

---

## 已知差距 / Known Gaps

诚实记录尚未收敛的差距，作为迭代工作项交给 Phase 6 之后的自愈循环。分两类：**截图差异**（可观测精确层的视觉未对齐）与**未决推断**（推断层待触发/待验证）。

Honestly record unresolved gaps as iteration work items carried into the post-Phase-6 self-healing loop. Two kinds: **screenshot diffs** (observable-exact visual mismatches) and **unresolved inferences** (inferred items awaiting trigger/verification).

### 截图差异 / Screenshot-diff items

Phase 6 将重建视图与 Phase 2 原站截图逐一比对，未对齐项列此。

| 视图/组件 / View or component | 差异描述 / Diff description | 原站取证 / Original evidence | 迭代动作 / Iteration action |
|:-----------------------------|:--------------------------|:---------------------------|:--------------------------|
| `[例如 订单列表分页控件 / order-list pagination]` | `[间距/字号/颜色/布局差异，越具体越好]` | `[.agents/clone.observations/orders.png 区域]` | `[调整 token / 重排组件，交 engineer-job 自愈循环收敛]` |
| `[真实差异条目 / real diff]` | `[...]` | `[...]` | `[...]` |

### 未决推断 / Unresolved inferences

推断层中尚未升级为 `可观测精确` 的项，携带到迭代继续验证或主动触发。

| 推断项 / Inferred item | 当前依据 / Current basis | 升级路径 / Path to upgrade | 状态 / Status |
|:----------------------|:------------------------|:--------------------------|:-------------|
| `[例如 Order.status 枚举全集 / full Order.status enum]` | `[仅观测到 pending/paid，全集未知]` | `[回 Phase 2 触发更多状态转移，抓到后回填并改标 可观测精确]` | `[待触发 / pending]` |
| `[例如 库存冲突 409 错误体 / stock-conflict 409 body]` | `[纯推断，服务端约束不可观测]` | `[尝试并发下单触发 409；若无法从客户端触发则保持 推断]` | `[待验证 / to verify]` |
| `[真实未决推断 / real inference]` | `[...]` | `[...]` | `[...]` |

---

> **验收提醒 / Acceptance note**：Phase 6 用本报告逐资产核对——`可观测精确` 项必须视觉/行为对齐原站，`推断` 项必须行为等价且标注清楚，`不可观测` 项必须为 mock/占位且未镜像任何真实资产。任何名不副实的标注都应在此暴露并回退修正。
