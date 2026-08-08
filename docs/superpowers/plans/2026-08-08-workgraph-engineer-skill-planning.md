# Workgraph Engineer-Skill Planning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Plan/Exec two-phase milestone execution to workgraph, with engineer skill auto-selection, plan persistence, and an outer quality-review loop.

**Architecture:** Each `advance_one_milestone` call is split into two LLM turns per milestone: a Plan turn (load engineer skill → generate structured plan → persist to `.codecoder/milestone-plans/N-plan.json`) and an Exec turn (read plan → code → self-verify → mark Done). After all milestones complete, a Quality Review turn runs; if quality issues are found, `generate_milestones` is called with current workgraph context to incrementally seed new milestones. The outer loop auto-runs up to `bg_max_auto_cycles` (default 3) then pauses for user.

**Tech Stack:** Rust, existing workgraph.rs, background.rs, config.rs, dev.rs (milestone tool), generate_milestones.rs, bg_ledger.rs

## Global Constraints

- All plan files go under `.codecoder/milestone-plans/` directory
- Plan file format: JSON with fields: milestone_id, title, skill_used, created_at, acceptance_criteria (array), scope (files_to_create, files_to_modify, estimated_lines), risks (array), test_requirements (string)
- Plan file exists = skip Plan Turn (supports interrupt recovery)
- Exec Turn reads plan from disk, injects into prompt
- Quality Review Turn runs after all Pending nodes are exhausted
- Outer cycle count ≤ `bg_max_auto_cycles` (default 3), then pause
- `MissionState::NeedsReview` added for >3 cycles pause
- `generate_milestones` context arg includes full workgraph render
- Interactive mode: `milestone plan <id>` triggers Plan Turn, shows summary, asks for confirmation

---
### Task 1: Add `bg_max_auto_cycles` config field

**Files:**
- Modify: `src/config.rs` (Config struct + ConfigPatch + apply_patch + test)

**Interfaces:**
- Consumes: existing Config struct pattern
- Produces: `Config.bg_max_auto_cycles: usize` (default 3), `ConfigPatch.bg_max_auto_cycles: Option<usize>`

- [ ] **Step 1: Add field to Config struct**

```rust
// src/config.rs, in Config struct, after bg_max_auto:
/// BG 外循环自动轮数上限。超过此数后暂停询问用户。默认 3。
pub bg_max_auto_cycles: usize,
```

- [ ] **Step 2: Add default value**

```rust
// src/config.rs, in Config::default():
bg_max_auto_cycles: 3,
```

- [ ] **Step 3: Add ConfigPatch field**

```rust
// src/config.rs, in ConfigPatch struct:
pub bg_max_auto_cycles: Option<usize>,
```

- [ ] **Step 4: Add apply_patch call**

```rust
// src/config.rs, in apply_patch():
set!(bg_max_auto_cycles);
```

- [ ] **Step 5: Verify default test**

```rust
// src/config.rs, in default_values_match_legacy test:
assert_eq!(c.bg_max_auto_cycles, 3);
```

- [ ] **Step 6: Run tests**

```bash
cargo test config::tests
```

