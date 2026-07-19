# Spec: Work Graph 工作图(一等公民 #2 —— 事前构造之图)

> 来源:`docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` 候选 **#2**(头号新增)。
> 把「在途工作」这一层文件化:一张**持久、依赖有序、验收挂载**的里程碑图,与 Session
> (事后记录树)构成「打算做什么 / 发生了什么」的两半。遵循「机制在内核、方法在磁盘」——
> 内核只放**图数据结构 + 调度/校验原语**,编排方法论(engineer 技能梯)留 `skills/`。
> 复用 [[0004-session-persistence-and-migration]] 的落盘/迁移范式;验收结果来自
> `docs/design/2026-07-19-review-verdict-rubric.md`(#4,已落地)。

## 目标

- 新增**一等公民 Work Graph(工作图)**:`WorkGraph { nodes: Vec<Milestone> }`,持久化到
  `workgraph.json`(版本化 + 原子写 + 前向迁移,镜像 `session.rs`)。
- **Milestone(里程碑)节点**:`id · title · acceptance · deps · status · verdict? · touched`。
- **调度原语**:`next_ready()`(所有 deps 已 done 的 pending 节点)、环检测、级联阻塞。
- 图对 agent **可见可写**(工具),对 Background Agent(0026)**可消费**(取下一个就绪节点当 task)。
- #4 的 `Review Verdict` 可**挂到节点**上作为验收结果(接缝已在 #4 备好)。

## 现状(必须兼容 / 决定去留)

- `todo` 工具(`src/tool/dev.rs:167`)= 扁平 `todos.json`:无依赖、无验收、不驱动调度。
  **本 spec 用 Work Graph 取代它**(扁平表是「无边的退化图」,迁移平凡)。
- `plan` 工具(`dev.rs:109`)= 一次性审批手势 → `PLAN.md`。**不动**:它是「审批」,不是「工作图」。
- `Session`(`session.rs`):`schema_version` + 原子 `save`(temp+rename)+ `while version < SCHEMA_VERSION` 迁移链。**Work Graph 照抄此范式,但用自己独立的 `WG_SCHEMA_VERSION`**。
- Background runner(`background.rs:30`):task 是裸 `String`。本 spec 让它**可选**改为「无显式 task 时取图中下一个就绪节点」。

## 命名(本 spec 的决策 —— 审计留给 spec 的问题)

- 概念:**Work Graph / 工作图**;节点:**Milestone / 里程碑**;文件:`workgraph.json`。
- 工具:**新增 `milestone` 工具,移除 `todo`**(内置工具总数仍 25)。一次性把旧 `todos.json`
  的每条 todo 迁成一个无依赖 Milestone(`status` 由 `done` 映射)。
- 术语表:`plan`(审批)/ `milestone`(工作图节点)/ Session(记录树)三者不混淆。
  **【开放决策——评审时可改】** 若你更想:(a) 保留 `todo` 作「turn 内轻量便签」与工作图并存,
  或 (b) 不叫 `milestone` 而复用 `todo` 之名升级其 schema,告诉我,我改。默认取「取代 + 新名」
  因为它最干净、且让「工作图」成为无歧义的一等公民。

## 设计

### 1. 数据模型(`src/workgraph.rs`)

```rust
pub const WG_SCHEMA_VERSION: u32 = 1;

pub enum NodeStatus { Pending, InProgress, Blocked, NeedsFix, Done }

pub struct Milestone {
    pub id: u64,
    pub title: String,
    pub acceptance: String,        // 编码前先写的验收契约;可空
    pub deps: Vec<u64>,            // blockedBy:必须先 done 的节点
    pub status: NodeStatus,
    pub verdict: Option<String>,   // 来自 #4 Review Verdict 的 as_str();挂载点
    pub touched: Vec<String>,      // 相关文件/commit,人读线索
}

pub struct WorkGraph {
    pub schema_version: u32,
    pub nodes: Vec<Milestone>,
}
```

- **持久化**:`save(path)` 原子写(temp+rename)、`load(raw)` 版本校验 + 迁移链——**与
  `session.rs` 逐字同构**(含「未来版本拒读」)。文件 `root/workgraph.json`。
- **`next_id()`**:`max(id)+1`(镜像 `Session::next_message_id`)。

### 2. 调度原语(纯函数,好单测)

- `fn next_ready(&self) -> Option<&Milestone>`:`status==Pending` 且**所有 `deps` 均 `Done`**
  的**最小 id** 节点(确定性)。
- `fn add(&mut self, title, acceptance, deps) -> Result<u64>`:**加边即环检测**(DFS),
  成环则 `Err`,不写入。
- `fn set_status(&mut self, id, status)`:置状态;若置 `Done`,触发 `recompute_blocked()`。
- `fn recompute_blocked(&mut self)`:任一节点若有**未 done 的硬依赖**→ 派生 `Blocked`
  展示(级联);依赖解除后回 `Pending`。**派生态不落盘覆盖用户显式态**:`Blocked` 仅由
  依赖推导,`NeedsFix`/`InProgress`/`Done` 由动作显式设置。

### 3. `milestone` 工具(替换 `todo`)

`Permission::None`(规划 scratch,和旧 `todo` 同级——写的是本地计划文件,无危险副作用)。

| action | 参数 | 效果 |
|---|---|---|
| `list` | — | 渲染工作图(拓扑序 + 状态 + 就绪标记 `▶`) |
| `add` | `title`, `acceptance?`, `deps?: [id]` | 加节点(环检测);返回新 id |
| `start` | `id` | → `InProgress` |
| `done` | `id`, `verdict?` | → `Done`,可挂 verdict;级联重算 |
| `needs_fix` | `id` | → `NeedsFix` |
| `next` | — | 返回 `next_ready()`(下一个该做的) |
| `remove` | `id` | 删节点(若被依赖则拒绝并提示) |

- 渲染:每行 `[status] #id title  (deps: …)  ▶就绪 / ✓verdict`;空图给引导语。
- **子 agent 不给此工具**:它是有副作用的规划写操作,`Toolbox::read_only_child()` 不含
  (与 `todo`/`plan`/`memory` 一致——只读子 agent 无本地 scratch 写,见 CONTEXT.md Sub-agent 条)。

### 4. 与 #4 验收的咬合(接缝,轻)

- `milestone done <id> verdict=<pass|needs_fix|rebuild>` 把 #4 产出的 `Review Verdict` 写进
  `Milestone.verdict`。`rebuild`/`needs_fix` 时**建议**不置 `Done` 而置对应态(工具校验:
  `done` 若带非 `pass` verdict → 落 `NeedsFix` 并提示)。**内核不强制**跑 review;由方法论
  (skill)驱动「done 前先 review」。

### 5. 与 Background Agent 接入(可选,最小)

- `background.rs`:若 `CODECODER_BG_TASK` 未设**且** `workgraph.json` 存在就绪节点 →
  task = 该节点 `title` + `acceptance`(拼成指令),跑完按结果置态。**外置调度器不再必需**,
  图自排序。**范围内仅「取一个就绪节点跑一轮」**;循环推进整图、每节点派生清洁子代理列为**延后**。

## 与在途/既有工作的关系

- **tree-sessions #1 统一节点模型**:`Milestone` 与会话/因果节点共享「带状态节点」形状。**本 spec
  先用 Work Graph 自己的最小 `NodeStatus`**;待 tree-sessions Phase A/E 落地、#1 统一节点模型
  成型后,可收敛到共享节点类型(**本 spec 不预先耦合**,避免依赖未落地的地基)。
- **Session**:工作图与会话是两半;v1 **不建**「节点 ↔ 会话分支绑定」(等 tree-sessions)。
- **compaction(0023)**:`workgraph.json` 是文件,天然跨压缩/重置存活;把「当前工作图摘要」
  注入 system prompt 作锚 = **延后的增值**,非 v1。

## 内核改动点

- **新增 `src/workgraph.rs`**:上述类型 + 持久化 + 迁移 + 调度原语 + 单测。
- **`src/lib.rs`**:`pub mod workgraph;`。
- **`src/tool/dev.rs`**:移除 `Todo`,新增 `Milestone` 工具(读写 `workgraph.json`)。
- **`src/tool/mod.rs`(Toolbox)**:注册表把 `todo` 换成 `milestone`;`read_only_child()` 不含它。
- **`background.rs`**:可选的「无 task 时取就绪节点」分支。
- **一次性迁移**:首次加载若只存在旧 `todos.json`、无 `workgraph.json` → 转换后写出。

## 测试(TDD)

- **`src/workgraph.rs` 单测**:
  - save/load round-trip;`schema_version` 未来版本拒读;`next_id`。
  - `next_ready`:无依赖取最小 id;有未 done 依赖的节点被跳过;全 done 后解锁。
  - `add` 环检测:自环、间接环 → `Err` 且不写入。
  - `set_status(Done)` → 级联 `recompute_blocked` 解锁下游;依赖未满足者显示 `Blocked`。
  - 旧 `todos.json` → Work Graph 迁移(done 位保持)。
- **行为验证(`tests/`,ScriptedProvider)**:
  - agent 连续调用 `milestone add`(带 deps)→ `next` 返回就绪节点 → `done` → `next` 解锁下游;
    断言最终 `workgraph.json` 的节点状态(文件系统面断言,见 `docs/testing/behavioral-validation.md`)。
  - `milestone` 不在子 agent 工具集(镜像 `l1_subagent` 的 toolset 断言)。

## 风险

- **命名/取代 `todo`**:见「命名」开放决策;取代会改工具契约,但 `todo` 使用面小、迁移平凡。
- **派生 `Blocked` vs 显式态**:严格区分「依赖推导态」与「动作显式态」,避免覆盖用户意图
  (设计已隔离:`Blocked` 只读推导)。
- **环检测成本**:节点规模小(人写的里程碑),DFS 足够;不引入图库依赖(契合 codecoder 无谓依赖原则)。
- **调度不硬驱动**:v1 图是「可见可查、可喂 background」,不强制 agent loop 按图跑——避免过度
  设计;硬驱动(每节点清洁子代理)留待确有需求另开。

## 刻意不做(v1)

- 不硬驱动 agent loop 逐节点派生清洁子代理(延后)。
- 不建「节点 ↔ 会话分支」绑定、不共享 tree-sessions 节点类型(等 #1 落地)。
- 不内核化 engineer 编排方法论(job/orchestrator/workflow)——留 `skills/`。
- 不注入工作图摘要进 system prompt(延后增值)。

## 文档同步(落地后)

- `CONTEXT.md`:新增术语 **Work Graph**(`_Avoid_`: todo list/backlog/plan)、**Milestone**
  (`_Avoid_`: task/step/ticket);更新/移除 **Todo** 条目。
- `ARCHITECTURE.md` / `README.md` / `CLAUDE.md`:模块 +1(`workgraph.rs`)、工具表 `todo`→`milestone`、
  测试计数;工具总数仍 25。
