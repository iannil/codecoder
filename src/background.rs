// Background Agent runner (ADR 0026): drives one delegated task headless (no TUI,
// no user present), then returns a structured outcome. Scheduling is external.
//
// Task 4: per-milestone gates (acceptance, command, review, needs_fix, retry)
// removed. The loop simply advances ready milestones; the agent self-reports
// completion and the kernel marks them Done.
use crate::agent::{AgentEvent, AgentLoop};
use crate::bg_ledger::MissionState;
use crate::milestone_plan::MilestonePlan;
use crate::provider::Provider;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::channel;

/// 一次 BG 调用中某个 milestone 的结果记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubgoalOutcome {
    pub milestone_id: u64,
    pub tool_cap_hit: bool,
    pub touched_files: Vec<String>,
}

/// The result of one headless Background Agent turn.
#[derive(Debug)]
pub struct BgOutcome {
    /// The final assistant text of the turn.
    pub final_text: String,
    /// Names of tools that actually executed (in order).
    pub tool_calls: Vec<String>,
    /// Tool outputs that reported an error (includes headless denials).
    pub denied: Vec<String>,
    /// Human-readable milestone lines.
    pub events: Vec<String>,
    /// 每个 milestone 的结果记录(空 = 非 milestone 模式)。
    pub subgoals: Vec<SubgoalOutcome>,
    /// 整次 BG 调用的任务终态(spec 2026-07-22)。
    pub mission_state: MissionState,
}

impl Default for BgOutcome {
    fn default() -> Self {
        Self {
            final_text: String::new(),
            tool_calls: vec![],
            denied: vec![],
            events: vec![],
            subgoals: vec![],
            mission_state: MissionState::Running,
        }
    }
}

/// 读取项目根目录的 AGENTS.md 作为使命描述。若文件不存在或为空，返回通用降级文本。
fn read_mission(root: &Path) -> String {
    let path = root.join("AGENTS.md");
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => "Initialize and develop the project in this directory.".to_string(),
    }
}

/// 空 workgraph 时，通过一个 headless agent turn 调用 generate_milestones 工具
/// 自动分解使命为里程碑并写入 workgraph.json。成功写入返回 true，失败返回 false。
/// 注意：不注册 SIGINT（避免与主循环的 cancel token 冲突）。
pub(crate) fn seed_workgraph_from_mission(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    tool_cap: usize,
) -> bool {
    let mission = read_mission(&root);
    let prompt = format!(
        "你是一个项目规划助手。当前项目是一个空目录，需要你来初始化。\n\n\
         项目使命：\n{}\n\n\
         请先使用 list_directory 工具了解项目结构，然后使用 generate_milestones 工具\
         将上述使命分解为 3-8 个里程碑，每个里程碑包含：\n\
         - title（简短、可行动的标题）\n\n\
         里程碑应按依赖顺序排列，前面的里程碑是后面里程碑的前提。\n\n\
         关键约束（必须遵守）：\n\
         1. 先写 package.json → 再 npm install → 再写源代码 → 最后 npm run build 验证\n\
         2. 首次 commit 前必须 git init\n\
         3. 优先使用内置工具（list_directory 替代 ls，read_file 替代 cat 等）\n\
         4. 避免复合 shell 命令（&&, ||, |, 2>&1）\n\
         5. 超过 200 行的文件分多步写入（write_file + edit_file 追加）\n\
         6. 每个里程碑应产生真实代码，使用 PlaceholderPage 或空壳组件会被拒绝",
        mission
    );

    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    agent.set_tool_cap(tool_cap);
    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(prompt, &tx);
        drop(tx);
        agent
    });
    // Drain events (不收集，seed turn 的日志不重要)
    for _ev in rx.into_iter() {}
    match handle.join() {
        Ok(_agent) => {
            let g = crate::workgraph::WorkGraph::read(&root);
            !g.nodes.is_empty()
        }
        Err(_panic) => false,
    }
}

