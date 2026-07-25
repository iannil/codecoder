# 设计：Workgraph 数据完整性/诚实性修复

> 日期: 2026-07-25
> 来源: `docs/superpowers/specs/2026-07-25-workgraph-initialization-analysis.md` 的 foot-gun #1/#3/#5
> 状态: 已批准，待写实现计划

## 一、范围

**做**（三项数据完整性/诚实性 bug）:
- **#1** headless BG 空图不再假成功（新 `MissionState::EmptyGraph` + 专用退出码 5）
- **#3** 存在但损坏/版本更新的 `workgraph.json` 不再被静默当空图覆盖（`read_checked` + 备份 + 中止）
- **#5** `reason.rs::to_milestone` 写入走 `with_lock`（修 ADR 0035 缺口）

**明确不做**（各自单独决策，非本 spec）:
- **B** 空图自动播种 — 产品方向，ADR 0033 刻意不做，需单独 brainstorm
- **#4** id 复用（`next_id=max+1` 非持久计数器）
- **#6/#7** 交互 vs headless 的 VERDICT 写回语义差异

## 二、#1 — 空图诚实退出

**问题**: `run_background_cfg` 的 workgraph 分支在空图时落到 `CompletedAllReady → exit 0`，调度器看到"成功"却什么都没做（分析 spec foot-gun #1）。需与"曾有里程碑、全部完成"的真 `CompletedAllReady` 区分。

**改动**:
1. `src/bg_gate.rs`：`MissionState` 新增变体 `EmptyGraph`（放在 `CompletedAllReady` 之后；带文档注释说明"图中无任何里程碑，headless 无事可做，需先 seed"）。
2. `src/bg_ledger.rs::mission_exit_code`：新增臂 `MissionState::EmptyGraph => 5`。该 match 是穷尽的，编译器会强制补臂（防漏）。5 当前空闲（现用 0/2/3/4）。
3. `src/background.rs::run_background_cfg` workgraph 分支**入口**（`let mut advanced = 0usize;` 之后、`loop {` 之前）插入判空：读图（用 #3 的 `read_checked`，见下），若 `graph.nodes.is_empty()` → observer 发 `empty workgraph — nothing to advance; seed workgraph.json first`，置 `out.mission_state = MissionState::EmptyGraph`，直接返回 `Ok(out)`（不进循环）。
   - 判定基准："入口即无节点" = EmptyGraph；"入口有节点、循环推进后无就绪且无 needs_fix" 仍是 `CompletedAllReady`（现有逻辑不变）。

## 三、#3 — 损坏/版本更新文件的数据丢失防护

**问题**: `WorkGraph::read`（`src/workgraph.rs:105-117`）用 `.ok().and_then(|raw| Self::load(&raw).ok())`——**存在但无法解析或版本更新的文件被静默当空图**，随后 `save` 原子覆盖 → 数据丢失（分析 spec foot-gun #3）。`load()` 本会对新版本 `bail!`，但 `read()` 丢弃了该 Err。

**区分**: **缺文件 / 空文件 / todos 迁移 → 合法空图**（走 #1 EmptyGraph 或正常迁移）；**存在但 `load()` 失败（损坏 JSON 或 `schema_version` > 支持）→ 错误**，不可覆盖。

**改动**:
1. `src/workgraph.rs`：新增 `pub fn read_checked(root: &Path) -> anyhow::Result<WorkGraph>`：
   - `workgraph.json` 存在 → 读取并 `load()`；`load` 成功返回 `Ok`，`load` 失败（损坏/新版本）返回 `Err`（带 path 上下文）。
   - `workgraph.json` 不存在 → 尝试 `todos.json` 迁移（同 `read`）；都无 → `Ok(WorkGraph::default())`（合法空图）。
   - 语义: 唯一返回 `Err` 的情形 = "`workgraph.json` 物理存在但内容不可用"。
   - `read()`（infallible）保留不变，供展示/探测路径（`render_for_prompt`、`next_ready` 探测）继续用。
2. 写入型入口改用 `read_checked`：
   - `src/background.rs`：`advance_one_milestone`、`retry_one_milestone`（均已返回 `anyhow::Result`）内部的首个 `WorkGraph::read` → `read_checked?` 传播（覆盖 headless 循环 **与** daemon 空闲推进，因 daemon 直接调 `advance_one_milestone`）。
   - `run_background_cfg` workgraph 分支入口：显式 `read_checked` 以做 #1 判空 + #3 中止（见下）。
