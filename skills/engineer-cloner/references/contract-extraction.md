# contract-extraction — Phase 3 契约与数据模型提取手册 / Phase 3 Contract & Data-Model Extraction

> engineer-cloner 的 **Phase 3（契约与数据模型提取）** 参考。上游读 `SKILL.md` 的逆向流水线，观测取证由 `references/observation-playbook.md`（Phase 2）产出，账本结构见 `references/coverage-ledger.schema.json`，下游合成的 CONTEXT.md 模板见 `engineer-architect/SKILL.md`。
>
> Reference for engineer-cloner **Phase 3 (Contract & Data-Model Extraction)**. Upstream: `SKILL.md` reverse pipeline. Input evidence: the `.agents/clone.observations/` captures + ledger `api_calls` produced by `references/observation-playbook.md` (Phase 2). Ledger shape: `references/coverage-ledger.schema.json`. Downstream templates: `engineer-architect/SKILL.md`.

---

## 目标 / Purpose

Phase 3 的任务：把 Phase 2 抓到的**客户端流量**，从"一堆 HAR 请求/响应"提炼成两样可交棒的东西——**API 契约**（每个客户端可见接口的形状）与**数据模型**（响应体背后的实体、字段、关系）。

**The job of Phase 3**: turn the client traffic captured in Phase 2 into two hand-offable artifacts — the **API contracts** (the shape of every client-visible endpoint) and the **data model** (the entities, fields, and relationships behind the response payloads).

沿用 `SKILL.md` 的黑盒底线：你抓到的只有**浏览器发出/收到的那一层**。因此本阶段有一条不可动摇的界线——

- **契约只对真正观测到的状态是 `可观测精确`。** 一个接口若只见过 `200` 响应，那么它的成功形状是 `可观测精确`；它的 `400 / 403 / 404 / 409 / 500` 错误体、它的服务端校验规则、它的排序算法，全部是 `推断`，除非你亲手触发并抓到了那个状态。
- **服务端源码、数据库 schema、纯服务端算法** 永远是 `不可观测`——你能推断行为等价，绝不能声称"复制了后端"。

**Contracts are `可观测精确 / Observable-exact` only for the states you actually observed.** Everything you did not trigger — unseen error bodies, server-side validation, sort algorithms — is `推断 / Inferred`. Server internals are `不可观测 / Unobservable`.

**输入 / Inputs.** 本阶段消费 Phase 2 的两处产物：

- `.agents/clone.observations/<id>.har` —— 逐节点的网络抓取（请求/响应)。
- `.agents/clone.ledger.json` 的功能条目 `api_calls` 字段 —— 指向上面 HAR 条目的引用（见 observation-playbook「记录规范」）。

**输出 / Outputs.** 提取结果写回账本节点（把 `coverage_status` 从 `explored` 推进到 `extracted`），并汇总成 CONTEXT.md 的 **核心数据字典 / Core Data Dictionary** + **API 契约 / API Contracts** + **领域词汇表 / Domain Glossary** 三节的素材，逐项带保真度层级标签，流入 `CLONE-FIDELITY.md`。

---

## API 契约重建 / API Contract Reconstruction

对账本里每一个带 `api_calls` 的节点，逐条 HAR 请求重建一份契约。契约的目标不是"复制这一次调用"，而是**归纳出这个端点的稳定形状**——把多次观测（列表页翻页、不同详情、不同筛选）聚合到同一个端点定义上。

For every ledger node carrying `api_calls`, reconstruct one contract per distinct endpoint. Aggregate multiple observations of the same endpoint into a single stable shape — do not emit one contract per HAR row.

### 每个观测请求要提取的字段 / Per observed request

| 维度 / Dimension | 提取方式 / How | 保真度 / Tier |
|:----------------|:--------------|:-------------|
| **method** 方法 | HAR 请求行的 HTTP 方法（GET/POST/PUT/PATCH/DELETE） | `可观测精确` |
| **path** 路径 | 请求 URL 的 path 部分，**把具体 ID 参数化为 `:id`**（见下） | `可观测精确` |
| **query params** 查询参数 | 从筛选/排序/分页观测中收集查询键（`?page=`、`?sort=`、`?q=`），标注每个参数的观测取值域 | `可观测精确`（见过的键）/ `推断`（未见过的取值） |
| **request body shape** 请求体形状 | 解析请求体 JSON（或表单），记录字段名 + 观测类型；必填/可选按"是否每次都出现"标注 | `可观测精确`（见过的字段）/ `推断`（必填性） |
| **response body shape** 响应体形状 | 解析成功响应体，记录字段名 + 类型 + 嵌套结构；数组元素抽象为元素形状 | `可观测精确` |
| **status codes seen** 见过的状态码 | 枚举该端点**实际抓到**的状态码；每个状态码对应一种响应体形状 | `可观测精确` |
| **auth scheme** 鉴权方式 | 从请求头/Cookie 推断：`Cookie: session=…`、`Authorization: Bearer …`、`X-API-Key`、CSRF token 头 | `可观测精确`（机制）/ `推断`（服务端校验逻辑） |

