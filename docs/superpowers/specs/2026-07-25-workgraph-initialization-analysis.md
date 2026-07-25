# 分析 Spec：Workgraph 是如何初始化的

> 日期: 2026-07-25
> 类型: 分析（描述现状 + 揭示 foot-gun），非改动 spec
> 范围: `src/workgraph.rs`、`src/background.rs`、`src/agent.rs`、`src/daemon/mod.rs`、`src/tool/dev.rs`、`src/tool/reason.rs`、`src/bg_ledger.rs`
> 目的: 为后续是否要改 workgraph 初始化（尤其"空图/自动播种"）提供事实基础与决策入口

---

## 一、一句话结论

**Workgraph 没有"初始化"仪式——它是懒生成的：磁盘上没有 `workgraph.json` 时 `WorkGraph::read` 返回一张空图，节点只在 LLM 显式调用 `milestone` 工具（或 rc 树 `to_milestone` 转换、或一次性 `todos.json` 迁移）时才被创建。没有任何代码从 task 字符串、`AGENTS.md` 或 LLM 计划自动播种里程碑。因此 headless `CODECODER_BG_WORKGRAPH=1` 模式假设"有人已经填好了 `workgraph.json`"；若没填，它立刻以退出码 0（"成功"）空跑退出。**

---

## 二、数据模型（`src/workgraph.rs`）

- **`WorkGraph`**（`:85-89`）= `{ schema_version: u32, nodes: Vec<Milestone> }`。常量 `WG_SCHEMA_VERSION = 1`（`:13`）。字段名是 `schema_version`（**不是** `version`——这点常被误写，cc-web/测试 fixture 曾因此踩坑）。
- **`Milestone`**（`:58-83`）:
  - `id: u64`（必填，无 serde default）、`title: String`（必填）、`status: NodeStatus`（必填）
  - `acceptance / deps / touched / fix_attempts` 均 `#[serde(default)]`
  - `verdict / last_failure / command` 均 `#[serde(default, skip_serializing_if = "Option::is_none")]`
  - `command` 有值 → bg_gate 走客观命令门；无值 → 回退 `extract_gate_command(acceptance)` 启发式，再退到独立评审门（ADR 0039）
- **`NodeStatus`**（`:18-30`，`#[serde(rename_all = "snake_case")]`）: `pending / in_progress / blocked / needs_fix / done / hypothesis / locked`。`hypothesis`、`locked` 属于 rc 诊断树，不是构建图状态。
- **关键不变量**（`:15-17`）: `Blocked` 是**派生**的（从未满足的依赖重算，不是权威意图记录），其余状态由动作显式设置。

## 三、创建与加载路径

磁盘后备文件: `<root>/workgraph.json`（`path()` `:92-94`）；锁文件 `workgraph.json.lock`（`:157`）；原子写临时文件 `workgraph.json.tmp`（`:142`）。

所有构造/加载入口:
- `Default`（`:96-100`）→ 空图 `{schema_version:1, nodes:[]}`。
- **`read(root) -> WorkGraph`（`:105-117`，永不报错/panic）**: ① 读 `workgraph.json` 经 `load()` 解析；② 否则迁移旧的扁平 `todos.json`（`migrate_todos` `:417-441`）；③ 否则 `Default`。
- `load(raw) -> Result`（`:121-134`）: 读 `schema_version`（缺失默认 `0`）；**拒绝比支持版本更新的文件**（`bail!` `:124-128`）；`while version < WG_SCHEMA_VERSION` 前向迁移（`:129-132`）；再 `from_value`。
- `save(&self, root)`（`:137-146`）: 建父目录、写 `.tmp`、`rename` 原子替换（对齐 session.rs / ADR 0004）。
- `with_lock(root, f)`（`:152-175`）: 见 §六。
- `add(title, acceptance, deps) -> Result<u64>`（`:189-217`）: 唯一铸造 id 的变更器（见 §五）。

**缺失/空/损坏时的行为**: 缺文件 → 空图；空文件 → 解析失败 → 空图；**损坏 → 静默空图**；**更新版本（如 v2 被 v1 二进制读）→ `load` 报错，但 `read` 用 `.ok().and_then(...)`（`:107`）吞掉错误 → 静默空图**（下一次 `save` 覆盖，数据丢失，见 foot-gun #3）。

**迁移**: `migrate(from, json)`（`:408-413`）仅注册 `0->1`（no-op passthrough），其它来源 `bail`。新增字段靠 `#[serde(default)]` 兜底（测试 `:523-535`、`:693-699`）。

## 四、里程碑从哪来（核心）

**生产代码里只有两处调用唯一的建节点 API `WorkGraph::add`:**

