// Background Agent runner (ADR 0026): drives one delegated task headless (no TUI,
// no user present), then returns a structured outcome. Scheduling is external.
use crate::agent::{AgentEvent, AgentLoop};
use crate::bg_gate::MissionState;
use crate::provider::Provider;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::channel;

/// 单个 milestone 的客观验收结论(供 BgOutcome.subgoals)。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SubgoalVerdict {
    Pass,
    NeedsFix,
    Inconclusive,
}

/// 一次 BG 调用中某个 milestone 的验收结果记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubgoalOutcome {
    pub milestone_id: u64,
    pub verdict: SubgoalVerdict,
    pub gate_reason: String,
    pub tool_cap_hit: bool,
    pub touched_files: Vec<String>,
    /// 本里程碑实际走的验收门类型(迭代 3 可观测)。旧账本记录缺此字段 → 默认 None。
    #[serde(default)]
    pub gate_kind: crate::bg_gate::GateKind,
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
    /// 每个 milestone 的客观验收结论(空 = 非 milestone 模式)。
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
fn seed_workgraph_from_mission(
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
         - title（简短、可行动的标题）\n\
         - acceptance（具体、可验证的验收标准，尽量包含可执行的命令如 cargo test）\n\n\
         里程碑应按依赖顺序排列，前面的里程碑是后面里程碑的前提。",
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

/// Resolve the task for a background run: an explicit non-empty `task` wins;
/// otherwise the workgraph's next ready milestone is used. Returns the chosen
/// task text and a human-readable label for event logging.
fn resolve_bg_task(task: &str, root: &std::path::Path) -> (String, String) {
    if !task.trim().is_empty() {
        return (task.to_string(), "explicit task".into());
    }
    // Empty task → check workgraph for a ready milestone (Plan #2).
    let g = crate::workgraph::WorkGraph::read(root);
    if let Some(n) = g.next_ready() {
        let label = format!("workgraph milestone #{}: {}", n.id, n.title);
        let ct = format!(
            "workgraph milestone #{}: {}\nacceptance: {}\n\n\
             Complete this milestone, then review your changes and report the \
             verdict (pass / needs_fix / rebuild).",
            n.id,
            n.title,
            if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
        );
        return (ct, label);
    }
    (String::new(), "no task (workgraph empty)".into())
}

/// Run one task to completion on the CURRENT thread, then drain events into a
/// BgOutcome. Same-thread + post-turn drain keeps it deterministic (no interleave).
/// When `task` is empty, falls back to the workgraph's next ready milestone and
/// auto-advances through up to 3 milestones.
pub fn run_background(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    task: String,
) -> anyhow::Result<BgOutcome> {
    let cfg = crate::config::Config::from_env();
    run_background_cfg(
        provider, model, max_tokens, temperature, root, task,
        cfg.bg_max_auto, cfg.bg_circuit_k, cfg.bg_milestone_tool_cap, cfg.bg_max_fix_attempts,
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
    circuit_k: usize,
    tool_cap: usize,
    max_fix_attempts: usize,
) -> anyhow::Result<BgOutcome> {
    let mut out = BgOutcome::default();

    // ── 显式任务分支:跑一 turn,不进验收门、不自动推进。──
    if !task.trim().is_empty() {
        out.events.push("task: explicit task".into());
        let mut obs = crate::bg_observer::BgObserver::start_run(&root);
        let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root);
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
        drain_bg_events(rx, &mut out, &mut obs);
        match handle.join() {
            Ok(agent) => {
                if let Some(e) = agent.last_error() {
                    out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
                }
            }
            Err(panic) => {
                let msg = format!("bg turn thread panicked: {}", panic_message(panic));
                obs.emit("error", &msg);
                out.mission_state = crate::bg_gate::MissionState::Error(msg);
            }
        }
        return Ok(out);
    }

    // ── Workgraph 分支:客观门驱动的 milestone 循环(spec 2026-07-22)。──
    // Reset the NDJSON event stream once at run start; per-milestone observers append.
    let mut obs = crate::bg_observer::BgObserver::start_run(&root);
    // #3 data-loss guard: a present-but-unreadable workgraph.json must never be
    // silently treated as empty and overwritten — back it up and abort.
    let graph = match crate::workgraph::WorkGraph::read_checked(&root) {
        Ok(g) => g,
        Err(e) => {
            let bad = root.join("workgraph.json");
            let backup = root.join(format!("workgraph.json.corrupt.{}", std::process::id()));
            let _ = std::fs::rename(&bad, &backup);
            let msg = format!("workgraph.json unreadable ({e}); backed up to {}", backup.display());
            obs.emit("error", &msg);
            out.mission_state = crate::bg_gate::MissionState::Error(msg);
            return Ok(out);
        }
    };
    // #1 empty graph: try to auto-seed from AGENTS.md; fall back to EmptyGraph on failure.
    if graph.nodes.is_empty() {
        obs.emit("seed", "empty workgraph — attempting to seed from AGENTS.md...");
        let seeded = seed_workgraph_from_mission(
            provider.clone(), model.clone(), max_tokens, temperature, root.clone(), tool_cap,
        );
        if seeded {
            obs.emit("seed", "workgraph seeded successfully — entering milestone loop");
            // Reset out state (drain from seed turn is irrelevant) and fall through
            // to the milestone loop below.
            out = BgOutcome::default();
            // Continue past this block into the loop
        } else {
            obs.emit("empty", "seed failed — empty workgraph");
            out.mission_state = crate::bg_gate::MissionState::EmptyGraph;
            return Ok(out);
        }
    }
    out.mission_state = crate::bg_gate::MissionState::Running;
    let mut consecutive_fail = 0usize;
    let mut advanced = 0usize;
    loop {
        if advanced >= max_auto {
            out.mission_state = crate::bg_gate::MissionState::CompletedAllReady;
            break;
        }
        // 选取:优先就绪(pending)里程碑;无就绪则尝试自恢复一个 needs_fix。
        let ready_id = { crate::workgraph::WorkGraph::read(&root).next_ready().map(|n| n.id) };
        let (step, from_retry) = if ready_id.is_some() {
            match advance_one_milestone(
                provider.clone(), model.clone(), max_tokens, temperature, root.clone(),
            ) {
                Ok(Some(s)) => (s, false),
                Ok(None) => break, // race-safe:重读后已无就绪
                Err(e) => {
                    out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
                    break;
                }
            }
        } else {
            match retry_one_milestone(
                provider.clone(), model.clone(), max_tokens, temperature, root.clone(), max_fix_attempts,
            ) {
                Ok(Some(s)) => (s, true),
                Ok(None) => {
                    // 既无就绪、也无可重试 needs_fix → 终态。仅在仍 Running 时置态,
                    // 区分"真完成"与"卡在 needs_fix(预算耗尽)"。
                    if out.mission_state == crate::bg_gate::MissionState::Running {
                        let g = crate::workgraph::WorkGraph::read(&root);
                        let needs_fix = g
                            .nodes
                            .iter()
                            .find(|n| n.status == crate::workgraph::NodeStatus::NeedsFix);
                        out.mission_state = match needs_fix {
                            Some(n) => crate::bg_gate::MissionState::StuckNeedsFix(n.id),
                            None => crate::bg_gate::MissionState::CompletedAllReady,
                        };
                    }
                    break;
                }
                Err(e) => {
                    out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
                    break;
                }
            }
        };

        // 累积输出(先取 last 再 extend,避免所有权移动)。
        let last = step.subgoals.last().cloned();
        out.final_text.push_str(&step.final_text);
        out.tool_calls.extend(step.tool_calls);
        out.denied.extend(step.denied);
        out.events.extend(step.events);
        out.subgoals.extend(step.subgoals);
        let Some(sg) = last else { break; };

        if from_retry {
            // 重试的成败由 fix_attempts 预算约束,不计入 max_auto / consecutive_fail /
            // next_action;下一轮 selection 会再重试(若仍有预算)或落 StuckNeedsFix。
            continue;
        }
        advanced += 1;
        let passed = matches!(sg.verdict, SubgoalVerdict::Pass);
        if !passed {
            // 该 milestone 仍有重试预算 → 交给下一轮 selection 自恢复,不计 cf、不走 next_action。
            let has_budget = {
                let g = crate::workgraph::WorkGraph::read(&root);
                g.get(sg.milestone_id).map(|n| n.fix_attempts < max_fix_attempts).unwrap_or(false)
            };
            if has_budget {
                continue;
            }
        }
        // pass,或失败且预算耗尽(硬失败)→ 沿用既有 next_action 语义(BlockedAt/CircuitBreaker/…)。
        if passed {
            consecutive_fail = 0;
        } else {
            consecutive_fail += 1;
        }
        let gv = if passed {
            crate::bg_gate::GateVerdict::Pass
        } else if matches!(sg.verdict, SubgoalVerdict::Inconclusive) {
            crate::bg_gate::GateVerdict::Inconclusive(sg.gate_reason.clone())
        } else {
            crate::bg_gate::GateVerdict::NeedsFix(sg.gate_reason.clone())
        };
        let g = crate::workgraph::WorkGraph::read(&root);
        // 这里的"预算"指 max_auto 推进预算(本次 run 还能推进多少个里程碑),
        // 与前面按节点检查的 fix_attempts 重试预算是两回事。
        let budget_left = advanced < max_auto;
        match crate::bg_gate::next_action(
            &g, sg.milestone_id, &gv, consecutive_fail, budget_left, circuit_k,
        ) {
            crate::bg_gate::NextAction::Advance(_) => continue,
            crate::bg_gate::NextAction::Stop(st) => {
                out.mission_state = st;
                break;
            }
        }
    }
    // observability: final mission state (fresh observer — this is the last write).
    crate::bg_observer::BgObserver::new(&root)
        .emit("mission_state", &format!("{:?}", out.mission_state));
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

/// Drain events from a background turn's rx into the BgOutcome accumulator,
/// teeing each to the observer for live stderr + NDJSON output.
fn drain_bg_events(
    rx: std::sync::mpsc::Receiver<AgentEvent>,
    out: &mut BgOutcome,
    obs: &mut crate::bg_observer::BgObserver,
) {
    for ev in rx.into_iter() {
        match ev {
            AgentEvent::StreamDelta(s) => out.final_text.push_str(&s),
            AgentEvent::ToolStarted { name, .. } => {
                obs.emit("tool_started", &name);
                out.tool_calls.push(name.clone());
                out.events.push(format!("tool: {name}"));
            }
            AgentEvent::ToolFinished { name, is_error, output } => {
                if is_error {
                    obs.emit("tool_error", &format!("{name}: {output}"));
                    out.denied.push(format!("{name}: {output}"));
                } else {
                    obs.emit("tool_finished", &name);
                }
            }
            AgentEvent::Notice(m) => {
                obs.emit("notice", &m);
                out.events.push(format!("notice: {m}"));
            }
            AgentEvent::Context { pct } => out.events.push(format!("context: {pct}%")),
            AgentEvent::SubAgentMilestone(m) => out.events.push(format!("sub-agent: {m}")),
            AgentEvent::TokenUsage { prompt_tokens, completion_tokens } => {
                obs.emit_with_data("llm_call", "tokens", Some(serde_json::json!({
                    "prompt_tokens": prompt_tokens,
                    "completion_tokens": completion_tokens,
                })));
            }
            _ => {}
        }
    }
}

/// 构造 needs_fix 重试的修复 prompt:注入上一轮失败原因 + acceptance,要求先针对
/// 失败做实际改动,再自评,并以内核可解析的 VERDICT 行结尾。纯函数,便于单测。
pub(crate) fn build_repair_prompt(m: &crate::workgraph::Milestone, last_failure: &str) -> String {
    format!(
        "workgraph milestone #{} ({}) 上一轮验收未通过,需要修复后重试。\n\
         上一轮失败原因:\n{}\n\n\
         acceptance: {}\n\n\
         请针对上述失败原因做出实际代码改动来修复它(不要只解释),然后自评。\
         你必须以下面这行精确格式结尾(其后不要有任何内容),以便内核解析并自动更新\
         里程碑状态:\n\
         VERDICT: <pass|needs_fix|rebuild>",
        m.id,
        m.title,
        last_failure.trim(),
        if m.acceptance.is_empty() { "(none)" } else { &m.acceptance },
    )
}

/// 推进 workgraph 的下一个就绪(pending)里程碑：跑一个 turn、客观门、写回状态。
/// 无就绪里程碑时返回 `Ok(None)`。daemon 与 background runner 共用此函数。
pub fn advance_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::WorkGraph;
    let (milestone_id, task_text, title) = {
        let g = WorkGraph::read_checked(&root)?;
        let Some(n) = g.next_ready() else { return Ok(None); };
        let t = format!(
            "workgraph milestone #{}: {}\nacceptance: {}\n\n\
             Complete this milestone, then self-review. You MUST end your reply \
             with a final line in EXACTLY this format (nothing after it) so the \
             kernel can parse and auto-update the milestone status:\n\
             VERDICT: <pass|needs_fix|rebuild>",
            n.id, n.title,
            if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
        );
        (n.id, t, n.title.clone())
    };
    run_milestone_and_gate(provider, model, max_tokens, temperature, root, milestone_id, task_text, title)
        .map(Some)
}

/// 自恢复一个 needs_fix 里程碑(ADR 0026 迭代 1)。选 `next_retryable`,**先**递增其
/// `fix_attempts`(即便 turn 崩溃预算也被尊重),再注入上一轮失败原因构造修复 prompt,
/// 跑一 turn + 客观门。无可重试项(无 needs_fix 或全部耗尽预算)时返回 `Ok(None)`。
pub fn retry_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    max_fix_attempts: usize,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::WorkGraph;
    let (milestone_id, prompt, title) = {
        let g = WorkGraph::read_checked(&root)?;
        let Some(n) = g.next_retryable(max_fix_attempts) else { return Ok(None); };
        let last = n
            .last_failure
            .clone()
            .unwrap_or_else(|| "(无记录的失败原因)".to_string());
        (n.id, build_repair_prompt(n, &last), n.title.clone())
    };
    // 先记账再跑:即便本次 turn 崩溃,预算也已消耗,避免无限重试。
    if let Err(e) = WorkGraph::with_lock(&root, |g| {
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.fix_attempts += 1;
        }
        Ok(())
    }) {
        eprintln!("ccd: fix_attempts bump failed for #{milestone_id}: {e}");
    }
    run_milestone_and_gate(provider, model, max_tokens, temperature, root, milestone_id, prompt, title)
        .map(Some)
}