### 路径参数化 / Parameterizing IDs into `:id`

同一端点在不同实体上会产生不同 URL——`/api/orders/8f3a`、`/api/orders/9b21`。**把可变的 ID 段收敛为 `:id`**，否则每条详情都会被误当成一个独立端点，账本爆炸且契约无法归纳。

Collapse the variable ID segment into `:id` so repeated detail hits fold into one endpoint:

```
观测到 / Observed:
  GET /api/v1/orders/8f3a2c
  GET /api/v1/orders/9b21ff
  GET /api/v1/orders/8f3a2c/items

归纳契约 / Reconstructed contract:
  GET /api/v1/orders/:id
  GET /api/v1/orders/:id/items
```

- 判定"这是 ID 段"的启发式：段值在多次同结构请求间变化、且形如 UUID / 数字自增 / slug / hash。
- 嵌套资源保留层级：`/orders/:id/items/:itemId`，用不同占位名区分。
- 若一个段**始终不变**（如 `/api/v1/orders`），它是资源名，不参数化。

### 未见错误状态标 `推断` / Unseen error states are Inferred

契约里"状态码"一栏只列**真正抓到**的码。绝大多数遍历只会自然触发 `2xx`（有时加上偶发的 `401` 会话过期）。因此：

- **见过的状态码 → `可观测精确`**，响应体形状按实抓记录。
- **没见过但业务上必然存在的错误态**（`400` 校验失败、`403` 越权、`404` 不存在、`409` 冲突、`422` 语义错误、`500` 服务端错）→ 在契约里**列为 `推断`**，响应体形状标注"推断形状，未观测"，并按同域端点的已观测错误体推测统一封套。
- 这条与 `SKILL.md` 边界情况「契约只见 2xx（错误态未触发)」一致：无法观测的错误状态标 `推断`，构建时按推断实现，并在 `CLONE-FIDELITY.md` 说明。

> 主动触发能把 `推断` 升为 `可观测精确`：回到 Phase 2 的入口清单，故意提交非法表单拿 `400`、访问越权资源拿 `403`、请求不存在 ID 拿 `404`。凡触发成功、抓到真实错误体的，改标 `可观测精确` 并回填响应形状。凡无法从客户端触发的（如 `500`），保持 `推断`。

### 单条契约输出格式 / Per-contract output shape

~~~markdown
#### `POST /api/v1/orders`

- **auth**: Cookie `session`（`可观测精确`）；服务端会话校验逻辑（`推断`）
- **request body**（`可观测精确` 字段名/类型，`推断` 必填性）:
  | 字段 | 类型 | 必填 | 观测/推断 |
  |------|------|:---:|:---------|
  | items | array<{sku:string, qty:number}> | 是 | `可观测精确` |
  | note | string | 否 | `可观测精确`（可选性为 `推断`） |
- **status codes seen**: `201`（`可观测精确`）
- **response `201`**（`可观测精确`）:
  ```json
  { "id": "uuid", "status": "pending", "items": [...], "created_at": "iso8601" }
  ```
- **推断错误态 / Inferred errors**（`推断`，未观测）:
  | 状态码 | 触发条件（推断） | 说明 |
  |:-----:|----------------|------|
  | 400 | 请求体校验失败 | 错误体形状为推断，按同域 `400` 封套 |
  | 401 | 会话失效 | 遍历中偶发观测到 → 若抓到则升 `可观测精确` |
  | 409 | 库存冲突 | 纯推断，服务端约束不可观测 |
~~~

---

## 数据模型反推 / Data-Model Inference

契约给出的是"线上传输的形状"；数据模型要回答"这些形状背后是哪些实体"。做法是**把响应载荷聚类成实体**，再补齐字段、类型、关系、主键。