/// Run one task to completion on the CURRENT thread, then drain events into a
/// BgOutcome. Same-thread + post-turn drain keeps it deterministic (no interleave).
/// When `task` is empty, falls back to the workgraph and auto-advances through
/// ready milestones.
pub fn run_background(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    task: String,
) -> anyhow::Result<BgOutcome> {
    let cfg = crate::config::Config::load();
    run_background_cfg(
        provider, model, max_tokens, temperature, root, task,
        cfg.bg_max_auto, cfg.bg_milestone_tool_cap,
    )
}

/// 与 `run_background` 同,但 caps 显式注入(测试用,避开全局 env 的并行竞态)。
pub(crate) fn run_background_cfg(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    task: String,
    max_auto: usize,
    tool_cap: usize,
) -> anyhow::Result<BgOutcome> {
    let mut out = BgOutcome::default();

    // ── 显式任务分支:跑一 turn,自动推进。──
    if !task.trim().is_empty() {
        out.events.push("task: explicit task".into());
        let mut external_obs = crate::bg_observer::BgObserver::start_run(&root);
        let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root);
        // Register BgObserver in AgentLoop's ObserverSet for auto-emission.
        agent.observer_set.register(Box::new(
            crate::bg_observer::BgObserver::new(&agent.root)
        ));
        // ADR 0026:wire SIGINT → cancel。
        if let Err(e) = agent.cancel_token().cancel_on_sigint() {
            eprintln!("ccd: SIGINT cancel not wired: {e}");
        }
        agent.set_tool_cap(tool_cap);
        let (tx, rx) = channel::<AgentEvent>();
        let handle = std::thread::spawn(move || {
            agent.run_one_turn(task, &tx);
            drop(tx);
            agent
        });
        drain_bg_events(rx, &mut out, None);
        match handle.join() {
            Ok(agent) => {
                if let Some(e) = agent.last_error() {
                    out.mission_state = MissionState::Error(e.to_string());
                }
            }
            Err(panic) => {
                let msg = format!("bg turn thread panicked: {}", panic_message(panic));
                external_obs.emit_external("error", &msg);
                out.mission_state = MissionState::Error(msg);
            }
        }
        return Ok(out);
    }

    // ── Workgraph 分支:milestone 推进循环。──
    // Reset the NDJSON event stream once at run start; per-milestone observers append.
    let mut external_obs = crate::bg_observer::BgObserver::start_run(&root);
    // #3 data-loss guard: a present-but-unreadable workgraph.json must never be
    // silently treated as empty and overwritten — back it up and abort.
    let graph = match crate::workgraph::WorkGraph::read_checked(&root) {
        Ok(g) => g,
        Err(e) => {
            let bad = root.join("workgraph.json");
            let backup = root.join(format!("workgraph.json.corrupt.{}", std::process::id()));
            let _ = std::fs::rename(&bad, &backup);
            let msg = format!("workgraph.json unreadable ({e}); backed up to {}", backup.display());
            external_obs.emit_external("error", &msg);
            out.mission_state = MissionState::Error(msg);
            return Ok(out);
        }
    };
    // #1 empty graph: try to auto-seed from AGENTS.md; fall back to EmptyGraph on failure.
    if graph.nodes.is_empty() {
        external_obs.emit_external("seed", "empty workgraph — attempting to seed from AGENTS.md...");
        let seeded = seed_workgraph_from_mission(
            provider.clone(), model.clone(), max_tokens, temperature, root.clone(), tool_cap,
        );
        if seeded {
            external_obs.emit_external("seed", "workgraph seeded successfully — entering milestone loop");
            // Reset out state (drain from seed turn is irrelevant) and fall through
            // to the milestone loop below.
            out = BgOutcome::default();
        } else {
            external_obs.emit_external("empty", "seed failed — empty workgraph");
            out.mission_state = MissionState::EmptyGraph;
            return Ok(out);
        }
    }
    out.mission_state = MissionState::Running;
    // #2 cross-run reset: any `in_progress` milestone left from a previous run
    // must revert to `pending` so the loop can advance it. An `in_progress` node
    // that was never completed is stale — no agent is actively working on it.
    {
        let _ = crate::workgraph::WorkGraph::with_lock(&root, |g| {
            for n in &mut g.nodes {
                if n.status == crate::workgraph::NodeStatus::InProgress {
                    n.status = crate::workgraph::NodeStatus::Pending;
                }
            }
            Ok(())
        });
    }
    let mut advanced = 0usize;
    loop {
        if advanced >= max_auto {
            out.mission_state = MissionState::Completed;
            break;
        }
        match advance_one_milestone(
            provider.clone(), model.clone(), max_tokens, temperature, root.clone(),
        ) {
            Ok(Some(step)) => {
                // 累积输出。
                out.final_text.push_str(&step.final_text);
                out.tool_calls.extend(step.tool_calls);
                out.denied.extend(step.denied);
                out.events.extend(step.events);
                out.subgoals.extend(step.subgoals);
                advanced += 1;
            }
            Ok(None) => {
                // 无可就绪里程碑 → 全部完成。
                out.mission_state = MissionState::Completed;
                break;
            }
            Err(e) => {
                out.mission_state = MissionState::Error(e.to_string());
                break;
            }
        }
    }
    // observability: final mission state (external_obs — this is the last write).
    external_obs.emit_external("mission_state", &format!("{:?}", out.mission_state));
    Ok(out)
}

