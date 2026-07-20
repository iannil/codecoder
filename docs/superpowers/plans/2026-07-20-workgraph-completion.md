# WorkGraph Completion Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close three gaps that keep the Work Graph from being a self-driving plan mechanism: system prompt visibility, review→milestone auto-writeback, and background-agent multi-milestone loop.

**Architecture:** Three independent modifications to existing files (`src/agent.rs`, `src/background.rs`, `src/workgraph.rs`), each adding 4–25 lines. No new files, no new tools, no schema changes.

**Tech Stack:** Rust, serde, channels (no async runtime).

## Global Constraints

- All new code must compile with `cargo build` and pass `cargo test` (160 tests, 0 failures)
- Follow existing patterns: pure functions in `review.rs`, tool dispatch in `agent.rs`, OS threads not tokio
- `Verdict::parse` is `fn` (private) in `review.rs` — `parse_review` is the public API

---

### Task 1: WorkGraph state injection into system prompt

**Files:**
- Modify: `src/agent.rs:1164-1179` (`build_system_prompt`)
- Test: `src/agent.rs` (existing test `trusted_project_loads_agents_md_and_allowlist`)

**Interfaces:**
- Consumes: `crate::workgraph::WorkGraph::read()`, `WorkGraph::render_for_prompt()` (already defined)
- Produces: system prompt string now includes workgraph status when nodes exist

- [ ] **Step 1: Modify `build_system_prompt` to append workgraph status**

In `src/agent.rs`, after the catalog block (line 1177) and before `parts.join(...)`, add:

```rust
    // Append workgraph status (Plan #2) so the agent is always aware of
    // outstanding milestones. Renders nothing when the graph is empty.
    let wg = crate::workgraph::WorkGraph::read(root);
    let wg_text = wg.render_for_prompt();
    if !wg_text.is_empty() {
        parts.push(wg_text);
    }
```

The complete function after the edit:

```rust
fn build_system_prompt(root: &std::path::Path) -> String {
    let mut parts = Vec::new();
    if let Ok(agents) = std::fs::read_to_string(root.join("AGENTS.md")) {
        let agents = agents.trim();
        if !agents.is_empty() {
            parts.push(agents.to_string());
        }
    }
    let catalog = Registry::scan(root).render_catalog();
    if !catalog.is_empty() {
        parts.push(catalog);
    }
    // Append workgraph status (Plan #2) so the agent is always aware of
    // outstanding milestones. Renders nothing when the graph is empty.
    let wg = crate::workgraph::WorkGraph::read(root);
    let wg_text = wg.render_for_prompt();
    if !wg_text.is_empty() {
        parts.push(wg_text);
    }
    parts.join("\n\n")
}
```

- [ ] **Step 2: Build and run existing tests**

```bash
cargo build 2>&1
cargo test agent::tests::trusted_project_loads_agents_md_and_allowlist -- --nocapture 2>&1
```
Expected: build succeeds, test passes (it checks that system_prompt contains `IDENTITY-MARKER`; workgraph is empty for this test so `render_for_prompt` returns empty string — no change to test outcome).

- [ ] **Step 3: Commit**

```bash
git add src/agent.rs
git commit -m "feat: inject workgraph status into system prompt (plan #2)"
```

---

### Task 2: Review→milestone auto-writeback in `drive_workgraph`

**Files:**
- Modify: `src/agent.rs:1140-1161` (`drive_workgraph`)
- Test: `src/agent.rs` (new test or extend existing milestone test)

**Interfaces:**
- Consumes: `crate::review::parse_review(text) -> ReviewOutcome` (public), `crate::workgraph::WorkGraph::read/set_status/save`, `self.root`, `self.last_assistant_text() -> String`
- Produces: after each `process_turn` in `drive_workgraph`, the milestone is updated: pass→Done with verdict, needs_fix/rebuild→NeedsFix with verdict

- [ ] **Step 1: Modify `drive_workgraph` to parse verdict and update milestone**

Replace the loop body in `drive_workgraph` (the `for _ in 0..MAX_AUTO` block) with:

