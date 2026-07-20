# Spec: 推理/根因树 — 一等公民 #3

> 对应 `docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` 候选 **#3**（后向诊断轴）。
> 支撑文档：`docs/design/2026-07-19-tree-sessions.md` Phase E。
> 依赖：会话树已落地（Phase A，`session.rs` 含 `SessionEntry.meta` 旁路字段）、
> 统一节点模型（`NodeStatus` 含 `Hypothesis`/`Locked`，`workgraph.rs`）。
> 方法论：`archived/skills/rc-causal-chain`（因果链 + 观察收敛 7 公理）。

## 目标

把 codecoder 的会话树从"对话分支"升级为**一等公民的推理/根因树**，让 agent 在做系统性调试和根因分析时，能把推理过程持久化、可回溯、可收敛到高杠杆行动。

**一句话：** 给 agent 一个"画因果树"的工具，树的每个节点是一个候选原因，带上验证状态和余量信息，整棵树持久化到 session 中，离开分支时自动摘要"为何排除"。

## 现状

以下基础设施已就绪：

| 需要什么 | 现状 |
|----------|------|
| 树状会话 | ✅ `SessionEntry.parent` + `active_thread()` + `navigate_to` + `abandoned_branch` |
| 节点元数据旁路 | ✅ `SessionEntry.meta: Option<serde_json::Value>` |
| 节点状态枚举 | ✅ `NodeStatus::Hypothesis` 和 `NodeStatus::Locked`（`workgraph.rs`），但 session 树尚未使用 |
| 离开分支摘要 | ✅ `agent.rs` `Navigate` 命令已调用 `summarize_span` |
| rc 方法论文档 | ✅ `archived/skills/rc-causal-chain/SKILL.md` |

**关键缺口：** 没有专门的工具来创建/管理推理节点；agent 只能用 `navigate_to` 手动跳转，没有结构化的"加一个候选原因"操作。

## 设计

### 原则

- **机制在内核，方法在磁盘。** 内核提供最小机制：一个工具创建推理节点、节点附带验证状态和元数据字段。**观察收敛的纪律**（逐节点、先验证后锁定、余量×杠杆三选）写成 skill，用 `use_skill` 注入。
- **复用已有基础设施。** 推理树用会话树当基底——不新建文件格式、不新建持久化通道。`SessionEntry.meta` 承载推理元数据。
- **与 Plan 形成闭环。** 推理树的产出（关键问题 / 高杠杆行动）应当能喂给 Plan 变成里程碑。

### 1. 新增工具：`reason`（推理）

新增一个 `Permission::None` 工具，管理推理树上的节点。操作与 `milestone` 类似，但作用于 session 树，不作用于 workgraph。

```rust
// 工具名: reason
// 描述: "Manage inference-tree nodes for root-cause analysis.
//        action = add | status | margin | list | trace.
//        `add` creates a new node on the current session branch.
//        `status` sets hypothesis|locked on the current node.
//        `margin` sets margin/leverage metadata.
//        `list` queries the causal tree.
//        `trace` walks from a node up to root."
```

**数据模型：** 每个推理节点就是一条 `SessionEntry`。`meta` 字段存储推理元数据：

```json
{
  "kind": "causal",
  "status": "hypothesis",
  "margin": null,
  "leverage": null,
  "terminal": null,
  "parent_cause": null
}
```

**操作：**

| action | 参数 | 效果 |
|--------|------|------|
| `add` | `question: str` | 在当前 session leaf 追加一条标记为 `causal` 的条目，问题文本作为用户消息 |
| `status` | `id: u64, status: "hypothesis" \| "locked"` | 更新 `SessionEntry.meta.status` |
| `margin` | `id: u64, margin?: str, leverage?: str, terminal?: str` | 设置节点的余量/杠杆/末端信息 |
| `list` | — | 从当前 leaf 回溯到根，渲染因果树 |
| `trace` | `id: u64` | 从指定节点回溯到根，渲染完整因果链 |

### 2. 渲染格式

`reason list` 的渲染输出：