/// Extract a human-readable message from a joined thread's panic payload
/// (`std::thread::Result` Err), so a turn panic becomes a recorded error
/// instead of aborting the whole headless process.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Drain events from a background turn's rx into the BgOutcome accumulator.
/// BgObserver events are now handled by AgentLoop's ObserverSet, so this
/// function only accumulates BgOutcome state (no manual obs.emit calls).
fn drain_bg_events(
    rx: std::sync::mpsc::Receiver<AgentEvent>,
    out: &mut BgOutcome,
    mut trace: Option<&mut crate::trace::TraceEmitter>,
) {
    for ev in rx.into_iter() {
        // Forward to trace emitter BEFORE pattern-matching (which may move fields)
        if let Some(ref mut t) = trace {
            t.on_agent_event(&ev);
        }
        match ev {
            AgentEvent::StreamDelta(s) => out.final_text.push_str(&s),
            AgentEvent::ToolStarted { name, .. } => {
                out.tool_calls.push(name.clone());
                out.events.push(format!("tool: {name}"));
            }
            AgentEvent::ToolFinished { name, is_error, output } => {
                if is_error {
                    out.denied.push(format!("{name}: {output}"));
                }
            }
            AgentEvent::Notice(m) => {
                out.events.push(format!("notice: {m}"));
            }
            AgentEvent::Context { pct } => out.events.push(format!("context: {pct}%")),
            AgentEvent::SubAgentMilestone(m) => out.events.push(format!("sub-agent: {m}")),
            _ => {}
        }
    }
}

/// 对里程碑 #N 执行 Plan Turn：加载 engineer skill，生成计划，写入 .codecoder/milestone-plans/。
/// 返回创建的计划对象。如果计划已存在，直接返回（支持中断恢复）。
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

/// 推进 workgraph 的下一个就绪(pending)里程碑：若无 plan 先执行 Plan Turn 生成计划，
/// 再执行 Exec Turn（编码 → 自验收 → 标记 Done）。两阶段在同一函数内同步完成，
/// 调用方无需感知 Plan/Exec 的区分。无就绪里程碑时返回 `Ok(None)`。
/// daemon 与 background runner 共用此函数。
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

    // ── Phase 1: Plan Turn（若尚无 plan）──
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
        agent // hand the agent back so we can read last_error()
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

/// 在 `memory/` 中追加一条 checkpoint 记录:已完成里程碑 + 触及文件。
fn update_bg_checkpoint(root: &Path, milestone_id: u64, title: &str, touched: &[String]) -> std::io::Result<()> {
    let files = if touched.is_empty() {
        String::new()
    } else {
        format!("\n  触及文件:\n    {}\n", touched.join("\n    "))
    };
    let line = format!("#{milestone_id} {title} ✅{files}\n");
    let existing = crate::memory::get(root, "bg_checkpoint").unwrap_or_default();
    crate::memory::set(root, "bg_checkpoint", &(existing + &line))
}

