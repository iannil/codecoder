// Background Agent runner (ADR 0026): drives one delegated task headless (no TUI,
// no user present), then returns a structured outcome. Scheduling is external.
use crate::agent::{AgentEvent, AgentLoop};
use crate::bg_gate::MissionState;
use crate::provider::Provider;
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
        if let Some(e) = agent.last_error() {
            out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
        }
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
        ) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // 无就绪 milestone。区分"真完成"与"卡在 needs_fix":后者是一个 fresh
                // 进程发现唯一可动的 milestone 是 needs_fix(无 pending-ready),不能
                // 假报 CompletedAllReady/exit 0,否则上层误判成功(见回归测试)。
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
                // provider 错误:置 Error(ADR 0033),不 ?逃逸成 anyhow Err→exit 1。
                out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
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
        let g = WorkGraph::read(&root);
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
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
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
    if let Some(e) = agent.last_error() {
        return Err(anyhow::anyhow!(e.to_string()));
    }

    // ── 客观验收门(覆盖 agent 自报 VERDICT)──
    let m = {
        let g = WorkGraph::read(&root);
        g.get(milestone_id).expect("just read").clone()
    };
    let tool_cap_hit = out.events.iter().any(|e| e.contains("tool-iteration cap"));
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
    let gate_reason = match &verdict {
        crate::bg_gate::GateVerdict::Pass => "gate pass".to_string(),
        crate::bg_gate::GateVerdict::NeedsFix(r) | crate::bg_gate::GateVerdict::Inconclusive(r) => r.clone(),
    };
    {
        let _ = WorkGraph::with_lock(&root, |g| {
            g.set_status(milestone_id, status);
            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                n.verdict = Some(vs_str.into());
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
            Arc::new(StubClient), "gpt-4o".into(), 256, 0.0, dir.clone(), "".into(), 3, 2, 8,
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

    #[test]
    fn build_repair_prompt_injects_failure_and_title() {
        use crate::workgraph::{Milestone, NodeStatus};
        let m = Milestone {
            id: 7,
            title: "CRDT 核心".into(),
            acceptance: "cargo test".into(),
            deps: vec![],
            status: NodeStatus::NeedsFix,
            verdict: None,
            touched: vec![],
            fix_attempts: 1,
            last_failure: Some("gate `cargo test` failed: 2 failed".into()),
        };
        let p = build_repair_prompt(&m, "gate `cargo test` failed: 2 failed");
        assert!(p.contains("CRDT 核心"), "含标题: {p}");
        assert!(p.contains("gate `cargo test` failed: 2 failed"), "含失败原因: {p}");
        assert!(p.contains("cargo test"), "含 acceptance: {p}");
        assert!(p.trim_end().ends_with("VERDICT: <pass|needs_fix|rebuild>"), "以 VERDICT 行结尾: {p}");
    }
}