/// 跑一个已选定 milestone 的 turn + 客观验收门 + 写回状态。被 `advance_one_milestone`
/// (pending 常规推进)与 `retry_one_milestone`(needs_fix 自恢复)共用。
fn run_milestone_and_gate(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    milestone_id: u64,
    task_text: String,
    title: String,
) -> anyhow::Result<BgOutcome> {
    use crate::workgraph::{NodeStatus, WorkGraph};
    // Clone provider/model for the independent review sub-agent BEFORE the
    // milestone agent consumes them (ADR 0039).
    let review_provider = provider.clone();
    let review_model = model.clone();
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    if let Err(e) = agent.cancel_token().cancel_on_sigint() {
        eprintln!("ccd: SIGINT cancel not wired: {e}");
    }
    let cfg = crate::config::Config::from_env();
    agent.set_tool_cap(cfg.bg_milestone_tool_cap);
    let cancel = agent.cancel_token();
    let mut out = BgOutcome::default();
    out.events.push(format!("task: workgraph milestone #{} ({})", milestone_id, title));
    let mut obs = crate::bg_observer::BgObserver::new(&root);
    obs.emit("milestone_start", &format!("#{milestone_id} {title}"));
    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(task_text, &tx);
        drop(tx);
        agent // hand the agent back so we can read last_error()
    });
    drain_bg_events(rx, &mut out, &mut obs);
    let agent = match handle.join() {
        Ok(agent) => agent,
        Err(panic) => {
            let msg = format!("bg turn thread panicked: {}", panic_message(panic));
            obs.emit("error", &msg);
            return Err(anyhow::anyhow!(msg));
        }
    };
    if let Some(e) = agent.last_error() {
        return Err(anyhow::anyhow!(e.to_string()));
    }

    // ── 客观验收门(覆盖 agent 自报 VERDICT)──
    let m = {
        let g = WorkGraph::read(&root);
        g.get(milestone_id).expect("just read").clone()
    };
    let tool_cap_hit = out.events.iter().any(|e| e.contains("tool-iteration cap"));
    let acceptance = m.acceptance.clone();
    let review_root = root.clone();
    let self_report = out.final_text.clone();
    let cancel_for_review = cancel.clone();
    let review_runner = || -> crate::bg_gate::GateVerdict {
        // On cancel, don't spend a review call — fall back to self-report parse.
        if cancel_for_review.is_cancelled() {
            let o = crate::review::parse_review(&self_report);
            return if !o.unparsed && matches!(o.verdict, crate::review::Verdict::Pass) {
                crate::bg_gate::GateVerdict::Pass
            } else if !o.unparsed {
                crate::bg_gate::GateVerdict::NeedsFix(format!("self-review: {:?}", o.verdict))
            } else {
                crate::bg_gate::GateVerdict::Inconclusive("review skipped (cancelled)".into())
            };
        }
        // Independent read-only review overrides agent self-report.
        let mut rev = AgentLoop::new_background(
            review_provider.clone(), review_model.clone(), max_tokens, temperature, review_root.clone(),
        );
        let (rtx, _rrx) = channel::<AgentEvent>();
        let target = format!("workgraph milestone acceptance: {acceptance}");
        let (outcome, _raw) = rev.run_review(&target, &rtx);
        match outcome.verdict {
            crate::review::Verdict::Pass => crate::bg_gate::GateVerdict::Pass,
            v => crate::bg_gate::GateVerdict::NeedsFix(format!("independent review: {v:?}")),
        }
    };
    let verdict = crate::bg_gate::evaluate(&m, &root, Some(&cancel), &review_runner);

    let (sv, status, vs_str) = match &verdict {
        crate::bg_gate::GateVerdict::Pass => (SubgoalVerdict::Pass, NodeStatus::Done, "pass"),
        crate::bg_gate::GateVerdict::NeedsFix(_) => (SubgoalVerdict::NeedsFix, NodeStatus::NeedsFix, "needs_fix"),
        crate::bg_gate::GateVerdict::Inconclusive(_) => (SubgoalVerdict::Inconclusive, NodeStatus::NeedsFix, "inconclusive"),
    };
    let gate_reason = match &verdict {
        crate::bg_gate::GateVerdict::Pass => "gate pass".to_string(),
        crate::bg_gate::GateVerdict::NeedsFix(r) | crate::bg_gate::GateVerdict::Inconclusive(r) => r.clone(),
    };
    obs.emit("gate", &format!("#{milestone_id} {vs_str}: {gate_reason}"));
    {
        let reason_for_persist = gate_reason.clone();
        let _ = WorkGraph::with_lock(&root, |g| {
            g.set_status(milestone_id, status);
            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                n.verdict = Some(vs_str.into());
                if matches!(status, NodeStatus::NeedsFix) {
                    n.last_failure = Some(reason_for_persist.clone());
                } else {
                    n.last_failure = None; // Pass 时清空,避免陈旧原因污染未来重试
                }
            }
            Ok(())
        });
    }
    if !matches!(verdict, crate::bg_gate::GateVerdict::Pass) {
        let _ = crate::tool::reason::record_cause(
            &root,
            &format!("milestone #{milestone_id} ({title}) 验收失败: {gate_reason}"),
            None,
        );
    }
    out.subgoals.push(SubgoalOutcome {
        milestone_id,
        verdict: sv,
        gate_reason,
        tool_cap_hit,
        touched_files: m.touched.clone(),
        gate_kind: crate::bg_gate::gate_kind(&m),
    });
    out.events.push(format!("milestone #{} ({}) gated: {vs_str}", milestone_id, title));
    Ok(out)
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
        g.add("do thing", "acceptance", vec![]).unwrap();
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
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap();
        assert!(out.is_some(), "should run a turn for the ready milestone");
        let outcome = out.unwrap();
        assert!(!outcome.final_text.is_empty(), "stub should produce some final text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 客观验收门集成测试(spec T1/T2/T4/T5)──

    use crate::bg_gate::MissionState;
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
            "do something".into(), 3, 2, 8, 0,
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
        ws(&dir, &[(1, "echo ok", vec![])]); // one ready milestone
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
            2,
            8,
            0,
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
        ws(&dir, &[(1, "echo ok", vec![])]); // 有就绪里程碑,但 provider 会错
        let out = run_background_cfg(
            Arc::new(FailingProvider),
            "m".into(),
            256,
            0.0,
            dir.clone(),
            "".into(), // 空 task → workgraph 分支
            3,
            2,
            8,
            0,
        )
        .unwrap();
        assert!(
            matches!(out.mission_state, MissionState::Error(_)),
            "got {:?}",
            out.mission_state
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ws(dir: &std::path::Path, nodes: &[(u64, &str, Vec<u64>)]) {
        let mut g = WorkGraph::default();
        for (id, acc, deps) in nodes {
            g.add(&format!("t{id}"), acc, deps.clone()).unwrap();
        }
        let _ = g.save(dir);
    }

    #[test]
    fn t1_command_gate_pass_marks_done() {
        let dir = std::env::temp_dir().join(format!("cc_t1_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // "rustc --version" 含已知模式 "rustc" 且 exit 0 → 命令门 Pass(hermetic:Rust 仓必装 rustc)。
        ws(&dir, &[(1, "rustc --version", vec![])]);
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap().unwrap();
        assert_eq!(WorkGraph::read(&dir).get(1).unwrap().status, NodeStatus::Done);
        assert_eq!(out.subgoals[0].verdict, SubgoalVerdict::Pass, "{:?}", out.subgoals);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn t2_command_gate_fail_marks_needsfix_and_causal() {
        let dir = std::env::temp_dir().join(format!("cc_t2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // "rustc --bad-flag-xyz" 含模式 "rustc" 且 exit≠0 → 命令门 NeedsFix。
        ws(&dir, &[(1, "rustc --bad-flag-xyz", vec![])]);
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap().unwrap();
        assert_eq!(WorkGraph::read(&dir).get(1).unwrap().status, NodeStatus::NeedsFix);
        assert_eq!(out.subgoals[0].verdict, SubgoalVerdict::NeedsFix);
        assert!(out.subgoals[0].gate_reason.contains("rustc"), "{:?}", out.subgoals[0].gate_reason);
        let causal = std::fs::read_to_string(dir.join("causal_tree.json")).unwrap_or_default();
        assert!(causal.contains("验收失败"), "causal node not written: {causal}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gate_failure_persists_last_failure_reason() {
        let dir = std::env::temp_dir().join(format!("cc_lastfail_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // prose acceptance(无命令模式)+ StubClient 无 VERDICT → 评审门 Inconclusive → NeedsFix。
        ws(&dir, &[(1, "渲染输出正确", vec![])]);
        let _ = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap().unwrap();
        let n = WorkGraph::read(&dir).get(1).unwrap().clone();
        assert_eq!(n.status, NodeStatus::NeedsFix);
        assert!(n.last_failure.is_some(), "needs_fix 应记录 last_failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn t4_blocked_at_when_dependent_blocked() {
        let dir = std::env::temp_dir().join(format!("cc_t4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "false", vec![]), (2, "echo ok", vec![1])]);
        let out = run_background_cfg(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), "".into(), 3, 2, 8, 0,
        ).unwrap();
        assert_eq!(out.mission_state, MissionState::BlockedAt(1), "{:?}", out.mission_state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn t5_circuit_breaker_on_consecutive_fails() {
        let dir = std::env::temp_dir().join(format!("cc_t5_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 两个独立 milestone,都 fail(false)→ 连续 2 fail → CircuitBreaker。
        ws(&dir, &[(1, "false", vec![]), (2, "false", vec![])]);
        let out = run_background_cfg(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), "".into(), 3, 2, 8, 0,
        ).unwrap();
        assert_eq!(out.mission_state, MissionState::CircuitBreaker, "{:?}", out.mission_state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stuck_needs_fix_when_only_needs_fix_and_nothing_ready() {
        // 回归:一个 fresh 进程发现唯一可动的 milestone 是 needs_fix(无 pending-ready),
        // 过去会走 Ok(None)→CompletedAllReady→exit 0 假报成功。现在应报 StuckNeedsFix。
        let dir = std::env::temp_dir().join(format!("cc_stuck_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = WorkGraph::default();
        let id = g.add("core", "cargo build", vec![]).unwrap();
        g.set_status(id, NodeStatus::NeedsFix);
        g.save(&dir).unwrap();
        let out = run_background_cfg(
            Arc::new(StubClient), "gpt-4o".into(), 256, 0.0, dir.clone(), "".into(), 3, 2, 8, 0,
        ).unwrap();
        assert_eq!(
            out.mission_state,
            MissionState::StuckNeedsFix(id),
            "needs_fix-only graph must not report success; got {:?}",
            out.mission_state
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bg_outcome_types_are_serializable() {
        use crate::bg_gate::MissionState;
        let sg = SubgoalOutcome {
            milestone_id: 1,
            verdict: SubgoalVerdict::NeedsFix,
            gate_reason: "gate failed".into(),
            tool_cap_hit: true,
            touched_files: vec!["a.rs".into()],
            gate_kind: crate::bg_gate::GateKind::None,
        };
        let j = serde_json::to_string(&sg).unwrap();
        assert!(j.contains("NeedsFix") && j.contains("a.rs"), "{j}");
        let back: SubgoalOutcome = serde_json::from_str(&j).unwrap();
        assert_eq!(back.milestone_id, 1);
        for s in [
            MissionState::Running,
            MissionState::CompletedAllReady,
            MissionState::BlockedAt(7),
            MissionState::CircuitBreaker,
            MissionState::Error("boom".into()),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            let back: MissionState = serde_json::from_str(&j).unwrap();
            assert_eq!(format!("{back:?}"), format!("{s:?}"));
        }
    }

    /// 有状态的测试 Provider:前 `fail_until` 次 `complete` 返回 `VERDICT: needs_fix`,
    /// 其后返回 `VERDICT: pass`。供 retry_one_milestone 测试与 Task 7 共用。
    struct FlakyProvider {
        fail_until: usize,
        calls: std::sync::Mutex<usize>,
    }
    impl crate::provider::Provider for FlakyProvider {
        fn name(&self) -> &str { "flaky" }
        fn complete(
            &self,
            _req: &crate::provider::CompletionRequest,
        ) -> anyhow::Result<crate::provider::Completion> {
            use crate::message::{Message, MessageItem, Role};
            let mut c = self.calls.lock().unwrap();
            let i = *c;
            *c += 1;
            let text = if i < self.fail_until { "VERDICT: needs_fix" } else { "VERDICT: pass" };
            Ok(Message {
                id: 0,
                role: Role::Assistant,
                items: vec![MessageItem::Text { text: text.into() }],
            }
            .into())
        }
    }

    #[test]
    fn retry_one_milestone_bumps_attempt_and_can_pass() {
        let dir = std::env::temp_dir().join(format!("cc_retry1_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 种一个 needs_fix 里程碑(prose acceptance → 走评审门,读 Provider 的 VERDICT)。
        let mut g = WorkGraph::default();
        let id = g.add("core", "渲染输出正确", vec![]).unwrap();
        g.set_status(id, NodeStatus::NeedsFix);
        g.nodes.iter_mut().find(|n| n.id == id).unwrap().last_failure = Some("上轮失败".into());
        g.save(&dir).unwrap();

        // Provider 立即 pass(fail_until=0)。
        let out = retry_one_milestone(
            Arc::new(FlakyProvider { fail_until: 0, calls: std::sync::Mutex::new(0) }),
            "m".into(), 4096, 0.0, dir.clone(), 3,
        ).unwrap();
        assert!(out.is_some(), "有可重试项应返回 Some");
        let n = WorkGraph::read(&dir).get(id).unwrap().clone();
        assert_eq!(n.status, NodeStatus::Done, "pass 后应 Done");
        assert_eq!(n.fix_attempts, 1, "重试应递增 fix_attempts");
        assert_eq!(n.last_failure, None, "pass 后应清空 last_failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retry_one_milestone_none_when_nothing_retryable() {
        let dir = std::env::temp_dir().join(format!("cc_retry0_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "渲染输出正确", vec![])]); // 默认 Pending,非 needs_fix
        let out = retry_one_milestone(
            Arc::new(StubClient), "m".into(), 4096, 0.0, dir.clone(), 3,
        ).unwrap();
        assert!(out.is_none(), "无 needs_fix 时应 None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workgraph_auto_retries_needs_fix_until_pass() {
        let dir = std::env::temp_dir().join(format!("cc_selfrec_pass_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "渲染输出正确", vec![])]); // prose → 评审门跑独立评审子 agent(ADR 0039)
        // 独立评审门(ADR 0039):每轮里程碑消耗 2 次 Provider 调用——里程碑 agent(偶数下标)
        // + 独立评审子 agent(奇数下标),门结论取评审调用。评审调用落在下标 1、3、5;
        // fail_until=5 使下标 0–4 均 needs_fix、下标 5 pass ⇒ 评审序列 needs_fix、needs_fix、pass,
        // 即 advance + retry#1 都 needs_fix、retry#2 pass ⇒ fix_attempts=2。
        let out = run_background_cfg(
            Arc::new(FlakyProvider { fail_until: 5, calls: std::sync::Mutex::new(0) }),
            "m".into(), 256, 0.0, dir.clone(), "".into(),
            5, 10, 8, 3,
        ).unwrap();
        assert_eq!(out.mission_state, MissionState::CompletedAllReady, "{:?}", out.mission_state);
        let n = WorkGraph::read(&dir).get(1).unwrap().clone();
        assert_eq!(n.status, NodeStatus::Done);
        assert_eq!(n.fix_attempts, 2, "两次重试后通过");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workgraph_gives_up_after_max_fix_attempts() {
        let dir = std::env::temp_dir().join(format!("cc_selfrec_giveup_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "渲染输出正确", vec![])]);
        // 恒 needs_fix(fail_until 极大)。
        let out = run_background_cfg(
            Arc::new(FlakyProvider { fail_until: 9999, calls: std::sync::Mutex::new(0) }),
            "m".into(), 256, 0.0, dir.clone(), "".into(),
            10, 10, 8, 2,
        ).unwrap();
        assert_eq!(out.mission_state, MissionState::StuckNeedsFix(1), "{:?}", out.mission_state);
        let n = WorkGraph::read(&dir).get(1).unwrap().clone();
        assert_eq!(n.fix_attempts, 2, "预算耗尽应等于 max_fix_attempts");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_repair_prompt_injects_failure_and_title() {
        use crate::workgraph::{Milestone, NodeStatus};
        // acceptance 刻意选一个不出现在失败字符串里的值,使两个 contains 断言互相独立。
        let m = Milestone {
            id: 7,
            title: "CRDT 核心".into(),
            acceptance: "cargo build --release".into(),
            deps: vec![],
            status: NodeStatus::NeedsFix,
            verdict: None,
            touched: vec![],
            fix_attempts: 1,
            last_failure: Some("gate `cargo test` failed: 2 failed".into()),
            command: None,
        };
        let p = build_repair_prompt(&m, "gate `cargo test` failed: 2 failed");
        assert!(p.contains("CRDT 核心"), "含标题: {p}");
        assert!(p.contains("gate `cargo test` failed: 2 failed"), "含失败原因: {p}");
        assert!(p.contains("cargo build --release"), "含 acceptance: {p}");
        assert!(p.trim_end().ends_with("VERDICT: <pass|needs_fix|rebuild>"), "以 VERDICT 行结尾: {p}");
    }

    #[test]
    fn build_repair_prompt_uses_none_for_empty_acceptance() {
        use crate::workgraph::{Milestone, NodeStatus};
        let m = Milestone {
            id: 3,
            title: "无验收命令".into(),
            acceptance: String::new(),
            deps: vec![],
            status: NodeStatus::NeedsFix,
            verdict: None,
            touched: vec![],
            fix_attempts: 0,
            last_failure: Some("self-review: NeedsFix".into()),
            command: None,
        };
        let p = build_repair_prompt(&m, "self-review: NeedsFix");
        assert!(p.contains("(none)"), "空 acceptance 应渲染为 (none): {p}");
    }

    #[test]
    fn workgraph_empty_graph_yields_empty_state() {
        let dir = std::env::temp_dir().join(format!("cc_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Workgraph mode, no workgraph.json → genuinely empty.
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 256, 0.0, dir.clone(),
            String::new(), 3, 2, 8, 0,
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
            String::new(), 3, 2, 8, 0,
        ).unwrap();
        // Stub 不生成里程碑 → EmptyGraph
        assert_eq!(out.mission_state, MissionState::EmptyGraph, "{:?}", out.mission_state);
    }

    #[test]
    fn workgraph_non_empty_still_advances_normally() {
        let dir = tempfile::tempdir().unwrap();
        ws(dir.path(), &[(1, "rustc --version", vec![])]); // 有节点
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 4096, 0.7, dir.path().to_path_buf(),
            String::new(), 3, 2, 8, 0,
        ).unwrap();
        // 应正常推进，非 EmptyGraph
        assert_ne!(out.mission_state, MissionState::EmptyGraph, "{:?}", out.mission_state);
    }

    #[test]
    fn workgraph_corrupt_file_aborts_and_backs_up() {
        let dir = std::env::temp_dir().join(format!("cc_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workgraph.json"), "{ not json").unwrap();
        let out = run_background_cfg(
            Arc::new(StubClient), "m".into(), 256, 0.0, dir.clone(),
            String::new(), 3, 2, 8, 0,
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