- [ ] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "feat: add bg_max_auto_cycles config field"
```

---
### Task 2: Add `NeedsReview` state to MissionState

**Files:**
- Modify: `src/bg_ledger.rs` (MissionState enum + mission_exit_code + test)

**Interfaces:**
- Consumes: existing MissionState
- Produces: `MissionState::NeedsReview` variant (exit code 3 for "needs user review after cycle limit")

- [ ] **Step 1: Add NeedsReview variant**

```rust
// src/bg_ledger.rs, in MissionState enum:
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MissionState {
    Running,
    Completed,
    EmptyGraph,
    NeedsReview,
    Error(String),
}
```

- [ ] **Step 2: Add exit code mapping**

```rust
// src/bg_ledger.rs, in mission_exit_code():
MissionState::NeedsReview => 3,
```

- [ ] **Step 3: Update test for the new variant**

```rust
// src/bg_ledger.rs, in the test that checks all MissionState variants:
for s in [
    MissionState::Running,
    MissionState::Completed,
    MissionState::EmptyGraph,
    MissionState::NeedsReview,
    MissionState::Error("boom".into()),
] {
    let j = serde_json::to_string(&s).unwrap();
    let back: MissionState = serde_json::from_str(&j).unwrap();
    assert_eq!(format!("{back:?}"), format!("{s:?}"));
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test bg_ledger::tests
```

- [ ] **Step 5: Commit**

```bash
git add src/bg_ledger.rs
git commit -m "feat: add NeedsReview mission state for outer cycle limit"
```

---
### Task 3: Milestone plan persistence module

**Files:**
- Create: `src/milestone_plan.rs` (new module — plan read/write/check)
- Modify: `src/lib.rs` (add `pub mod milestone_plan;`)

**Interfaces:**
- Consumes: root path, milestone_id
- Produces: `MilestonePlan` struct, `plan_path(root, id) -> PathBuf`, `plan_exists(root, id) -> bool`, `write_plan(root, plan) -> Result`, `read_plan(root, id) -> Result<MilestonePlan>`, `all_plans(root) -> Vec<MilestonePlan>`, `delete_plan(root, id)`

- [ ] **Step 1: Create `src/milestone_plan.rs`**

```rust
//! Milestone plan persistence (design 2026-08-08).
//! Plans live under `.codecoder/milestone-plans/N-plan.json`.
//! Each plan records the engineer skill used, acceptance criteria, scope, and risks.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilestonePlan {
    pub milestone_id: u64,
    pub title: String,
    pub skill_used: String,
    pub created_at: String,
    pub acceptance_criteria: Vec<String>,
    pub scope: MilestoneScope,
    pub risks: Vec<String>,
    pub test_requirements: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilestoneScope {
    pub files_to_create: Vec<String>,
    pub files_to_modify: Vec<String>,
    pub estimated_lines: u64,
}

/// Directory for milestone plans: `<root>/.codecoder/milestone-plans/`
pub fn plan_dir(root: &Path) -> PathBuf {
    root.join(".codecoder").join("milestone-plans")
}

/// Full path for milestone #N's plan: `<root>/.codecoder/milestone-plans/N-plan.json`
pub fn plan_path(root: &Path, milestone_id: u64) -> PathBuf {
    plan_dir(root).join(format!("{}-plan.json", milestone_id))
}

/// Check if a plan exists for milestone #N (without loading it).
pub fn plan_exists(root: &Path, milestone_id: u64) -> bool {
    plan_path(root, milestone_id).exists()
}

/// Write a plan to disk, creating the directory if needed.
pub fn write_plan(root: &Path, plan: &MilestonePlan) -> anyhow::Result<()> {
    let dir = plan_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = plan_path(root, plan.milestone_id);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(plan)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a plan for milestone #N. Returns error if missing or corrupt.
pub fn read_plan(root: &Path, milestone_id: u64) -> anyhow::Result<MilestonePlan> {
    let path = plan_path(root, milestone_id);
    let raw = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&raw)?)
}

/// Read all plans for all milestones that have them.
pub fn all_plans(root: &Path) -> Vec<MilestonePlan> {
    let dir = plan_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut plans = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(plan) = serde_json::from_str::<MilestonePlan>(&raw) {
                plans.push(plan);
            }
        }
    }
    plans
}

