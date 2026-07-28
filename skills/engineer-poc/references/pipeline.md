# engineer-poc — 六阶段详细流水线 / Detailed Pipeline

本文件是 `SKILL.md` 流水线表的展开，供执行阶段逐步照做。

## 阶段间数据流 / Data Flow

```
Phase 0  scope        REQUIREMENTS.md + FRONTEND-DESIGN.md(+CONTEXT.md) → POC 范围 + 行业匹配 + do-not-fake
   │
   ▼
Phase 1  scaffold     演进式前端工程（设计 Token + 路由 + mock adapter 接缝）
   │
   ▼
Phase 2  ledger       .agents/poc.ledger.json（每个 page/component/state/flow → coverage_status=planned）
   │
   ▼
Phase 3  implement    逐页高保真实现（全 UI 状态 + mock 数据 + 交互）— loop-until-dry
   │
   ▼
Phase 4  wire         跨页流程 + mock 登录/角色 + 内存持久化 + seed 数据 + run gate
   │
   ▼
Phase 5  synth        POC-MANIFEST.md + POC-FIDELITY.md + 报告 → stop 或交棒 engineer-job
```

## 阶段 0：输入校验与行业识别

1. 读 `REQUIREMENTS.md`、`FRONTEND-DESIGN.md`；有则读 `CONTEXT.md`（取数据模型/契约）。
2. 三文档全缺 → 降级：从用户 prompt 生成最小需求 + 页面清单，并在 `POC-FIDELITY.md` 标注"输入降级"。
3. 从需求识别行业/形态，匹配 `industry-patterns.md` 的一个或多个原型。
4. 划定 do-not-fake 边界（支付网关、第三方 SaaS、纯服务端算法），写入 ledger `meta.do_not_fake`。

## 阶段 1：POC 脚手架

1. 技术栈：优先取 `FRONTEND-DESIGN.md`；缺失则默认 Vite + React（或 Vue，按需求）。
2. 接入设计 Token（来自 `FRONTEND-DESIGN.md` 或 `frontend-spec.json`）为 CSS 变量/主题。
3. 建立路由骨架（按页面树）。
4. 建立 mock adapter 接缝（`src/mocks/`），规范见 `mock-layer-guide.md`。

## 阶段 2：覆盖账本构建

1. 从 `FRONTEND-DESIGN.md` 页面树/组件树/UI 状态机枚举节点，写入 `.agents/poc.ledger.json`（schema 见 `poc-ledger.schema.json`），coverage_status=planned。
2. 每个 page 节点记录应覆盖的 `ui_states`（loading/empty/error/normal/edge）。

## 阶段 3：逐页高保真实现（loop-until-dry 核心）

```
frontier ← 账本中 coverage_status == "planned" 的 page/component 节点
repeat:
    new_nodes ← 0
    for each node in frontier:
        实现该节点全部 ui_states + mock 数据 + 交互（套用行业范式）
        发现的子视图 / 弹窗 / 状态分支 / 流程 若未在账本:
            加入账本(coverage_status="planned"); new_nodes++
        node.coverage_status ← "implemented"; 标 fidelity 层级
    # coverage critic 复查
    critic 追问：还有哪些页面 / 状态 / 流程 / 角色视图没覆盖？
    critic 发现的遗漏写入账本(planned); new_nodes += critic 新增
    frontier ← 账本中仍为 "planned" 的节点
until new_nodes == 0 且 frontier 为空
assert 每个 node 都 implemented/placeholder/skipped 且带 fidelity
```

单页实现失败 → 降级为占位页，coverage_status="placeholder"，fidelity="占位未实现"。

## 阶段 4：全生命周期贯通

1. 打通跨页流程（列表→详情→编辑→保存→回列表）。
2. mock 登录/角色切换（内存 session；不同角色看到不同视图/权限）。
3. 内存持久化（增删改在会话内可见）+ 真实感 seed 数据。
4. run gate：构建 + 启动开发服务器必须通过；失败进入修复循环，修不动标 DOES_NOT_RUN。

## 阶段 5：产物合成与交棒

1. 写 `POC-MANIFEST.md`（页面/组件/路由/mock 端点 + mock→真实演进映射）。
2. 写 `POC-FIDELITY.md`（逐资产 `真实交互 / mock 数据 / 占位未实现`）。
3. `stop_at_poc` → 输出交棒摘要并停止；否则交棒 engineer-job Phase 4 演进。

## 恢复 / Recovery

会话中断后从 `.agents/poc.ledger.json` 恢复：
- 跳过 coverage_status == "implemented" / "placeholder" 的节点。
- 从 "planned" 节点继续 loop-until-dry。
- ledger 不存在 → 从阶段 0 重新开始。