```rust
fn drive_workgraph(&mut self, event_tx: &Sender<AgentEvent>) {
    use crate::workgraph::{NodeStatus, WorkGraph};
    const MAX_AUTO: usize = 3;
    for _ in 0..MAX_AUTO {
        let milestone_id = {
            let g = WorkGraph::read(&self.root);
            match g.next_ready() {
                Some(n) => n.id,
                None => break,
            }
        };
        // Build task text with the milestone details (no longer borrowing `g`).
        let (task, title) = {
            let g = WorkGraph::read(&self.root);
            let n = g.get(milestone_id).expect("just read, must exist");
            let task = format!(
                "workgraph milestone #{}: {}\nacceptance: {}\n\n\
                 Complete this milestone, then review your changes and report the \
                 verdict (pass / needs_fix / rebuild).",
                n.id,
                n.title,
                if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
            );
            (task, n.title.clone())
        };
        self.cancel.reset();
        self.process_turn(task, event_tx);

        // Auto-writeback: parse the turn's final assistant text for a review
        // verdict. When found, update the milestone status accordingly.
        let text = self.last_assistant_text();
        let outcome = crate::review::parse_review(&text);
        if !outcome.unparsed {
            let mut g = WorkGraph::read(&self.root);
            let (status, verdict_str) = match outcome.verdict {
                crate::review::Verdict::Pass => (NodeStatus::Done, "pass"),
                crate::review::Verdict::NeedsFix => (NodeStatus::NeedsFix, "needs_fix"),
                crate::review::Verdict::Rebuild => (NodeStatus::NeedsFix, "rebuild"),
            };
            g.set_status(milestone_id, status);
            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
                n.verdict = Some(verdict_str.to_string());
            }
            let _ = g.save(&self.root);
            let _ = event_tx.send(AgentEvent::Notice(format!(
                "milestone #{} ({}) auto-updated: {}",
                milestone_id, title, verdict_str,
            )));
        }
    }
}
```

- [ ] **Step 2: Build**

```bash
cargo build 2>&1
```
Expected: build succeeds.

- [ ] **Step 3: Run full test suite**

```bash
cargo test 2>&1
```
Expected: 160 passed, 0 failed, 4 ignored.

- [ ] **Step 4: Commit**

```bash
git add src/agent.rs
git commit -m "feat: review→milestone auto-writeback in drive_workgraph (plan #2)"
```

---

### Task 3: Background Agent multi-milestone loop

**Files:**
- Modify: `src/background.rs:46-91` (`run_background`)
- Test: `tests/l1_background.rs` (extend existing background tests)

**Interfaces:**
- Consumes: `resolve_bg_task()` (already defined), `WorkGraph::read/next_ready/set_status/save`, `crate::review::parse_review`
- Produces: `run_background` now runs up to 3 milestones sequentially when no explicit task is given

- [ ] **Step 1: Modify `run_background` to loop over workgraph milestones**

Replace the body of `run_background` from the `let (resolved, label)` line onward:

```rust
pub fn run_background(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    task: String,
) -> anyhow::Result<BgOutcome> {
    use crate::workgraph::{NodeStatus, WorkGraph};
    const MAX_AUTO: usize = 3;
    let mut out = BgOutcome::default();

    // Determine initial task from explicit arg or workgraph.
    let (initial_task, label) = resolve_bg_task(&task, &root);
    if initial_task.is_empty() && !label.starts_with("workgraph milestone") {
        out.events.push(label);
        return Ok(out);
    }
    out.events.push(format!("task: {label}"));

    // Build the agent once; reuse across milestones (same provider, model, root).
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());

    // Run the first turn (explicit task or first workgraph milestone).
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(initial_task, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);

    // If no explicit task was given, auto-advance through more workgraph milestones.
    if task.trim().is_empty() {
        for _ in 0..MAX_AUTO.saturating_sub(1) {
            let milestone_id = {
                let g = WorkGraph::read(&root);
                match g.next_ready() {
                    Some(n) => n.id,
                    None => break,
                }
            };
            let (task_text, title) = {
                let g = WorkGraph::read(&root);
                let n = g.get(milestone_id).expect("just read");
                let t = format!(
                    "workgraph milestone #{}: {}\nacceptance: {}\n\n\
                     Complete this milestone, then review your changes and report the \
                     verdict (pass / needs_fix / rebuild).",
                    n.id, n.title,
                    if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
                );
                (t, n.title.clone())
            };
            out.events.push(format!("task: workgraph milestone #{} ({})", milestone_id, title));
            let (tx, rx) = channel::<AgentEvent>();
            agent.run_one_turn(task_text, &tx);
            drop(tx);
            drain_bg_events(rx, &mut out);

            // Auto-writeback: parse verdict and update milestone.
            let text = &out.final_text;
            let outcome = crate::review::parse_review(text);
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
            AgentEvent::SubAgentMilestone(m) => out.events.push(format!("sub-agent: {m}")),
            _ => {}
        }
    }
}
```

- [ ] **Step 2: Build**

```bash
cargo build 2>&1
```
Expected: build succeeds.

- [ ] **Step 3: Run full test suite**

```bash
cargo test 2>&1
```
Expected: 160 passed, 0 failed, 4 ignored (plus 5 background tests still pass).

- [ ] **Step 4: Commit**

```bash
git add src/background.rs
git commit -m "feat: background agent auto-advances workgraph milestones (plan #2)"
```

---

## Verification

After all three tasks:

```bash
cargo test 2>&1
```
Expected output: `test result: ok. 160 passed; 0 failed; 2 ignored; ...`

The ignored tests are the pre-existing: 2 Docker e2e, L2 pty smoke, L3 real-LLM smoke.