3. 中止 + 备份逻辑（入口）:
   ```
   let graph = match crate::workgraph::WorkGraph::read_checked(&root) {
       Ok(g) => g,
       Err(e) => {
           let bad = crate::workgraph::path(&root); // workgraph.json
           let backup = root.join(format!("workgraph.json.corrupt.{}", std::process::id()));
           let _ = std::fs::rename(&bad, &backup); // 备份，绝不 save 覆盖
           let msg = format!("workgraph.json unreadable ({e}); backed up to {}", backup.display());
           obs.emit("error", &msg);
           out.mission_state = crate::bg_gate::MissionState::Error(msg);
           return Ok(out);
       }
   };
   if graph.nodes.is_empty() { /* #1 EmptyGraph 分支 */ }
   ```
   - 备份名用 `std::process::id()`（`Date::now`/时间戳在本环境受限；pid 足够唯一且可复现）。
   - `crate::workgraph::path` 目前是私有 `fn path`；将其提升为 `pub(crate) fn path` 供 background.rs 复用（或在 background.rs 内 `root.join("workgraph.json")`，避免暴露）。选后者以最小化可见性变更：`let bad = root.join("workgraph.json");`。

## 四、#5 — reason.rs 走 with_lock

**问题**: `src/tool/reason.rs:172-175` 用裸 `WorkGraph::read` + `wg.add` + `wg.save`，绕过 `with_lock`（分析 spec foot-gun #5 / 违反 ADR 0035"三写入点统一"）。与 daemon tick 或 `milestone` 工具并发时可丢更新。

**改动**: `to_milestone` 的写入改为 `WorkGraph::with_lock(ctx.root, |g| { let new_id = g.add(&title, &acceptance, vec![])?; Ok(new_id) })`，返回 `new_id` 用于成功消息；对齐 `src/tool/dev.rs` 的 `milestone` 工具写法。`read` 出的 `wg` 仅用于取节点信息（title/margin/leverage）——保留该只读读取，写入换成 with_lock。

## 五、测试

- **#1**（`src/background.rs` tests）:
  - 空图跑 `run_background_cfg`（workgraph 模式，空 dir 无 workgraph.json）→ 断言 `mission_state == MissionState::EmptyGraph`。
  - `src/bg_ledger.rs` tests：`mission_exit_code(&MissionState::EmptyGraph) == 5`。
  - 回归：有节点全 Done 的图仍 `CompletedAllReady`（现有测试不回归）。
- **#3**（`src/workgraph.rs` tests + `src/background.rs` tests）:
  - `read_checked`：缺文件 → `Ok(nodes.is_empty())`；写入 `"{not json"` → `Err`；写入 `schema_version` 大于 `WG_SCHEMA_VERSION` 的 JSON → `Err`。
  - `run_background_cfg` 对一个损坏 `workgraph.json` → 断言 `mission_state` 为 `Error(_)`、原文件已重命名为 `workgraph.json.corrupt.<pid>`、`workgraph.json` 不再存在（未被空图覆盖）。
- **#5**（`src/tool/reason.rs` tests）:
  - 现有 `to_milestone_creates_milestone_from_locked_node` 仍通过（换 with_lock 后行为不变）。

全量 `cargo test` 保持绿（当前 master 稳定）。

## 六、文档

- `docs/adr/0033-bg-ledger-and-exit-codes.md`：退出码表增补 `EmptyGraph → 5`；说明"空图 = headless 无事可做的诚实失败，区别于 CompletedAllReady 的真完成"。
- `docs/superpowers/specs/2026-07-25-workgraph-initialization-analysis.md`：foot-gun #1/#3/#5 标注"已修（见本 spec）"；#5 备注 reason.rs 已统一 with_lock。
- `CLAUDE.md` / `README.md`：若引用 BG 退出码语义（0/2/3/4），补 5=EmptyGraph。

## 七、风险与边界

- **对外契约变更**: 新增退出码 5——已获用户批准。旧脚本若把"非 0 即失败"当真，行为更正确（空图本就不该算成功）。
- **`read_checked` 的 ripple 受控**: 只改写入型入口（BG 入口 + advance/retry），只读展示路径仍用 infallible `read()`——不波及系统提示构建等热路径。
- **备份不覆盖**: 中止路径只 `rename`（备份）不 `save`，杜绝数据丢失；`rename` 失败（罕见）也不 `save`，仍以 Error 中止。
- **不碰安全边界与已刻意保留项**: 不做自动播种（B）、不动 id 分配（#4）、不统一写回语义（#6/#7）。