Contracts give you wire shapes; the data model asks what entities sit behind them. Cluster response payloads into entities, then infer fields, types, relationships, and keys.

### 第一步：聚类成实体 / Cluster payloads into entities

- 把所有响应体（列表项、详情体、嵌套对象）按**字段指纹**聚类：字段名集合高度重合的对象归为同一实体。
- 一个 `GET /orders/:id` 详情体与 `GET /orders` 列表项若共享 `id/status/total/created_at`，属同一 `Order` 实体（列表是其投影/子集）。
- 嵌套对象独立成实体：`Order.customer = {id, name}` → 抽出 `Customer` 实体 + `Order → Customer` 关系。

### 第二步：推断字段与类型 / Infer fields & types

| 推断项 / Inferred | 依据 / Basis | 保真度 / Tier |
|:-----------------|:------------|:-------------|
| 字段是否存在 | 该字段在响应中出现过 | `可观测精确` |
| 字段类型 | JSON 值的运行时类型（string/number/bool/array/object/null） | `可观测精确`（表层）|
| 语义类型 | 从取值推断：`created_at` 值形如 ISO8601 → timestamp；`status ∈ {pending,paid}` → enum | `推断` |
| 可空性 | 观测到 `null` 或字段缺失 → nullable | `可观测精确`（若见过 null）/ `推断`（未见过）|
| 是否必填 | 服务端约束——**不可从响应观测**，只能推断 | `推断` |

### 第三步：推断关系与主键 / Infer relationships & keys

- **主键**：稳定、唯一、被 URL 用作 `:id` 的字段（通常 `id`）→ 推断为 PK。
- **外键 / 关系**：一实体内出现另一实体的 id（`order.customer_id` 或嵌套 `order.customer.id`）→ 推断 `Order N—1 Customer`。基数从观测推断：详情里 `items` 是数组 → `Order 1—N OrderItem`。
- **关系基数与级联规则是 `推断`**：你看到的是"这次返回了几条"，不是数据库约束。

### 第四步：对齐领域词汇表 / Align into a domain glossary

把聚类出的实体名对齐成一份领域词汇表，**复用 `engineer-architect/SKILL.md` 第四步的词汇表格式**（术语 / 英文 / 定义 / 边界），不要另造格式。目的：让线上 API 用的名字（可能是 `acct`、`usr`）收敛成克隆项目里一致的领域术语（`Account`、`User`），并主动质疑过载词。

Align entity names into a glossary reusing the **`engineer-architect/SKILL.md` glossary format** (term / English / definition / boundary). Normalize wire names (`acct`, `usr`) into consistent domain terms, and flag overloaded words.

```markdown
| 术语 | 英文 | 定义 | 边界/说明 | 保真度 |
|------|------|------|-----------|:------:|
| 订单 | Order | 一次下单产生的交易记录 | 不含退货流程（未观测） | `可观测精确`（字段）|
| 客户 | Customer | 下单主体 | 与登录 User 是否同一体：`推断` | `推断` |
```

> **服务端约束/校验是 `推断`。** 字段长度上限、唯一性约束、正则校验、跨字段规则、默认值、数据库触发器——这些**从响应体看不到**。除非你在 Phase 2 故意提交非法值、从 `400` 错误体里读到了具体规则文案，否则一律标 `推断`，构建时按推断实现并在 `CLONE-FIDELITY.md` 说明。**Server-side constraints and validation are `推断` unless a triggered `400` body exposed the exact rule.**

---

## 与 architect 的衔接 / Handoff to architect

Phase 3 的产物不是最终蓝图——它是喂给 Phase 5 三文档合成的**素材**，最终落到 `CONTEXT.md`。合成时**直接套用 `engineer-architect/SKILL.md` 的模板格式**（不复制其领域内容，只借结构）：

Phase 3 output is raw material for Phase 5 synthesis into `CONTEXT.md`, using the `engineer-architect/SKILL.md` templates verbatim in structure (not content):

| Phase 3 产物 | 落入 CONTEXT.md 的哪一节 | 复用哪个 architect 模板 |
|:------------|:----------------------|:----------------------|
| 聚类出的实体 + 字段/类型/关系/主键 | **核心数据字典 / Core Data Dictionary** | `engineer-architect/SKILL.md` 第六步「数据模型设计」的实体表 + 第十一步 CONTEXT.md 模板的「核心数据字典」节 |
| 重建的 API 契约（method/path/请求响应形状/状态码/鉴权) | **API 契约 / API Contracts** | `engineer-architect/SKILL.md` 第七步「API 契约设计」的路由表 + 详细契约格式 |
| 对齐后的实体命名 | **领域词汇表 / Domain Glossary** | `engineer-architect/SKILL.md` 第四步「领域词汇表设计」格式 |