1. **`milestone` 内置工具**（`src/tool/dev.rs:139-278`）——`permission() == None`（`:166-168`，从不门控）。`action ∈ {list,add,start,done,needs_fix,next,remove}`。全程包在 `with_lock`（`:179`）里。`"add"` 分支（`:200-233`）读 `title/acceptance/command/deps` → `g.add(...)` → 设 `command`，并在无可运行命令但 acceptance 非空时提示"评审门较弱"。**这是 agent 驱动的来源**：LLM 决定建里程碑。
2. **rc 树 `to_milestone`**（`src/tool/reason.rs:150-182`）——把 `locked` 因果节点转成里程碑，`acceptance = "Resolve the causal finding: {question}. margin:… leverage:…"`，`wg.add(...)+wg.save()`。**注意此路用裸 `read+add+save`，未走 `with_lock`**（ADR 0035 一致性缺口，见 foot-gun #5）。

**消费型路径（只读取/推进，从不创建）:**
- **交互 `drive_workgraph`**（`src/agent.rs:1419-1486`，仅当 `persist && trust==Trusted`）: 消费 `next_ready`，跑至多 `MAX_AUTO=3` turn，靠解析 turn 的 `VERDICT:` 行写回状态。**从不建里程碑。**
- **daemon 空闲推进**（`src/daemon/mod.rs:101-124`）: 每 30s `try_lock` 到 turn_token 就 `advance_one_milestone`。**只消费。**
- **headless BG runner**（`src/background.rs`）: `resolve_bg_task`（`:64-83`）、`advance_one_milestone`（`:327-351`）都是 `read`+`next_ready`。**空图行为**: workgraph 分支（`:151-255`）`ready_id=None` → `retry_one_milestone` 返回 `Ok(None)` → `Ok(None)` 分支（`:180-195`）发现无 `NeedsFix` → 置 `MissionState::CompletedAllReady` → `break` → 经 `bg_ledger::mission_exit_code`（`src/bg_ledger.rs:43`）**退出码 0**。

**从 AGENTS.md / task / LLM 计划自动播种——确定不存在。** `AGENTS.md` 只被读进系统提示身份（`src/agent.rs:1506-1510`），从不解析成里程碑；`plan` 工具在 headless 下被自动拒绝（ADR 0026）。图的填充只有三条来源：LLM 调 `milestone` 工具、rc `to_milestone`、一次性 `todos.json` 迁移。

## 五、派生状态与不变量

- `recompute_blocked`（`:275-284`）: `Pending&&blocked→Blocked`；`Blocked&&!blocked→Pending`；**不碰** `InProgress/NeedsFix/Done/…`（意图保真，测试 `:475-482`）。每次 `add/set_status/remove` 后调用。
- `deps_done`（`:267-269`）: 所有 dep 解析到 `Done` 节点；悬空 dep id → `false`。
- `next_ready`（`:246-251`）: 最小 id 且 `Pending` 且 `deps_done` → **id 顺序 = 执行顺序**。`next_retryable`（`:256-265`）是 `NeedsFix` 且 `fix_attempts<max` 的对应选择器。
- `next_id`（`:177-179`）= `max(id)+1`（空则 1）——**从最大值单调，不是持久计数器**（id 复用隐患，foot-gun #4）。
- `add` 校验（`:189-217`）: 拒空 title、拒未知 dep，`validate()` 失败则弹出该节点回滚（原子）。`validate`（`:287-321`）做引用完整性 + DFS 三色环检测。

## 六、并发与持久化（ADR 0035）

- `with_lock`（`:152-175`）: 对**独立**的 `workgraph.json.lock` 用 `fs2::lock_exclusive`（独立是因 `save` 的原子 rename 换 inode 会破坏对数据文件持有的锁）。闭包内 `read→mutate→save` 全程持锁，毫秒级，**从不覆盖 LLM turn**。
- 动机（ADR 0035 背景）: P9 压测 4 并发 `milestone add` → **0 存活**（静默丢更新）。修复把三处写入统一到 `with_lock`: `advance/retry_one_milestone`、`drive_workgraph`、`milestone` 工具。**例外**: `reason.rs:172-175` 仍裸写（缺口）。
- 只读点（`render_for_prompt` `:1517`、`next_ready` 探测）故意不加锁（容忍轻微陈旧；竞争只在写-写）。
- 写盘时机: 每次变更 `with_lock` 结尾都 `save`（原子 temp+rename）。daemon 用 `turn_token` 串行化 turn 与 tick。

## 七、入口与环境变量

