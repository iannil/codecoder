// Background Agent runner (ADR 0026): drives one delegated task headless (no TUI,
// no user present), then returns a structured outcome. Scheduling is external.
use crate::agent::{AgentEvent, AgentLoop};
use crate::provider::Provider;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::channel;

/// The result of one headless Background Agent turn.
#[derive(Debug, Default)]
pub struct BgOutcome {
    /// The final assistant text of the turn.
    pub final_text: String,
    /// Names of tools that actually executed (in order).
    pub tool_calls: Vec<String>,
    /// Tool outputs that reported an error (includes headless denials).
    pub denied: Vec<String>,
    /// Human-readable milestone lines.
    pub events: Vec<String>,
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
    const MAX_AUTO: usize = 3;
    let mut out = BgOutcome::default();

    // Determine initial task from explicit arg or workgraph.
    let (initial_task, label) = resolve_bg_task(&task, &root);
    if initial_task.is_empty() && !label.starts_with("workgraph milestone") {
        out.events.push(label);
        return Ok(out);
    }
    out.events.push(format!("task: {label}"));

    // Build the agent once; reuse across milestones.
    let mut agent = AgentLoop::new_background(provider.clone(), model.clone(), max_tokens, temperature, root.clone());

    // Run the first turn (explicit task or first workgraph milestone).
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(initial_task, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);

    // If no explicit task was given, auto-advance through more workgraph milestones.
    if task.trim().is_empty() {
        for _ in 0..MAX_AUTO.saturating_sub(1) {
            match advance_one_milestone(
                provider.clone(),
                model.clone(),
                max_tokens,
                temperature,
                root.clone(),
            )? {
                None => break,
                Some(step_out) => {
                    out.final_text.push_str(&step_out.final_text);
                    out.tool_calls.extend(step_out.tool_calls);
                    out.denied.extend(step_out.denied);
                    out.events.extend(step_out.events);
                }
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
    let mut out = BgOutcome::default();
    out.events.push(format!("task: workgraph milestone #{} ({})", milestone_id, title));
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(task_text, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);

    // auto-writeback：解析 verdict 更新里程碑状态
    let outcome = crate::review::parse_review(&out.final_text);
    if !outcome.unparsed {
        let mut g = WorkGraph::read(&root);
        let (status, vs) = match outcome.verdict {
            crate::review::Verdict::Pass => (NodeStatus::Done, "pass"),
            crate::review::Verdict::NeedsFix => (NodeStatus::NeedsFix, "needs_fix"),
            crate::review::Verdict::Rebuild => (NodeStatus::NeedsFix, "rebuild"),
        };
        g.set_status(milestone_id, status);
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.verdict = Some(vs.to_string());
        }
        let _ = g.save(&root);
        out.events.push(format!("milestone #{} ({}) auto-updated: {}", milestone_id, title, vs));
    }
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
}