衔接要点：

- **命名一致性**：数据字典的实体名、API 契约的资源名、词汇表的术语三者必须同源（architect 第六/七步的「衔接说明」原则）——线上叫 `usr` 也要在克隆里统一成 `User`。
- **`:id` 参数化路径**直接成为 CONTEXT.md「API 契约」节的路由；`推断` 的错误态在契约里保留标注，供 engineer-job 构建时知道"这是猜的"。
- **每个字段/契约项都带保真度标签**（下一节），标签随素材一起流入 CONTEXT.md 并汇总进 `CLONE-FIDELITY.md`——architect 模板本身没有这一列，这是 cloner 的增补。

> 与正向 architect 的唯一差别：architect 的数据模型/契约是**设计出来的**（面向未来），cloner 的是**观测反推的**（面向既有站点）。因此 cloner 多带一列保真度，且明确区分"抓到的"与"猜的"。格式相同，来源不同。

---

## 诚实标注 / Honesty Tagging

这是本阶段的**交付本质**：每一条重建的契约项、每一个推断的模型字段，都必须携带 `SKILL.md` 原则一定义的三层保真度标签之一，并流入 `CLONE-FIDELITY.md`。

This is the deliverable's essence: every reconstructed contract item and inferred model field carries one of the three tier tags from `SKILL.md` Principle 1, and flows into `CLONE-FIDELITY.md`.

| 层级 / Tier | 用于契约/模型的哪些项 |
|:-----------|:--------------------|
| `可观测精确` / Observable-exact | 真正抓到的：method、path、见过的状态码、成功响应体形状、请求体出现过的字段、鉴权机制类型 |
| `推断` / Inferred | 反推的：未触发的错误态、字段必填性、语义类型（enum/timestamp）、关系基数、服务端校验与约束、排序/分页算法 |
| `不可观测` / Unobservable | 客户端根本看不到的：服务端源码、数据库物理 schema、密钥、支付网关内部、第三方 SaaS 内部、纯服务端算法 |

### 标注纪律 / Tagging discipline

- **一项一签**：不给"整个端点"一个笼统标签。一个端点的成功形状可以是 `可观测精确`，同时它的错误态是 `推断`，它依赖的支付回调是 `不可观测`——三者分别标注。
- **默认下沉**：拿不准就标更低的保真度（`推断` 优先于 `可观测精确`；涉及服务端内部一律 `不可观测`）。诚实的下限比乐观的上限安全。
- **可升级**：`推断` 项若后续在 Phase 2 补触发并抓到真实响应，回填形状并升为 `可观测精确`，同步更新账本与 `CLONE-FIDELITY.md`。
- **账本回写**：标注结果写回对应节点，并把 `coverage_status` 从 `explored` 推进到 `extracted`（见 observation-playbook「状态转移」）——只在契约与数据模型**真正提取完成**后才置 `extracted`，不得谎报。

### 流入 CLONE-FIDELITY.md / Flowing into CLONE-FIDELITY.md

每条契约/模型项在保真度报告里留下逐资产记录，供 Phase 6 验收对照。模板与三层级标注规范见 `references/fidelity-report-template.md`。示意：

```markdown
| 资产 | 类型 | 保真度 | 说明 |
|------|------|:------:|------|
| POST /api/v1/orders 成功体 | API 契约 | `可观测精确` | HAR 抓到 201，形状 1:1 |
| POST /api/v1/orders 400 错误体 | API 契约 | `推断` | 未触发，按同域封套推断 |
| Order.status 枚举取值 | 数据模型 | `推断` | 仅观测到 pending/paid，全集未知 |
| 支付回调 /webhook/pay | API 契约 | `不可观测` | 第三方内部，mock 实现 |
```

> 一句话原则：**抓到的写"抓到"，猜到的写"猜到"，看不到的写"看不到"。** 任何把 `推断` 冒充 `可观测精确` 的标注，都会在 Phase 6 视觉/行为验收时暴露，并违背 `SKILL.md`「绝不声称复制了后端」的红线。
