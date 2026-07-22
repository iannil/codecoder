// Background Agent runner (ADR 0026): drives one delegated task headless (no TUI,
// no user present), then returns a structured outcome. Scheduling is external.
use crate::agent::{AgentEvent, AgentLoop};
use crate::bg_gate::MissionState;
use crate::provider::Provider;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::channel;

/// 单个 milestone 的客观验收结论(供 BgOutcome.subgoals)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubgoalVerdict {
    Pass,
    NeedsFix,
    Inconclusive,
}

/// 一次 BG 调用中某个 milestone 的验收结果记录。
#[derive(Debug, Clone)]
pub struct SubgoalOutcome {
    pub milestone_id: u64,
    pub verdict: SubgoalVerdict,
    pub gate_reason: String,
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
        cfg.bg_max_auto, cfg.bg_circuit_k, cfg.bg_milestone_tool_cap,
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
) -> anyhow::Result<BgOutcome> {
    let mut out = BgOutcome::default();

    // ── 显式任务分支:跑一 turn,不进验收门、不自动推进。──
    if !task.trim().is_empty() {
        out.events.push("task: explicit task".into());
        let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root);
        // ADR 0026:wire SIGINT → cancel。
        if let Err(e) = agent.cancel_token().cancel_on_sigint() {
            eprintln!("ccd: SIGINT cancel not wired: {e}");
        }
        agent.set_tool_cap(tool_cap);
        let (tx, rx) = channel::<AgentEvent>();
        agent.run_one_turn(task, &tx);
        drop(tx);
        drain_bg_events(rx, &mut out);
        return Ok(out);
    }

    // ── Workgraph 分支:客观门驱动的 milestone 循环(spec 2026-07-22)。──
    out.mission_state = crate::bg_gate::MissionState::Running;
    let mut consecutive_fail = 0usize;
    let mut advanced = 0usize;
    loop {
        if advanced >= max_auto {
            out.mission_state = crate::bg_gate::MissionState::CompletedAllReady;
            break;
        }
        let step = match advance_one_milestone(
            provider.clone(),
            model.clone(),
            max_tokens,
            temperature,
            root.clone(),
        )? {
            Some(s) => s,
            None => {
                // 无就绪 milestone(空图或全部完成/阻塞)。
                if out.mission_state == crate::bg_gate::MissionState::Running {
                    out.mission_state = crate::bg_gate::MissionState::CompletedAllReady;
                }
                break;
            }
        };
        let last = step.subgoals.last().cloned();
        out.final_text.push_str(&step.final_text);
        out.tool_calls.extend(step.tool_calls);
        out.denied.extend(step.denied);
        out.events.extend(step.events);
        out.subgoals.extend(step.subgoals);
        advanced += 1;

        let Some(sg) = last else { break; };
        let passed = matches!(sg.verdict, SubgoalVerdict::Pass);
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
        let budget_left = advanced < max_auto;
        match crate::bg_gate::next_action(
            &g,
            sg.milestone_id,
            &gv,
            consecutive_fail,
            budget_left,
            circuit_k,
        ) {
            crate::bg_gate::NextAction::Advance(_) => continue,
            crate::bg_gate::NextAction::Stop(st) => {
                out.mission_state = st;
                break;
            }
        }
    }
    Ok(out)
}

/// Drain events from a background turn's rx into the BgOutcome accumulator.
fn drain_bg_events(rx: std::sync::mpsc::Receiver<AgentEvent>, out: &mut BgOutcome) {
    for ev in rx.into_iter() {
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
            AgentEvent::Notice(m) => out.events.push(format!("notice: {m}")),
            AgentEvent::Context { pct } => out.events.push(format!("context: {pct}%")),
            AgentEvent::SubAgentMilestone(m) => out.events.push(format!("sub-agent: {m}")),
            _ => {}
        }
    }
}