- `bg_mode_from_env`（`src/lib.rs:62-75`）: 优先级 `CODECODER_BG_TASK`（非空→Explicit）> `CODECODER_BG_WORKGRAPH=="1"`→Workgraph > None（daemon）。`src/main.rs:8-12` 分派。`CODECODER_ROOT` 定 root。上限从 Config 注入: `bg_max_auto / bg_circuit_k / bg_milestone_tool_cap / bg_max_fix_attempts`（`src/background.rs:97-102`）。
- 首次触图: headless → `run_background_cfg` 的 `WorkGraph::read`（`:163` 或 `resolve_bg_task :69`）；daemon → 空闲线程 `advance_one_milestone` 的 `read`（`:336`）+ 每次建系统提示读（`agent.rs:1517`）；交互 → `drive_workgraph` 的 `read`（`agent.rs:1424`）。

## 八、相关 ADR / 设计文档

- `docs/design/2026-07-19-plan-work-graph.md` — 奠基 spec（"事前构造之图"）：`WorkGraph/Milestone`、`next_ready`、环检测、级联阻塞、版本化/原子/前向迁移。
- `docs/adr/0035-workgraph-concurrency-write-protection.md` — `with_lock`、"4 并发 add→0 存活"、独立 `.lock`、锁不覆盖 turn。
- `docs/adr/0026-background-agent-headless-runner.md` — headless runner、无人在场权限门、iter-1 自恢复。
- `docs/adr/0030-bg-objective-acceptance-gate.md` — 客观验收门覆盖自报 VERDICT、`next_action` 续/停策略。
- `docs/adr/0033-bg-ledger-and-exit-codes.md` — `mission_state→exit_code`（`CompletedAllReady/Running→0`、`BlockedAt/StuckNeedsFix→2`、`CircuitBreaker→3`、`Error→4`）。
- `docs/adr/0039-bg-review-gate-and-observability.md` — 独立评审门 + headless 可观测性（本轮新增）。
- `docs/adr/0004-session-persistence-and-migration.md` — workgraph 持久化/迁移所镜像的范式。

## 九、发现的 foot-gun（供后续决策）

> 以下是**现状问题**，不是本 spec 要修的内容——列出以便你决定是否单开改动 spec。

1. **空图 → 退出码 0"成功"（headless）。** 空/缺 `workgraph.json` → `CompletedAllReady` → exit 0，什么都没做。`StuckNeedsFix` 修复只覆盖了"只剩 needs_fix"的假成功，**未覆盖"真空图"**。调度器看到"成功"。（这正是 experiment-report #3 的真实内核——当时刻意保留未改。）
2. **完全无自动播种。** 没有任何东西从 task/AGENTS.md/LLM 计划派生里程碑。headless `CODECODER_BG_WORKGRAPH=1` 假设有人已填 `workgraph.json`；否则见 #1。
3. **`read()` 静默吞损坏与更新版本文件**（`:107`）。损坏或 v2 文件与"空"不可区分，下一次 `save` 覆盖 → 数据丢失陷阱。`load()` 正确报错，但 `read()` 丢弃了该错误。
4. **id 复用隐患。** `next_id=max+1` 非持久计数器；删掉最大 id 节点后，下一次 `add` 复用其 id → `bg_ledger.jsonl` / 因果节点 / 按 id 记的 `verdict`/`last_failure` 历史可能指向另一个里程碑。
5. **`reason.rs:172-175` 绕过 `with_lock`**（裸 read/add/save），违反 ADR 0035"三处写入统一"，与 daemon tick 或 milestone 工具并发时可丢更新。
6. **`drive_workgraph` 写回依赖 LLM 输出可解析的 `VERDICT:` 行**（`agent.rs:1451-1477`）；缺失则里程碑停在 `in_progress` 仅发 notice → 交互自动推进可能静默卡住（headless 有客观门覆盖，不受此影响）。
7. **同一张图两套写回语义**: 交互 `drive_workgraph` 信任 agent 自报 VERDICT（`agent.rs:1452`），headless `run_milestone_and_gate` 用客观/评审门覆盖（`background.rs`）。同一里程碑可能交互标 `Done`、headless 标 `NeedsFix`。

## 十、可能的后续方向（未决，供你选）

- **A. 空图不再假成功**: headless 空图 → 新 `MissionState`（如 `EmptyGraph`）+ 非 0 退出码（或明确 log 提示需先 seed），修 foot-gun #1。
- **B. 可选自动播种**: 在 `CODECODER_BG_WORKGRAPH` 下，空图时从某来源（task 字符串 / 一个显式的 `milestones.md` / 一次 LLM 规划 turn）生成初始里程碑——需权衡自主性 vs 可控性（ADR 0033 当初刻意不做）。
- **C. `read()` 区分"空"与"坏"**: 损坏/更新版本时不静默降级为空图，改为报错或备份原文件，修 foot-gun #3、#4 的数据丢失面。
- **D. 统一 `reason.rs` 写入走 `with_lock`**（修 #5）；统一交互与 headless 的写回语义（#7）。

这些都各自值得单开改动 spec；本文只做分析，不含实现。