/// 读取 BG checkpoint 文本，用于注入 milestone prompt 提供项目上下文。
fn read_bg_checkpoint(root: &Path) -> String {
    match crate::memory::get(root, "bg_checkpoint") {
        Some(cp) => format!(
            "\n--- 项目上下文 (上次 BG 运行 checkpoint) ---\n\
             以下里程碑已在之前完成:\n{cp}\n\
             不要重新完成它们。请基于已有项目结构继续推进。\n",
        ),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::stub::StubClient;
    use crate::workgraph::WorkGraph;
    use std::sync::Arc;

    fn root_with_one_milestone() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cc_bg_advance_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = WorkGraph::default();
        g.add("do thing", vec![]).unwrap();
        g.save(&dir).unwrap();
        dir
    }

    #[test]
    fn advance_one_milestone_returns_none_when_empty() {
        let dir = std::env::temp_dir().join(format!("cc_bg_empty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap();
        assert!(out.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_one_milestone_runs_a_turn() {
        let dir = root_with_one_milestone();
        // StubClient doesn't call use_skill or write plan files, so the Plan Turn
        // will fail. This is expected — the function now requires a real LLM for
        // the Plan phase.
        let result = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        );
        assert!(result.is_err(), "stub should fail plan turn (no plan written)");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("plan turn did not write plan"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use crate::workgraph::NodeStatus;

    /// 总是 provider 错误(503),用于 Error(4) 路径测试(ADR 0033)。
    struct FailingProvider;
    impl crate::provider::Provider for FailingProvider {
        fn name(&self) -> &str { "failing" }
        fn complete(
            &self,
            _req: &crate::provider::CompletionRequest,
        ) -> anyhow::Result<crate::provider::Completion> {
            Err(anyhow::anyhow!("provider down: simulated 503"))
        }
    }

    /// Panics inside the turn (not a graceful `Err`), exercising the worker-thread
    /// `join()` panic path — which must be mapped to a recorded error instead of
    /// aborting the headless process.
    struct PanicProvider;
    impl crate::provider::Provider for PanicProvider {
        fn name(&self) -> &str { "panic" }
        fn complete(
            &self,
            _req: &crate::provider::CompletionRequest,
        ) -> anyhow::Result<crate::provider::Completion> {
            panic!("simulated turn panic");
        }
    }

    #[test]
    fn explicit_task_turn_panic_maps_to_error_state() {
        let dir = std::env::temp_dir().join(format!("cc_panic1_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A panicking turn must NOT abort the process; run_background_cfg returns
        // Ok(out) with mission_state = Error(... panicked ...).
        let out = run_background_cfg(
            Arc::new(PanicProvider), "m".into(), 256, 0.0, dir.clone(),
            "do something".into(), 3, 8,
        )
        .expect("must return Ok, not abort");
        match out.mission_state {
            MissionState::Error(ref m) => assert!(m.contains("panicked"), "got {m}"),
            other => panic!("expected Error state, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_one_milestone_turn_panic_returns_err() {
        let dir = std::env::temp_dir().join(format!("cc_panic2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir); // one ready milestone
        // A panicking milestone turn must surface as Err, not a process abort.
        let r = advance_one_milestone(
            Arc::new(PanicProvider), "m".into(), 256, 0.0, dir.clone(),
        );
        let e = r.expect_err("panic must map to Err");
        assert!(e.to_string().contains("panicked"), "got {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_task_provider_error_yields_error_state() {
        let dir = std::env::temp_dir().join(format!("cc_experr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = run_background_cfg(
            Arc::new(FailingProvider),
            "m".into(),
            256,
            0.0,
            dir.clone(),
            "do something".into(),
            3,
            8,
        )
        .unwrap();
        assert!(
            matches!(out.mission_state, MissionState::Error(_)),
            "got {:?}",
            out.mission_state
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workgraph_provider_error_yields_error_state() {
        let dir = std::env::temp_dir().join(format!("cc_wgerr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir); // 有就绪里程碑,但 provider 会错
        let out = run_background_cfg(
            Arc::new(FailingProvider),
            "m".into(),
            256,
            0.0,
            dir.clone(),
            "".into(), // 空 task → workgraph 分支
            3,
            8,
        )
        .unwrap();
        assert!(
            matches!(out.mission_state, MissionState::Error(_)),
            "got {:?}",
            out.mission_state
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ws(dir: &std::path::Path) {
        let mut g = WorkGraph::default();
        g.add("t1", vec![]).unwrap();
        let _ = g.save(dir);
    }

    #[test]
    fn advance_marks_done() {
        let dir = std::env::temp_dir().join(format!("cc_t1_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir);
        // Pre-write a plan so the Exec Turn runs (StubClient can't write one).
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "t1".into(),
            skill_used: "engineer-coach".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
            acceptance_criteria: vec!["complete the task".into()],
            scope: crate::milestone_plan::MilestoneScope {
                files_to_create: vec![],
                files_to_modify: vec![],
                estimated_lines: 10,
            },
            risks: vec![],
            test_requirements: "none".into(),
        };
        crate::milestone_plan::write_plan(&dir, &plan).unwrap();
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap().unwrap();
        assert_eq!(WorkGraph::read(&dir).get(1).unwrap().status, NodeStatus::Done);
        assert_eq!(out.subgoals[0].milestone_id, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_with_preexisting_plan_does_exec_turn() {
        let dir = std::env::temp_dir().join(format!("cc_plan_exec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir);
        // Pre-write a plan so the Exec Turn runs.
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "t1".into(),
            skill_used: "engineer-coach".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
            acceptance_criteria: vec!["complete the task".into()],
            scope: crate::milestone_plan::MilestoneScope {
                files_to_create: vec![],
                files_to_modify: vec![],
                estimated_lines: 10,
            },
            risks: vec![],
            test_requirements: "none".into(),
        };
        crate::milestone_plan::write_plan(&dir, &plan).unwrap();
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap().unwrap();
        // Should have Exec Turn events (not plan events).
        assert!(out.events.iter().any(|e| e.starts_with("exec:")), "should run exec turn: {:?}", out.events);
        assert!(out.events.iter().any(|e| e.contains("completed")), "should mark complete: {:?}", out.events);
        assert_eq!(WorkGraph::read(&dir).get(1).unwrap().status, NodeStatus::Done);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bg_outcome_types_are_serializable() {
        let sg = SubgoalOutcome {
            milestone_id: 1,
            tool_cap_hit: true,
            touched_files: vec!["a.rs".into()],
        };
        let j = serde_json::to_string(&sg).unwrap();
        assert!(j.contains("a.rs"), "{j}");
        let back: SubgoalOutcome = serde_json::from_str(&j).unwrap();
        assert_eq!(back.milestone_id, 1);
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
    }

    #[test]
    fn workgraph_empty_graph_yields_empty_state() {
        let dir = std::env::temp_dir().join(format!("cc_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Workgraph mode, no workgraph.json → genuinely empty.
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 256, 0.0, dir.clone(),
            String::new(), 3, 8,
        )
        .unwrap();
        assert_eq!(out.mission_state, MissionState::EmptyGraph);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workgraph_self_seeds_and_advances() {
        // 集成测试：空 workgraph + AGENTS.md → seed → 推进
        // 用 StubClient（不调用工具），seed 失败 → EmptyGraph，不走后续循环
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Build a test project").unwrap();
        // 没有 workgraph.json → 空图
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 256, 0.0, dir.path().to_path_buf(),
            String::new(), 3, 8,
        ).unwrap();
        // Stub 不生成里程碑 → EmptyGraph
        assert_eq!(out.mission_state, MissionState::EmptyGraph, "{:?}", out.mission_state);
    }

    #[test]
    fn workgraph_advances_to_completed() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = WorkGraph::default();
        g.add("t1", vec![]).unwrap();
        let _ = g.save(dir.path());
        // Pre-write a plan so the Exec Turn runs (StubClient can't write one).
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "t1".into(),
            skill_used: "engineer-coach".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
            acceptance_criteria: vec!["complete the task".into()],
            scope: crate::milestone_plan::MilestoneScope {
                files_to_create: vec![],
                files_to_modify: vec![],
                estimated_lines: 10,
            },
            risks: vec![],
            test_requirements: "none".into(),
        };
        crate::milestone_plan::write_plan(dir.path(), &plan).unwrap();
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 4096, 0.7, dir.path().to_path_buf(),
            String::new(), 3, 8,
        ).unwrap();
        assert_eq!(out.mission_state, MissionState::Completed, "{:?}", out.mission_state);
        assert_eq!(WorkGraph::read(dir.path()).get(1).unwrap().status, NodeStatus::Done);
    }

    #[test]
    fn workgraph_corrupt_file_aborts_and_backs_up() {
        let dir = std::env::temp_dir().join(format!("cc_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workgraph.json"), "{ not json").unwrap();
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 256, 0.0, dir.clone(),
            String::new(), 3, 8,
        )
        .unwrap();
        assert!(matches!(out.mission_state, MissionState::Error(_)), "got {:?}", out.mission_state);
        // Original must be preserved (backed up), NOT overwritten with an empty graph.
        assert!(!dir.join("workgraph.json").exists(), "corrupt file must be renamed away");
        let backup = dir.join(format!("workgraph.json.corrupt.{}", std::process::id()));
        assert!(backup.exists(), "backup must exist at {}", backup.display());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_mission_returns_agents_md_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Build a Rust CLI tool").unwrap();
        let m = read_mission(dir.path());
        assert_eq!(m, "Build a Rust CLI tool");
    }

    #[test]
    fn read_mission_fallback_when_no_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let m = read_mission(dir.path());
        assert!(m.contains("Initialize and develop"));
    }

    #[test]
    fn read_mission_fallback_when_agents_md_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "").unwrap();
        let m = read_mission(dir.path());
        assert!(m.contains("Initialize and develop"));
        // 纯空格也算空
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(dir2.path().join("AGENTS.md"), "   \n\n").unwrap();
        let m2 = read_mission(dir2.path());
        assert!(m2.contains("Initialize and develop"));
    }

    #[test]
    fn seed_workgraph_from_mission_yields_milestones_with_stub() {
        // StubClient 不调用 generate_milestones → 返回 false
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Build a CLI tool").unwrap();
        let ok = seed_workgraph_from_mission(
            Arc::new(StubClient), "m".into(), 4096, 0.0, dir.path().to_path_buf(), 8,
        );
        // Stub 不调用工具，workgraph 应为空
        assert!(!ok, "stub should not produce milestones");
        let g = WorkGraph::read(dir.path());
        assert!(g.nodes.is_empty(), "stub should not write any nodes");
    }

    #[test]
    fn seed_workgraph_from_mission_no_agents_md_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // 无 AGENTS.md → 走降级路径，不 panic
        let ok = seed_workgraph_from_mission(
            Arc::new(StubClient), "m".into(), 4096, 0.0, dir.path().to_path_buf(), 8,
        );
        // 降级后仍不调用工具 → false
        assert!(!ok);
    }

    #[test]
    fn seed_workgraph_panicking_turn_returns_false() {
        struct PanicOnComplete;
        impl crate::provider::Provider for PanicOnComplete {
            fn name(&self) -> &str { "panic_seed" }
            fn complete(&self, _: &crate::provider::CompletionRequest) -> anyhow::Result<crate::provider::Completion> {
                panic!("seed provider panic");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "test").unwrap();
        let ok = seed_workgraph_from_mission(
            Arc::new(PanicOnComplete), "m".into(), 256, 0.0, dir.path().to_path_buf(), 8,
        );
        assert!(!ok, "panic should return false");
    }
}