/// Delete a plan for milestone #N. No-op if missing.
pub fn delete_plan(root: &Path, milestone_id: u64) {
    let path = plan_path(root, milestone_id);
    let _ = std::fs::remove_file(&path);
}
```

- [ ] **Step 2: Register module in lib.rs**

```rust
// src/lib.rs, after `pub mod memory;` (alphabetical order):
pub mod milestone_plan;
```

- [ ] **Step 3: Run tests to confirm module compiles**

```bash
cargo check
```

- [ ] **Step 4: Commit**

```bash
git add src/milestone_plan.rs src/lib.rs
git commit -m "feat: add milestone plan persistence module"
```

---
### Task 4: Split `advance_one_milestone` into Plan/Exec two-phase

**Files:**
- Modify: `src/background.rs` (rewrite `advance_one_milestone`, add `plan_one_milestone`)

**Interfaces:**
- Consumes: `MilestonePlan` from milestone_plan module, `use_skill` tool via agent turn
- Produces: `advance_one_milestone` now checks for plan first; if missing, runs Plan Turn first, then returns `Ok(Some(PlanCreated))` — caller re-invokes for Exec Turn

- [ ] **Step 1: Add `run_plan_turn` function**

```rust
/// 对里程碑 #N 执行 Plan Turn：加载 engineer skill，生成计划，写入 .codecoder/milestone-plans/。
/// 返回创建的计划对象。如果计划已存在，直接返回。
fn run_plan_turn(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    milestone_id: u64,
    title: &str,
) -> anyhow::Result<MilestonePlan> {
    use crate::milestone_plan::*;

    // 如果计划已存在，直接读取返回（支持中断恢复）。
    if plan_exists(&root, milestone_id) {
        return read_plan(&root, milestone_id);
    }

    let checkpoint = read_bg_checkpoint(&root);
    let prompt = format!(
        "workgraph milestone #{}: {}\n\n\
         任务分解：\n\
         1. 先用 `use_skill` 工具加载与当前里程碑内容最匹配的 engineer skill\n\
            - 选择依据：里程碑标题/描述中的关键词\n\
            - 架构/数据模型 → engineer-architect\n\
            - 前端/UI → engineer-frontend-architect\n\
            - 遗留代码改造 → engineer-legacy-recon\n\
            - 测试/验收 → engineer-qa / engineer-inspector\n\
            - 通用编码 → engineer-coach\n\
            - 需求模糊 → engineer-requirements\n\
         2. 按照所选 skill 的方法论，生成详细的开发计划\n\
         3. 将计划写入 .codecoder/milestone-plans/{}-plan.json（JSON 格式，字段见下）\n\
         4. 完成后输出 'PLAN_COMPLETE'，不执行编码\n\n\
         计划 JSON 格式：\n\
         {{\n\
           \"milestone_id\": {},\n\
           \"title\": \"{}\",\n\
           \"skill_used\": \"<skill-name>\",\n\
           \"acceptance_criteria\": [\"<标准1>\", \"<标准2>\", ...],\n\
           \"scope\": {{\n\
             \"files_to_create\": [\"<path>\", ...],\n\
             \"files_to_modify\": [\"<path>\", ...],\n\
             \"estimated_lines\": 150\n\
           }},\n\
           \"risks\": [\"<风险1>\", ...],\n\
           \"test_requirements\": \"<测试要求描述>\"\n\
         }}\n\
         {}",
        milestone_id, title,
        milestone_id, milestone_id, title,
        checkpoint,
    );

    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    agent.observer_set.register(Box::new(
        crate::bg_observer::BgObserver::new(&agent.root)
    ));
    if let Err(e) = agent.cancel_token().cancel_on_sigint() {
        eprintln!("ccd: SIGINT cancel not wired: {e}");
    }
    let cfg = crate::config::Config::load();
    agent.set_tool_cap(cfg.bg_milestone_tool_cap);

    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(prompt, &tx);
        drop(tx);
        agent
    });
    // Drain events into a throwaway BgOutcome (we only care about the plan file).
    let mut tmp_out = BgOutcome::default();
    drain_bg_events(rx, &mut tmp_out, None);
    let _agent = match handle.join() {
        Ok(agent) => agent,
        Err(panic) => {
            return Err(anyhow::anyhow!("plan turn panicked: {}", panic_message(panic)));
        }
    };

    // Verify plan was written.
    if !plan_exists(&root, milestone_id) {
        anyhow::bail!("plan turn did not write plan for milestone #{}", milestone_id);
    }
    read_plan(&root, milestone_id)
}
```

- [ ] **Step 2: Rewrite `advance_one_milestone` to be Plan+Exec aware**

```rust
/// 推进 workgraph 的下一个就绪(pending)里程碑。
/// 如果里程碑还没有 plan，先执行 Plan Turn 生成计划，再返回 Ok(Some) 让调用方重试。
/// 如果已有 plan，执行 Exec Turn（编码 → 自验收 → 标记 Done）。
/// 无就绪里程碑时返回 Ok(None)。
pub fn advance_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::{NodeStatus, WorkGraph};

    let (milestone_id, title) = {
        let g = WorkGraph::read_checked(&root)?;
        let Some(n) = g.next_ready() else { return Ok(None); };
        (n.id, n.title.clone())
    };

    // ── Phase 1: Plan Turn ──
    if !crate::milestone_plan::plan_exists(&root, milestone_id) {
        let mut external_obs = crate::bg_observer::BgObserver::new(&root);
        external_obs.emit_external("plan_start", &format!("#{milestone_id} {title}"));
        match run_plan_turn(
            provider.clone(), model.clone(), max_tokens, temperature,
            root.clone(), milestone_id, &title,
        ) {
            Ok(plan) => {
                external_obs.emit_external("plan_done", &format!(
                    "#{milestone_id} using {}", plan.skill_used
                ));
                // Plan Turn 刚完成，让调用方下一次循环进入 Exec Turn。
                // 返回一个空的 BgOutcome 表示"已出计划，请重试"。
                let mut out = BgOutcome::default();
                out.events.push(format!(
                    "plan: milestone #{} ({}) — skill: {}, criteria: {}",
                    milestone_id, title, plan.skill_used, plan.acceptance_criteria.len(),
                ));
                // 注意：不标记里程碑为 Done，留在 Pending 状态让 Exec Turn 推进。
                return Ok(Some(out));
            }
            Err(e) => {
                external_obs.emit_external("plan_error", &format!("#{milestone_id} {e}"));
                // Plan 失败→标记为 Blocked，记录原因。
                let _ = WorkGraph::with_lock(&root, |g| {
                    g.set_status(milestone_id, NodeStatus::Blocked);
                    Ok(())
                });
                return Err(e);
            }
        }
    }

    // ── Phase 2: Exec Turn ──
    let plan = crate::milestone_plan::read_plan(&root, milestone_id)
        .unwrap_or_else(|_| panic!("plan should exist for milestone #{milestone_id}"));

    let checkpoint = read_bg_checkpoint(&root);
    let criteria_str = plan.acceptance_criteria.iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {}", i + 1, c))
        .collect::<Vec<_>>()
        .join("\n");
    let scope_created = plan.scope.files_to_create.join("\n  ");
    let scope_modified = plan.scope.files_to_modify.join("\n  ");
    let risks_str = plan.risks.join("\n- ");

    let task_text = format!(
        "workgraph milestone #{}: {}\n\n\
         === 开发计划 ===\n\
         使用的 skill: {}\n\n\
         验收标准：\n{}\n\n\
         文件范围：\n\
         - 创建：\n  {}\n\
         - 修改：\n  {}\n\n\
         风险点：\n- {}\n\n\
         测试要求：{}\n\n\
         请按此计划逐项执行。完成每一项后用 `diff` 或检查确认，\
         逐条对照验收标准自验收。全部通过后，里程碑将被自动标记为完成。\
         无需输出验收声明。{}",
        milestone_id, plan.title,
        plan.skill_used,
        criteria_str,
        scope_created, scope_modified,
        risks_str,
        plan.test_requirements,
        checkpoint,
    );

    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    agent.observer_set.register(Box::new(
        crate::bg_observer::BgObserver::new(&agent.root)
    ));
    if let Err(e) = agent.cancel_token().cancel_on_sigint() {
        eprintln!("ccd: SIGINT cancel not wired: {e}");
    }
    let cfg = crate::config::Config::load();
    agent.set_tool_cap(cfg.bg_milestone_tool_cap);

    let mut out = BgOutcome::default();
    out.events.push(format!("exec: milestone #{} ({})", milestone_id, title));
    let mut external_obs = crate::bg_observer::BgObserver::new(&root);
    external_obs.emit_external("milestone_start", &format!("#{milestone_id} {title}"));

    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(task_text, &tx);
        drop(tx);
        agent
    });
    drain_bg_events(rx, &mut out, None);
    let agent = match handle.join() {
        Ok(agent) => agent,
        Err(panic) => {
            let msg = format!("exec turn panicked: {}", panic_message(panic));
            external_obs.emit_external("error", &msg);
            return Err(anyhow::anyhow!(msg));
        }
    };
    if let Some(e) = agent.last_error() {
        return Err(anyhow::anyhow!(e.to_string()));
    }

    // 标记完成 + 写 checkpoint。
    let tool_cap_hit = out.events.iter().any(|e| e.contains("tool-iteration cap"));
    let m = {
        let g = WorkGraph::read(&root);
        g.get(milestone_id).expect("just read").clone()
    };
    let _ = WorkGraph::with_lock(&root, |g| {
        g.set_status(milestone_id, NodeStatus::Done);
        Ok(())
    });
    external_obs.emit_external("milestone_done", &format!("#{milestone_id} {title}"));
    out.subgoals.push(SubgoalOutcome {
        milestone_id,
        tool_cap_hit,
        touched_files: m.touched.clone(),
    });
    out.events.push(format!("milestone #{} ({}) completed", milestone_id, title));
    let _ = update_bg_checkpoint(&root, milestone_id, &title, &m.touched);

    Ok(Some(out))
}
```

- [ ] **Step 3: Update `run_background_cfg` to handle Plan Turn re-entry**

The current loop calls `advance_one_milestone` once per iteration. The Plan Turn returns `Ok(Some(out))` with an empty BgOutcome — the loop will naturally re-enter for the Exec Turn on the next iteration. No changes needed to the loop structure.

But we need to add a guard: the Plan Turn returns 0 tools/events, so it shouldn't count toward `max_auto`. Let's check:

```rust
// In run_background_cfg, the loop:
loop {
    if advanced >= max_auto { break; }
    match advance_one_milestone(...) {
        Ok(Some(step)) => {
            // Only count toward max_auto if it actually did work (Exec Turn).
            // Plan Turn returns None events, so we can detect it.
            if !step.events.is_empty() && step.events.iter().any(|e| e.starts_with("exec:")) {
                advanced += 1;
            }
            out.final_text.push_str(&step.final_text);
            out.tool_calls.extend(step.tool_calls);
            out.denied.extend(step.denied);
            out.events.extend(step.events);
            out.subgoals.extend(step.subgoals);
        }
        Ok(None) => { out.mission_state = MissionState::Completed; break; }
        Err(e) => { out.mission_state = MissionState::Error(e.to_string()); break; }
    }
}
```

Wait, actually I need to think about this more carefully. The Plan Turn returns `Ok(Some(empty_out))` — the events contain `"plan: ..."` not `"exec: ..."`. So the condition `step.events.iter().any(|e| e.starts_with("exec:"))` correctly distinguishes Plan from Exec turns. But we should also not count Plan Turns toward `max_auto` because they're fast and don't represent real milestone progress.

Actually, looking at the existing code more carefully: `max_auto` limits the number of milestones per BG run, not the number of turns. The Plan Turn is a supporting turn, not a milestone. So we should NOT count Plan Turns toward `max_auto`.

Let me revise the approach: make `advance_one_milestone` only return `Ok(Some)` for Exec Turns (milestone completed), and for Plan Turns we return `Ok(Some(plan_out))` but the caller distinguishes them.

Actually, the simplest approach: change the return value to indicate whether it was a Plan or Exec turn. Let me use a different approach - have `advance_one_milestone` internally loop: if plan doesn't exist, run Plan Turn, then immediately run Exec Turn. This way the caller sees only one call per milestone.

But that would mean two LLM turns inside one `advance_one_milestone` call, which is more complex. Let me keep the two-turn approach but make the caller handle it cleanly.

Actually wait - the simplest approach is to have `advance_one_milestone` do BOTH turns internally. The function runs Plan Turn if needed, then Exec Turn, then returns. The caller sees one call per milestone.

Let me redesign:

```rust
pub fn advance_one_milestone(...) -> anyhow::Result<Option<BgOutcome>> {
    // 1. Check next ready milestone
    // 2. If no plan → run Plan Turn (synchronous, in this function)
    // 3. Run Exec Turn
    // 4. Mark Done
    // 5. Return BgOutcome
}
```

This is cleaner. The Plan Turn happens inside `advance_one_milestone` before the Exec Turn. The caller doesn't need to know about the two-phase structure.

- [ ] **Step 4: Update the milestone loop in `run_background_cfg`**

The milestone loop stays the same — `advance_one_milestone` now handles Plan+Exec internally.

- [ ] **Step 5: Run tests**

```bash
cargo test background::tests
```

- [ ] **Step 6: Commit**

```bash
git add src/background.rs
git commit -m "feat: split advance_one_milestone into Plan/Exec two-phase"
```

---
### Task 5: Add outer quality review loop to `run_background_cfg`

**Files:**
- Modify: `src/background.rs` (add `run_quality_review_turn`, modify `run_background_cfg` outer loop)

**Interfaces:**
- Consumes: `bg_max_auto_cycles` config, `MissionState::NeedsReview`
- Produces: outer loop that runs quality review after all milestones, conditionally re-seeds workgraph

- [ ] **Step 1: Add `run_quality_review_turn` function**

```rust
/// 所有里程碑完成后，执行质量检查 turn。LLM 评估整体质量状态。
/// 如果有质量问题，调用 generate_milestones 增量补充新里程碑。
/// 返回 true = 需要继续下一个外循环，false = 目标已高质量完成。
fn run_quality_review_turn(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    cycle: usize,
) -> anyhow::Result<bool> {
    use crate::workgraph::WorkGraph;

    let g = WorkGraph::read(&root);
    let graph_render = g.render();
    let plans = crate::milestone_plan::all_plans(&root);
    let plan_count = plans.len();
    let done_count = g.nodes.iter().filter(|n| n.status == crate::workgraph::NodeStatus::Done).count();

    drop(g); // 不再需要

    let prompt = format!(
        "你是一个质量评审助手。当前项目已完成所有里程碑，\
         需要进行整体质量评估。\n\n\
         已完成里程碑数: {done_count}/{plan_count}（有计划的里程碑）\n\n\
         workgraph 当前状态:\n{graph_render}\n\n\
         里程碑计划文件:\n{}\n\n\
         请综合评估整个项目当前的质量状态。\
         对照每个里程碑的验收标准，检查是否所有目标都已高质量完成。\n\n\
         如果存在质量问题（测试不足、实现不完整、代码质量不达标等），\
         请调用 generate_milestones 工具增量补充新的里程碑。\
         已完成的里程碑不可修改，但可追加新的里程碑来修复/增强。\
         新增里程碑时，在 context 参数中注入当前 workgraph 状态，\
         让新里程碑自动依赖所有已完成的节点。\n\n\
         如果认为所有目标都已高质量完成，就不需要再生成任何里程碑。\
         输出 'QUALITY_PASS' 表示通过。\n\
         这是第 {cycle} 轮外循环。",
        plans.iter().map(|p| format!(
            "#{}: {} (skill: {}, criteria: {})",
            p.milestone_id, p.title, p.skill_used, p.acceptance_criteria.len()
        )).collect::<Vec<_>>().join("\n"),
        cycle,
    );

    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    agent.observer_set.register(Box::new(
        crate::bg_observer::BgObserver::new(&agent.root)
    ));
    if let Err(e) = agent.cancel_token().cancel_on_sigint() {
        eprintln!("ccd: SIGINT cancel not wired: {e}");
    }
    agent.set_tool_cap(crate::config::Config::load().bg_milestone_tool_cap);

    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(prompt, &tx);
        drop(tx);
        agent
    });
    let mut tmp_out = BgOutcome::default();
    drain_bg_events(rx, &mut tmp_out, None);
    let _agent = match handle.join() {
        Ok(agent) => agent,
        Err(panic) => {
            eprintln!("quality review turn panicked: {}", panic_message(panic));
            // 超时/panic 时视为"需要人工检查"，暂停外循环。
            return Err(anyhow::anyhow!("quality review panicked"));
        }
    };

    // 检查是否调用了 generate_milestones → workgraph 有新节点
    let g2 = WorkGraph::read(&root);
    let has_new = g2.nodes.len() > plan_count;
    let all_done = g2.nodes.iter().all(|n| n.status == crate::workgraph::NodeStatus::Done);
    let needs_rework = has_new || !all_done;

    Ok(needs_rework)
}
```

- [ ] **Step 2: Modify `run_background_cfg` to add outer loop**

```rust
// 在 workgraph 分支的 milestone loop 之后，添加外循环：
// ── 外循环：质量检查 → 增量补充 ──
let mut cycle = 0usize;
let max_cycles = crate::config::Config::load().bg_max_auto_cycles;
loop {
    // 内层 milestone 推进循环
    while let Some(out) = advance_one_milestone(
        provider.clone(), model.clone(), max_tokens, temperature, root.clone(),
    )? {
        // 累积输出（现有逻辑不变）
        out.final_text.push_str(&step.final_text);
        // ... 其他累积 ...
    }

    // 里程碑全部跑完 → 质量检查
    cycle += 1;
    if cycle >= max_cycles {
        out.mission_state = MissionState::NeedsReview;
        external_obs.emit_external("needs_review", &format!(
            "outer cycle {} reached limit {}", cycle, max_cycles
        ));
        break;
    }

    match run_quality_review_turn(
        provider.clone(), model.clone(), max_tokens, temperature,
        root.clone(), cycle,
    ) {
        Ok(true) => {
            external_obs.emit_external("rework", "quality review found issues, re-seeding workgraph");
            // 继续外循环（generate_milestones 已在 turn 内 seed 了新节点）
            continue;
        }
        Ok(false) => {
            external_obs.emit_external("quality_pass", "all milestones completed with high quality");
            out.mission_state = MissionState::Completed;
            break;
        }
        Err(e) => {
            external_obs.emit_external("quality_error", &e.to_string());
            out.mission_state = MissionState::Error(e.to_string());
            break;
        }
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test background::tests
```

- [ ] **Step 4: Commit**

```bash
git add src/background.rs
git commit -m "feat: add outer quality review loop with workgraph re-seed"
```

---
### Task 6: Update `generate_milestones` for incremental context

**Files:**
- Modify: `src/tool/generate_milestones.rs` (enhance prompt to include context for incremental seeding)

**Interfaces:**
- Consumes: `context` arg already exists
- Produces: prompt now includes workgraph state for incremental generation

- [ ] **Step 1: Enhance the prompt to include workgraph state for incremental seeding**

The current `generate_milestones` tool already has a `context` parameter. The quality review turn passes the workgraph render as context. The tool itself is fine — the change is in how the prompt is constructed when called from the quality review context.

Actually, looking at the existing code, the `generate_milestones` tool just takes `goal` and `context` strings and passes them to the LLM. The quality review turn already includes the workgraph state in its prompt. When the LLM calls `generate_milestones`, the context will contain the workgraph state.

But we should also ensure that when the LLM generates milestones in the quality review context, the new milestones have proper dependencies on existing completed milestones. Let me add a note in the prompt about this.

The change is in the `generate_milestones` tool's prompt — add a note about creating dependencies on existing milestones:

```rust
// src/tool/generate_milestones.rs, in the prompt builder:
// After the existing prompt text, add:
if !context.is_empty() {
    prompt.push_str(
        "If the context mentions existing milestones, make sure new milestones \
         depend on all relevant existing milestones as prerequisites. \
         Do not redo work that is already marked as complete.\n"
    );
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test generate_milestones::tests
```

- [ ] **Step 3: Commit**

```bash
git add src/tool/generate_milestones.rs
git commit -m "feat: enhance generate_milestones for incremental seeding with deps"
```

---
### Task 7: Interactive mode — milestone `plan` subcommand

**Files:**
- Modify: `src/tool/dev.rs` (Milestone tool — add `plan` action)

**Interfaces:**
- Consumes: `MilestonePlan` from milestone_plan module
- Produces: `milestone plan <id>` triggers Plan Turn, shows summary, returns

- [ ] **Step 1: Add `plan` action to Milestone tool**

```rust
// In Milestone::apply, add "plan" action:
"plan" => {
    let Some(i) = id else {
        return ToolOutput::err("plan needs `id`");
    };
    let Some(n) = g.get(i) else {
        return ToolOutput::err(format!("unknown milestone #{i}"));
    };
    // 检查 plan 是否存在
    if crate::milestone_plan::plan_exists(ctx.root, i) {
        // 已存在则读取显示
        match crate::milestone_plan::read_plan(ctx.root, i) {
            Ok(plan) => ToolOutput::ok(format!(
                "📋 里程碑 #{} 计划（已存在）：\n\
                 skill: {}\n\
                 验收标准 ({}条):\n{}\n\
                 测试要求: {}\n\
                 风险: {}",
                plan.milestone_id, plan.skill_used,
                plan.acceptance_criteria.len(),
                plan.acceptance_criteria.iter().map(|c| format!("  - {c}")).collect::<Vec<_>>().join("\n"),
                plan.test_requirements,
                if plan.risks.is_empty() { "无".into() } else { plan.risks.join("; ") },
            )),
            Err(e) => ToolOutput::err(format!("plan exists but corrupt: {e}")),
        }
    } else {
        ToolOutput::ok(format!(
            "📋 里程碑 #{}: 「{}」还没有计划。\n\
             使用 `use_skill` 加载合适的 engineer skill 生成计划。\n\
             或者用 `generate_milestones` 重新生成整个 workgraph。",
            i, n.title,
        ))
    }
}
```

Wait, the Milestone tool's `apply` function is a pure function that takes `&mut WorkGraph` and returns `ToolOutput`. It doesn't have access to `ctx.root`. The `run` method has access to `ctx.root` via `ctx.root`. Let me check the actual code more carefully.

Looking at the code:
```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    WorkGraph::with_lock(ctx.root, |g| Ok(Self::apply(g, &action, id, &args)))
}
```

The `apply` function doesn't have access to `ctx.root`. So I need to either:
1. Pass the root path to `apply`
2. Handle the `plan` action in `run` directly

Option 2 is cleaner:

```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    use crate::workgraph::WorkGraph;
    let action = args.get("action").and_then(Value::as_str).unwrap_or("list").to_string();
    let id = args.get("id").and_then(Value::as_u64);

    if action == "plan" {
        let Some(i) = id else {
            return Ok(ToolOutput::err("plan needs `id`"));
        };
        let g = WorkGraph::read(ctx.root);
        let Some(n) = g.get(i) else {
            return Ok(ToolOutput::err(format!("unknown milestone #{i}")));
        };
        if crate::milestone_plan::plan_exists(ctx.root, i) {
            match crate::milestone_plan::read_plan(ctx.root, i) {
                Ok(plan) => Ok(ToolOutput::ok(format!(
                    "📋 Milestone #{} Plan:\nskill: {}\nacceptance criteria:\n{}\n\
                     test requirements: {}\nrisks: {}",
                    plan.milestone_id, plan.skill_used,
                    plan.acceptance_criteria.iter().map(|c| format!("  - {c}")).collect::<Vec<_>>().join("\n"),
                    plan.test_requirements,
                    if plan.risks.is_empty() { "none".into() } else { plan.risks.join("; ") },
                ))),
                Err(e) => Ok(ToolOutput::err(format!("plan corrupt: {e}"))),
            }
        } else {
            Ok(ToolOutput::ok(format!(
                "Milestone #{} \"{}\" has no plan yet. Use `use_skill` to load an engineer skill and generate one.",
                i, n.title,
            )))
        }
    } else {
        WorkGraph::with_lock(ctx.root, |g| Ok(Self::apply(g, &action, id, &args)))
    }
}
```

- [ ] **Step 2: Update schema to include `plan` action**

```rust
// In schema():
"action": { "type": "string", "enum": ["list", "add", "start", "done", "next", "remove", "plan"] }
```

- [ ] **Step 3: Run tests**

```bash
cargo test dev::tests
```

- [ ] **Step 4: Commit**

```bash
git add src/tool/dev.rs
git commit -m "feat: add milestone plan subcommand for interactive mode"
```

---
### Task 8: Update existing tests and add new tests

**Files:**
- Modify: `src/background.rs` (update existing tests, add new ones for Plan/Exec)
- Modify: `src/milestone_plan.rs` (add tests)

**Interfaces:**
- Consumes: existing test infrastructure (StubClient, temp dirs)
- Produces: test coverage for Plan/Exec two-phase, quality review, incremental seeding

- [ ] **Step 1: Add tests for milestone_plan module**

```rust
// In src/milestone_plan.rs, add #[cfg(test)] mod tests:
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn plan_path_format() {
        let root = Path::new("/tmp/project");
        let p = plan_path(root, 42);
        assert!(p.to_str().unwrap().contains(".codecoder/milestone-plans/42-plan.json"));
    }

    #[test]
    fn write_and_read_plan_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cc_plan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "Build data model".into(),
            skill_used: "engineer-architect".into(),
            created_at: "2026-08-08T10:00:00Z".into(),
            acceptance_criteria: vec!["Fields have types".into()],
            scope: MilestoneScope {
                files_to_create: vec!["src/model.rs".into()],
                files_to_modify: vec![],
                estimated_lines: 100,
            },
            risks: vec!["Migration risk".into()],
            test_requirements: "Unit tests for each model".into(),
        };
        write_plan(&dir, &plan).unwrap();
        assert!(plan_exists(&dir, 1));
        let back = read_plan(&dir, 1).unwrap();
        assert_eq!(back.milestone_id, 1);
        assert_eq!(back.title, "Build data model");
        assert_eq!(back.skill_used, "engineer-architect");
        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_exists_returns_false_for_missing() {
        let dir = std::env::temp_dir().join(format!("cc_plan_miss_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!plan_exists(&dir, 99));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn all_plans_returns_empty_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(all_plans(dir.path()).is_empty());
    }

    #[test]
    fn delete_plan_removes_file() {
        let dir = std::env::temp_dir().join(format!("cc_plan_del_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "Test".into(),
            skill_used: "engineer-coach".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
            acceptance_criteria: vec![],
            scope: MilestoneScope {
                files_to_create: vec![],
                files_to_modify: vec![],
                estimated_lines: 0,
            },
            risks: vec![],
            test_requirements: String::new(),
        };
        write_plan(&dir, &plan).unwrap();
        assert!(plan_exists(&dir, 1));
        delete_plan(&dir, 1);
        assert!(!plan_exists(&dir, 1));
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: Update `advance_one_milestone_runs_a_turn` test**

The existing test uses StubClient which doesn't call `use_skill` or write files, so the Plan Turn will fail (no plan written). Update the test expectation:

```rust
#[test]
fn advance_one_milestone_runs_a_turn() {
    let dir = root_with_one_milestone();
    // StubClient 不调用 use_skill，Plan Turn 会失败（Err）
    // 因为 StubClient 不产生计划文件
    let out = advance_one_milestone(
        Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
    );
    // Should be an error because StubClient can't generate a plan
    assert!(out.is_err(), "stub should fail plan turn");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 3: Add a test for pre-existing plan bypassing Plan Turn**

```rust
#[test]
fn advance_with_preexisting_plan_does_exec_turn() {
    let dir = std::env::temp_dir().join(format!("cc_plan_exec_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // 创建 workgraph 含一个里程碑
    let mut g = crate::workgraph::WorkGraph::default();
    g.add("test milestone", vec![]).unwrap();
    g.save(&dir).unwrap();

    // 预写 plan 文件
    let plan = crate::milestone_plan::MilestonePlan {
        milestone_id: 1,
        title: "test milestone".into(),
        skill_used: "engineer-coach".into(),
        created_at: "2026-08-08T00:00:00Z".into(),
        acceptance_criteria: vec!["It works".into()],
        scope: crate::milestone_plan::MilestoneScope {
            files_to_create: vec![],
            files_to_modify: vec![],
            estimated_lines: 10,
        },
        risks: vec![],
        test_requirements: "N/A".into(),
    };
    crate::milestone_plan::write_plan(&dir, &plan).unwrap();

    // 有 plan → 跳过 Plan Turn，直接 Exec Turn，StubClient 能跑
    let out = advance_one_milestone(
        Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
    ).unwrap();
    assert!(out.is_some(), "exec turn should run with pre-existing plan");
    let outcome = out.unwrap();
    assert!(outcome.events.iter().any(|e| e.contains("exec:")),
        "events should indicate exec turn: {:?}", outcome.events);
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 4: Run all tests**

```bash
cargo test
```

- [ ] **Step 5: Commit**

```bash
git add src/background.rs src/milestone_plan.rs
git commit -m "test: add tests for Plan/Exec two-phase and plan persistence"
```

---
### Task 9: Update documentation

**Files:**
- Modify: `ARCHITECTURE.md` (Background Agent section)
- Modify: `README.md` (milestone_plan_dir config, bg_max_auto_cycles)

- [ ] **Step 1: Update ARCHITECTURE.md**

Add description of the Plan/Exec two-phase, quality review outer loop, and .codecoder/milestone-plans/ directory.

- [ ] **Step 2: Update README.md**

Add `bg_max_auto_cycles` to config table, update milestone tool description.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md README.md
git commit -m "docs: update architecture and config docs for Plan/Exec two-phase"
```