```
Causal tree:
  ? #3 为什么用户流失率上升?  [hypothesis]
    ? #4 新用户引导流程太长    [hypothesis]  margin:低, leverage:中
      × #5 A/B测试显示引导缩短不影响留存  [locked]  terminal:已排除
    ✓ #6 核心功能首次使用失败率30%  [locked]  margin:高, leverage:高
```

符号：
- `?` = hypothesis（未验证）
- `✓` = locked（已验证）
- `×` = excluded（已排除，通过离开分支摘要）

### 3. 与 Plan 的闭环

`reason list` 的输出中，`leverage: 高` 且 `margin: 高` 的节点是"关键节点"——agent 应当把它们转为 workgraph 里程碑。

由 skill 驱动，不在内核强制：`skills/debug-causal.md` 的纪律包含"收敛后调用 `milestone add` 把高杠杆节点转为里程碑"。

### 4. 落地形态

三步走（不依赖 Phase D 带类型条目）：

1. **纯 `meta` 旁路**：`SessionEntry.meta` 已存在，直接往里写 `{"kind":"causal","status":"hypothesis"}`。`reason` 工具读写 `meta`。
2. **`reason` 工具**：新增 `tool/builtin.rs` 中的 `Reason` 结构体，注册到 `Toolbox`。`Permission::None`。
3. **skill 方法论**：`skills/debug-causal.md`，用 `use_skill` 激活。

## 内核改动点

### 新增 `src/tool/reason.rs`

```rust
pub struct Reason;

impl Tool for Reason {
    fn name(&self) -> &str { "reason" }
    fn description(&self) -> &str {
        "Manage inference-tree nodes for root-cause analysis. ..."
    }
    fn schema(&self) -> Value { ... }
    fn permission(&self, _args, _root) -> Permission { Permission::None }
    fn run(&self, args, ctx) -> Result<ToolOutput> {
        // 读 session 的 meta 字段
        // 操作：add / status / margin / list / trace
    }
}
```

### 修改 `src/tool/mod.rs`

`Toolbox::builtin()` 注册 `Box::new(reason::Reason)`。

### 创建 `skills/debug-causal.md`

基于 `archived/skills/rc-causal-chain/SKILL.md` 精简，适配 codecoder 的会话树和 `reason` 工具。

### 修改 `src/lib.rs`

`pub mod reason`（或者放到 `tool/reason.rs` 子模块）。

## 测试

- `reason add` 在当前 session 追加条目，`meta.kind == "causal"`
- `reason status` 更新 `meta.status`
- `reason list` 从当前 leaf 回溯渲染
- `reason` 不在子 agent 工具集（同 `milestone`/`plan`/`memory`）
- 子 agent 发起 `reason` 调用 → 错误

## 刻意不做(v1)

- **不创建新文件格式**——推理树共享 session 的持久化通道
- **不创建新 AgentEvent 变体**——`reason list` 输出是 `ToolResult` 文本
- **不内核化 rc 纪律**——留 `skills/debug-causal.md`
- **不硬驱动 Plan 闭环**——由 skill 注入纪律
- **不依赖 Phase D 带类型条目**——纯 `meta` 旁路

## 风险

- `meta` 字段是自由 JSON——agent 可能写入不一致的数据。风险低，因为 `reason` 工具是唯一写入者。
- 树状会话已有 `navigate_to` 就地分支——推理树的分支切换和对话分支切换共用同一机制，不会冲突。

## 依赖

- 会话树 Phase A（已落地）
- `NodeStatus::Hypothesis` / `Locked`（已落地，但推理树用的是 `SessionEntry.meta` 中的 `status`，不是 `workgraph.rs` 的 `NodeStatus`——两者是平行概念，不共享）
- 不依赖 Phase B（`/tree` 导航）/ Phase C（分支摘要）/ Phase D（带类型条目）——但若这些已落地，体验更好

## 文档同步

- `CONTEXT.md`：新增术语 **Inference Tree** / **Causal Tree**（`_Avoid_`: debug tree, trace tree, root-cause tree）
- `ARCHITECTURE.md`：工具列表 +1、模块 +1