/// 推进 workgraph 的下一个就绪里程碑：跑一个 turn、解析 verdict、写回状态。
/// 无就绪里程碑时返回 `Ok(None)`。daemon 与 background runner 共用此函数。
pub fn advance_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::{NodeStatus, WorkGraph};
    let milestone_id = {
        let g = WorkGraph::read(&root);
        match g.next_ready() {
            Some(n) => n.id,
            None => return Ok(None),
        }
    };
    let (task_text, title) = {
        let g = WorkGraph::read(&root);
        let n = g.get(milestone_id).expect("just read");
        let t = format!(
            "workgraph milestone #{}: {}\nacceptance: {}\n\n\
             Complete this milestone, then self-review. You MUST end your reply \
             with a final line in EXACTLY this format (nothing after it) so the \
             kernel can parse and auto-update the milestone status:\n\
             VERDICT: <pass|needs_fix|rebuild>",
            n.id, n.title,
            if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
        );
        (t, n.title.clone())
    };
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    // Each auto-advanced milestone runs on its own agent (own cancel token), so
    // re-wire SIGINT here too — signal-hook stacks handlers, all tokens get set.
    if let Err(e) = agent.cancel_token().cancel_on_sigint() {
        eprintln!("ccd: SIGINT cancel not wired: {e}");
    }
    let cfg = crate::config::Config::from_env();
    agent.set_tool_cap(cfg.bg_milestone_tool_cap);
    let cancel = agent.cancel_token();
    let mut out = BgOutcome::default();
    out.events.push(format!("task: workgraph milestone #{} ({})", milestone_id, title));
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(task_text, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);

    // ── 客观验收门(覆盖 agent 自报 VERDICT)── spec 2026-07-22
    let m = {
        let g = WorkGraph::read(&root);
        g.get(milestone_id).expect("just read").clone()
    };
    let tool_cap_hit = out.events.iter().any(|e| e.contains("tool-iteration cap"));
    // v1 review 门:复用 agent 自产 VERDICT 文本(parse_review)作兜底;
    // 真正的 review 子代理门为后续增强(spec §5.1 (b))。
    let review_runner = || -> crate::bg_gate::GateVerdict {
        let o = crate::review::parse_review(&out.final_text);
        if !o.unparsed && matches!(o.verdict, crate::review::Verdict::Pass) {
            crate::bg_gate::GateVerdict::Pass
        } else if !o.unparsed {
            crate::bg_gate::GateVerdict::NeedsFix(format!("self-review: {:?}", o.verdict))
        } else {
            crate::bg_gate::GateVerdict::Inconclusive("no command gate; review gate deferred in v1".into())
        }
    };
    let verdict = crate::bg_gate::evaluate(&m, &root, Some(&cancel), &review_runner);

    let (sv, status, vs_str) = match &verdict {
        crate::bg_gate::GateVerdict::Pass => (SubgoalVerdict::Pass, NodeStatus::Done, "pass"),
        crate::bg_gate::GateVerdict::NeedsFix(_) => (SubgoalVerdict::NeedsFix, NodeStatus::NeedsFix, "needs_fix"),
        crate::bg_gate::GateVerdict::Inconclusive(_) => (SubgoalVerdict::Inconclusive, NodeStatus::NeedsFix, "inconclusive"),
    };
    {
        let mut g = WorkGraph::read(&root);
        g.set_status(milestone_id, status);
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.verdict = Some(vs_str.into());
        }
        let _ = g.save(&root);
    }
    let gate_reason = match &verdict {
        crate::bg_gate::GateVerdict::Pass => "gate pass".to_string(),
        crate::bg_gate::GateVerdict::NeedsFix(r) | crate::bg_gate::GateVerdict::Inconclusive(r) => r.clone(),
    };
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
    });
    out.events.push(format!("milestone #{} ({}) gated: {vs_str}", milestone_id, title));
    Ok(Some(out))
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
    fn t4_blocked_at_when_dependent_blocked() {
        let dir = std::env::temp_dir().join(format!("cc_t4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "false", vec![]), (2, "echo ok", vec![1])]);
        let out = run_background_cfg(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), "".into(), 3, 2, 8,
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
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), "".into(), 3, 2, 8,
        ).unwrap();
        assert_eq!(out.mission_state, MissionState::CircuitBreaker, "{:?}", out.mission_state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
