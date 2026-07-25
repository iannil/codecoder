# CodeCoder Report Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the four genuinely-broken items from `docs/proof/experiment-report.md` (B: `bg_max_auto` default; C: headless observability; A: independent review gate; cc-web: read-only usability), and correct the report's misdiagnoses.

**Architecture:** Four independent phases in the existing Rust kernel. B is a one-line config default. C adds a `BgObserver` that tees background events to stderr + `<root>/.ccd.bg.ndjson`, draining live via a worker thread. A adds `AgentLoop::run_review` and swaps the BG milestone `review_runner` from parsing the agent's self-report to an independent read-only review sub-agent. cc-web is frontend-only edits to `static/index.html` (no new write routes).

**Tech Stack:** Rust (std threads + `crossbeam`/`mpsc` channels, `serde_json`, `tiny_http`), vanilla JS/HTML for cc-web. Tests via `cargo test` with `StubClient` (no API key needed). Frontend verified by manual smoke test (no JS test harness in repo).

## Global Constraints

- Do NOT weaken security boundaries: ADR 0036 compound-command keying, `.ccd.env` secret filtering, and BG empty-graph "no auto-seed" all stay as-is.
- cc-web stays strictly read-only this round: NO new POST / write routes.
- `NodeStatus` serializes `snake_case` (`pending`/`in_progress`/`blocked`/`needs_fix`/`done`) — frontend filters must match exactly.
- `Role` serializes lowercase (`user`/`assistant`/`system`); `MessageItem` is tagged by field `item` with `snake_case` values (`text`/`reasoning`/`tool_call`/`tool_result`).
- Full `cargo test` must stay green (currently 336 pass + 3 ignored).
- Every commit ends with the `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer.
- Work happens on branch `fix/report-issues`.

## File Structure

- `src/config.rs` — MODIFY: `bg_max_auto` default 3→10 + test assertion. (Task 1)
- `src/bg_observer.rs` — CREATE: `BgObserver` (stderr + NDJSON tee) + unit tests. (Task 2)
- `src/lib.rs` — MODIFY: `mod bg_observer;`. (Task 2)
- `src/background.rs` — MODIFY: thread the turn + live-drain with observer; milestone-level emits; swap `review_runner` to independent review. (Tasks 2, 3)
- `src/agent.rs` — MODIFY: add `pub fn run_review`; refactor the `review` tool handler to use it. (Task 3)
- `static/index.html` — MODIFY: blocked group; default landing = Workgraph + preload; readable session replay. (Tasks 4, 5, 6)
- `.gitignore` — MODIFY: ignore `.ccd.bg.ndjson`. (Task 2)
- `docs/adr/0037-bg-review-gate-and-observability.md` — CREATE. (Task 7)
- `docs/proof/experiment-report.md`, `README.md`, `CLAUDE.md`, `ARCHITECTURE.md` — MODIFY: corrections + numbers. (Task 7)

---

### Task 1: Bump `bg_max_auto` default 3 → 10

**Files:**
- Modify: `src/config.rs:62-64` (default), `src/config.rs:185` (test assertion)

**Interfaces:**
- Consumes: nothing.
- Produces: `Config::bg_max_auto` defaults to `10` when `CODECODER_BG_MAX_AUTO` unset.

- [ ] **Step 1: Update the failing test assertion first**

In `src/config.rs`, in `bg_env_defaults_and_overrides`, change:
```rust
        assert_eq!(c.bg_max_auto, 3);
```
to:
```rust
        assert_eq!(c.bg_max_auto, 10);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test bg_env_defaults_and_overrides`
Expected: FAIL — `assertion left == right failed: left: 3, right: 10`.

- [ ] **Step 3: Change the default**

In `src/config.rs`, change:
```rust
            bg_max_auto: env("CODECODER_BG_MAX_AUTO")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
```
to:
```rust
            bg_max_auto: env("CODECODER_BG_MAX_AUTO")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test bg_env_defaults_and_overrides`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): bump bg_max_auto default 3->10

Circuit breaker (bg_circuit_k) still bounds failures; larger auto budget
covers most real projects without manual CODECODER_BG_MAX_AUTO.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `BgObserver` — live headless observability (stderr + NDJSON)

**Files:**
- Create: `src/bg_observer.rs`
- Modify: `src/lib.rs` (add `mod bg_observer;`), `.gitignore`
- Modify: `src/background.rs` (drain sites + milestone emits)

**Interfaces:**
- Produces:
  - `pub struct BgObserver`
  - `pub fn BgObserver::new(root: &std::path::Path) -> BgObserver` — truncate-creates `<root>/.ccd.bg.ndjson`; NDJSON disabled (None) if the file can't be opened (stderr still works).
  - `pub fn BgObserver::emit(&mut self, kind: &str, msg: &str)` — writes `[bg] {kind}: {msg}\n` to stderr AND one line `{"kind":<kind>,"msg":<msg>}` (serde_json-escaped) to the NDJSON file, flushing each line.
- Consumes: `drain_bg_events` gains a `obs: &mut BgObserver` parameter.

- [ ] **Step 1: Write the failing test**

Create `src/bg_observer.rs`:
```rust
//! Live observability for headless Background runs (spec 2026-07-25, ADR 0037).
//! Tees each event to stderr (human) and `<root>/.ccd.bg.ndjson` (machine/tail).
use std::io::Write;
use std::path::Path;

pub struct BgObserver {
    ndjson: Option<std::fs::File>,
}

impl BgObserver {
    /// Truncate-create `<root>/.ccd.bg.ndjson`. NDJSON is best-effort: if the file
    /// can't be opened, stderr output still happens.
    pub fn new(root: &Path) -> Self {
        let path = root.join(".ccd.bg.ndjson");
        let ndjson = std::fs::File::create(&path).ok();
        Self { ndjson }
    }

    /// Emit one event: stderr line + one JSON line to the NDJSON file.
    pub fn emit(&mut self, kind: &str, msg: &str) {
        eprintln!("[bg] {kind}: {msg}");
        if let Some(f) = self.ndjson.as_mut() {
            let line = serde_json::json!({ "kind": kind, "msg": msg }).to_string();
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ndjson_appends_one_valid_json_line_per_emit() {
        let dir = tempdir().unwrap();
        let mut obs = BgObserver::new(dir.path());
        obs.emit("tool_started", "run_command");
        obs.emit("gate", "pass");
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "one line per emit");
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["kind"], "tool_started");
        assert_eq!(v0["msg"], "run_command");
        let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v1["kind"], "gate");
        assert_eq!(v1["msg"], "pass");
    }

    #[test]
    fn new_truncates_prior_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".ccd.bg.ndjson"), "stale\n").unwrap();
        let mut obs = BgObserver::new(dir.path());
        obs.emit("k", "v");
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        assert!(!body.contains("stale"), "prior content truncated");
        assert_eq!(body.lines().count(), 1);
    }
}
```

- [ ] **Step 2: Register the module**

In `src/lib.rs`, add alongside the other `mod` declarations:
```rust
mod bg_observer;
```

- [ ] **Step 3: Run tests to verify they fail, then pass**

Run: `cargo test bg_observer`
Expected: compiles and PASSES (module is self-contained). If `tempfile` isn't a dev-dependency, confirm it is (it's already used across the suite, e.g. `bg_gate.rs` tests).

- [ ] **Step 4: Ignore the NDJSON artifact**

Add to `.gitignore`:
```
.ccd.bg.ndjson
```

- [ ] **Step 5: Wire `BgObserver` into `drain_bg_events`**

In `src/background.rs`, change the signature and body of `drain_bg_events` to emit per event:
```rust
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
            _ => {}
        }
    }
}
```
(Keep the tail `}` structure identical to the original — only the parameter and the added `obs.emit(...)` calls change.)

- [ ] **Step 6: Live-drain the milestone turn on a worker thread**

In `run_milestone_and_gate` (`src/background.rs`), replace the synchronous run+drain block:
```rust
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(task_text, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);
```
with a threaded live drain so events stream while the turn runs:
```rust
    let mut obs = crate::bg_observer::BgObserver::new(&root);
    obs.emit("milestone_start", &format!("#{milestone_id} {title}"));
    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(task_text, &tx);
        drop(tx);
        agent // hand the agent back so we can read last_error()
    });
    drain_bg_events(rx, &mut out, &mut obs);
    let agent = handle.join().expect("bg turn thread panicked");
    if let Some(e) = agent.last_error() {
        return Err(anyhow::anyhow!(e.to_string()));
    }
```
Note: `cancel` is captured from `agent.cancel_token()` BEFORE the move (that line already precedes this block at `let cancel = agent.cancel_token();`) — keep it. `obs` is reused by the gate emits in Step 7, so declare it here (before the gate).

- [ ] **Step 7: Emit the gate verdict + status writeback**

Still in `run_milestone_and_gate`, right after `let verdict = crate::bg_gate::evaluate(...)` and after `status`/`vs_str` are computed, add:
```rust
    obs.emit("gate", &format!("#{milestone_id} {vs_str}: {gate_reason}"));
```
(Place it after `gate_reason` is defined near `src/background.rs:399-402`.)

- [ ] **Step 8: Fix the other `drain_bg_events` call site**

In `run_background_cfg`'s explicit-task branch (`src/background.rs` ~line 160-165), update the call to pass an observer:
```rust
        let mut obs = crate::bg_observer::BgObserver::new(&root);
        let (tx, rx) = channel::<AgentEvent>();
        let handle = std::thread::spawn(move || { agent.run_one_turn(task, &tx); drop(tx); agent });
        drain_bg_events(rx, &mut out, &mut obs);
        let agent = handle.join().expect("bg turn thread panicked");
        if let Some(e) = agent.last_error() {
            out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
        }
```
(Replaces the existing `agent.run_one_turn(task, &tx); drop(tx); drain_bg_events(rx, &mut out);` and the following `if let Some(e) = agent.last_error()` block. `root` must be cloned before the move if still needed afterward — clone as `let root = root.clone();` at branch top if the borrow checker complains.)

- [ ] **Step 9: Emit mission_state terminal in the workgraph loop**

In `run_background_cfg`'s workgraph loop, just before each `break` that sets `out.mission_state`, add a single observer emit. Simplest: after the loop, once, add:
```rust
    // observability: final mission state
    crate::bg_observer::BgObserver::new(&root).emit("mission_state", &format!("{:?}", out.mission_state));
```
Place this immediately before `Ok(out)` returns from the workgraph branch. (A fresh observer append-truncates — acceptable, it's the last line; OR reuse a loop-scoped observer if one is already open. Prefer: open one `let mut loop_obs = BgObserver::new(&root);` at the top of the workgraph branch and call `loop_obs.emit(...)` here to avoid truncating mid-run.)

- [ ] **Step 10: Verify the whole suite compiles and passes**

Run: `cargo test`
Expected: PASS (336+ tests). Fix any borrow/move errors surfaced by the threading change (typical: clone `root`/`title` before the `move` closure).

- [ ] **Step 11: Smoke-test live output**

Run (throwaway dir):
```bash
D=$(mktemp -d); printf '# A\n' > "$D/AGENTS.md"
printf '{"version":1,"nodes":[{"id":1,"title":"t","acceptance":"true","command":"true","status":"pending","deps":[]}]}' > "$D/workgraph.json"
CODECODER_ROOT="$D" CODECODER_BG_WORKGRAPH=1 cargo run --bin codecoder 2>&1 | grep '\[bg\]' | head
cat "$D/.ccd.bg.ndjson" | head
```
Expected: `[bg] milestone_start ...`, `[bg] gate #1 pass ...`, `[bg] mission_state ...` on stderr AND matching JSON lines in `.ccd.bg.ndjson`.

- [ ] **Step 12: Commit**

```bash
git add src/bg_observer.rs src/lib.rs src/background.rs .gitignore
git commit -m "feat(background): live headless observability via BgObserver

Tee BG events to stderr + <root>/.ccd.bg.ndjson; drain live on a worker
thread so tool calls / gate verdicts / mission_state stream during the run.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Independent review gate (A)

**Files:**
- Modify: `src/agent.rs` (add `run_review`; refactor `review` tool handler at `:1135-1145`)
- Modify: `src/background.rs` (`review_runner` in `run_milestone_and_gate`)

**Interfaces:**
- Produces: `pub fn AgentLoop::run_review(&mut self, target: &str, event_tx: &Sender<AgentEvent>) -> crate::review::ReviewOutcome`.
- Consumes: `crate::review::{review_task, parse_review, ReviewOutcome, Verdict}`; `crate::bg_gate::GateVerdict`.

- [ ] **Step 1: Write the failing test for `run_review`**

In `src/agent.rs` tests module, add:
```rust
#[test]
fn run_review_returns_structured_outcome_under_stub() {
    use crate::provider::StubClient;
    use std::sync::mpsc::channel;
    let dir = tempfile::tempdir().unwrap();
    let mut agent = AgentLoop::new_background(
        std::sync::Arc::new(StubClient::default()),
        "stub".into(), 256, 0.0, dir.path().to_path_buf(),
    );
    let (tx, _rx) = channel();
    let outcome = agent.run_review("the current changes", &tx);
    // Stub yields parseable-or-default review text → a concrete verdict exists.
    let _ = outcome.verdict; // does not panic; type is ReviewOutcome
}
```
(Match `StubClient`'s real constructor — if it's `StubClient::new()` not `default()`, use that. Verify with `grep -n "impl StubClient" src/provider/*.rs`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test run_review_returns_structured_outcome_under_stub`
Expected: FAIL — `no method named run_review`.

- [ ] **Step 3: Add `run_review` and DRY the tool handler**

In `src/agent.rs`, add the method near `spawn_sub_agent_text`:
```rust
    /// Run an independent read-only review sub-agent against `target` and parse
    /// its prose into a structured verdict. Reused by the `review` tool and the
    /// Background review gate (ADR 0037).
    pub fn run_review(
        &mut self,
        target: &str,
        event_tx: &Sender<AgentEvent>,
    ) -> crate::review::ReviewOutcome {
        let raw = self.spawn_sub_agent_text(crate::review::review_task(target), event_tx);
        crate::review::parse_review(&raw)
    }
```
Then refactor the `review` tool handler (`src/agent.rs:1135-1145`) to use it:
```rust
        if name == "review" {
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("the current changes");
            let outcome = self.run_review(target, event_tx);
            let raw = crate::review::format_result(&outcome, "");
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: raw,
                is_error: false,
            });
        }
```
Note: the original passed the sub-agent's raw prose to `format_result`. To preserve that exactly, instead have `run_review` also return the raw text, OR keep the handler inline. Simplest DRY that preserves behavior — make `run_review` return `(ReviewOutcome, String)`:
```rust
    pub fn run_review(
        &mut self,
        target: &str,
        event_tx: &Sender<AgentEvent>,
    ) -> (crate::review::ReviewOutcome, String) {
        let raw = self.spawn_sub_agent_text(crate::review::review_task(target), event_tx);
        let outcome = crate::review::parse_review(&raw);
        (outcome, raw)
    }
```
and the handler:
```rust
        if name == "review" {
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("the current changes");
            let (outcome, raw) = self.run_review(target, event_tx);
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: crate::review::format_result(&outcome, &raw),
                is_error: false,
            });
        }
```
Update the Step-1 test to bind `let (outcome, _raw) = agent.run_review(...);`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test run_review_returns_structured_outcome_under_stub`
Expected: PASS. Also run `cargo test sub_agent` to confirm the existing review/sub-agent tests still pass.

- [ ] **Step 5: Swap the BG `review_runner` to independent review**

In `src/background.rs` `run_milestone_and_gate`, capture provider/model clones BEFORE the milestone agent consumes `provider` (i.e. clone at the top of the function, before `AgentLoop::new_background(provider, ...)`):
```rust
    let review_provider = provider.clone();
    let review_model = model.clone();
```
Then replace the `review_runner` closure (`src/background.rs:382-391`) with:
```rust
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
```
Confirm `CancelToken` is `Clone` (it wraps an `Arc`); if `cancel` is `Option<&CancelToken>` in scope, adjust to clone the underlying token. The `evaluate(&m, &root, Some(&cancel), &review_runner)` call stays unchanged.

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: PASS. The `bg_gate::evaluate_*` tests inject their own fake `review_runner` and are unaffected. Fix any move/borrow issues (clone before move).

- [ ] **Step 7: Smoke-test a Review-kind milestone**

```bash
D=$(mktemp -d); printf '# A\n' > "$D/AGENTS.md"
printf '{"version":1,"nodes":[{"id":1,"title":"t","acceptance":"renderer output is correct","status":"pending","deps":[]}]}' > "$D/workgraph.json"
CODECODER_ROOT="$D" CODECODER_BG_WORKGRAPH=1 cargo run --bin codecoder 2>&1 | grep -E '\[bg\] gate' | head
```
Expected: gate line shows a verdict from `independent review: ...` (not `self-review: ...`) for the prose-acceptance milestone.

- [ ] **Step 8: Commit**

```bash
git add src/agent.rs src/background.rs
git commit -m "feat(background): BG review gate uses independent review sub-agent

Add AgentLoop::run_review (DRYs the review tool handler); BG Review-kind
milestones now get an objective read-only review that overrides the agent's
self-reported VERDICT. Falls back to self-report on cancel.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: cc-web #3 — render `blocked` milestones

**Files:**
- Modify: `static/index.html` (`renderWorkgraph`, ~lines 263-273)

**Interfaces:**
- Consumes: `/api/v1/workgraph` JSON with `status` values in `snake_case`.
- Produces: a "Blocked" group in the Workgraph tab.

- [ ] **Step 1: Add the blocked filter + group**

In `static/index.html` `renderWorkgraph`, after the existing status filters add:
```javascript
  const blocked = nodes.filter(n => n.status === 'blocked');
```
and add a group entry to the `groups` array (place after "Need Fix"):
```javascript
    { label: 'Blocked', items: blocked, color: '#a371f7' },
```

- [ ] **Step 2: Smoke-test the render**

```bash
D=$(mktemp -d); printf '# A\n' > "$D/AGENTS.md"
printf '{"version":1,"nodes":[{"id":1,"title":"dep","acceptance":"x","status":"needs_fix","deps":[]},{"id":2,"title":"downstream","acceptance":"y","status":"blocked","deps":[1]}]}' > "$D/workgraph.json"
CODECODER_ROOT="$D" CODECODER_DAEMON=1 cargo run --bin codecoder >/tmp/d.log 2>&1 &
sleep 2
CODECODER_ROOT="$D" cargo run --bin cc-web -- --port 9901 --daemon-socket "$D/.ccd.sock" >/tmp/w.log 2>&1 &
sleep 2
curl -s http://localhost:9901/api/v1/workgraph | grep -q '"status":"blocked"' && echo "API OK: blocked node present"
# Open http://localhost:9901, click Workgraph tab, confirm a purple "Blocked (1)" group shows #2.
kill %1 %2 2>/dev/null; rm -rf "$D"
```
Expected: API returns the blocked node; Workgraph tab shows a "Blocked (1)" group. (Visual confirm in browser.)

- [ ] **Step 3: Commit**

```bash
git add static/index.html
git commit -m "fix(cc-web): render blocked milestones in Workgraph tab

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: cc-web #1 + #6 — default landing = Workgraph, preload on load

**Files:**
- Modify: `static/index.html` (tab-bar/tab-content active classes ~lines 87-97; add initial `loadWorkgraph()` after the tab-click wiring ~line 342)

**Interfaces:**
- Consumes: `loadWorkgraph()` (already defined at ~line 247).
- Produces: Workgraph is the active tab on load and is populated without a click.

- [ ] **Step 1: Move the `active` class from timeline to workgraph**

In the tab-bar (`static/index.html:87-92`), change so `workgraph` is active and `timeline` is not:
```html
<div id="tab-bar">
  <div class="tab" data-tab="timeline">实时时间线</div>
  <div class="tab active" data-tab="workgraph">Workgraph</div>
  <div class="tab" data-tab="sessions">Session 回放</div>
  <div class="tab" data-tab="tests">测试热力图</div>
</div>
```
And in the tab-content divs (`:94-95`), move `active`:
```html
<div id="timeline" class="tab-content"><div id="timeline-hint" style="padding:20px;color:#8b949e;line-height:1.6;">⏳ 等待实时事件…<br>daemon 空闲时时间线为空。此处在有活动 turn 时实时更新。</div></div>
<div id="workgraph" class="tab-content active"><p style="padding:20px;color:#8b949e;">Loading workgraph…</p></div>
```
(Leave `sessions`/`tests` tab-content as-is.)

- [ ] **Step 2: Preload the workgraph on page load**

Immediately after the tab-click wiring block (`static/index.html` ~line 342, after the `forEach` that adds click listeners), add:
```javascript
// Landing tab is Workgraph — populate it immediately (don't wait for a click).
loadWorkgraph();
```

- [ ] **Step 3: Smoke-test the landing experience**

Reuse the Task 4 smoke harness but do NOT click anything:
```bash
# ... start daemon + cc-web on :9901 as in Task 4 ...
# Open http://localhost:9901 fresh. Expected: Workgraph tab is active and shows
# milestone cards WITHOUT any click; timeline is a background tab.
```
Expected: first paint shows the Workgraph content; no empty "⏳ 等待实时事件…" as the landing screen.

- [ ] **Step 4: Commit**

```bash
git add static/index.html
git commit -m "fix(cc-web): land on Workgraph tab and preload it on page load

Idle daemons no longer show an empty timeline as the first screen.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: cc-web #5 — readable session replay

**Files:**
- Modify: `static/index.html` (`loadSessionDetail`, ~lines 363-372)

**Interfaces:**
- Consumes: `/api/v1/sessions/{id}` → the raw `Session` JSON: `{ entries: [{ message: { role, items: [ {item, ...} ] } }] }`. `role` is lowercase; each item is tagged by `item` (`text`/`reasoning`/`tool_call`/`tool_result`).
- Produces: a message-by-message rendered replay instead of a raw JSON dump.

- [ ] **Step 1: Replace the raw dump with a message renderer**

In `static/index.html`, replace `loadSessionDetail` body (keep the async signature and the "← Back" header) so it renders `entries` in storage order:
```javascript
async function loadSessionDetail(id) {
  const resp = await fetch(`/api/v1/sessions/${id}`);
  const data = await resp.json();
  const container = document.getElementById('sessions');
  container.innerHTML = `<div style="padding:12px 16px;"><a href="#" onclick="loadSessions();return false;">← Back</a> <span style="color:#8b949e;">Session ${escapeHtml(id)}</span></div>`;
  const entries = (data.entries || []);
  for (const entry of entries) {
    const m = entry.message || {};
    const role = m.role || 'unknown';
    const roleColor = role === 'user' ? '#58a6ff' : role === 'assistant' ? '#3fb950' : '#8b949e';
    const block = document.createElement('div');
    block.style.cssText = 'margin:8px 12px;padding:8px 12px;background:#161b22;border-left:3px solid ' + roleColor + ';border-radius:4px;';
    const hdr = document.createElement('div');
    hdr.style.cssText = 'font-size:11px;text-transform:uppercase;color:' + roleColor + ';margin-bottom:4px;';
    hdr.textContent = role;
    block.appendChild(hdr);
    for (const it of (m.items || [])) {
      const line = document.createElement('div');
      line.style.cssText = 'font-size:13px;white-space:pre-wrap;margin:2px 0;';
      if (it.item === 'text') {
        line.textContent = it.text || '';
      } else if (it.item === 'reasoning') {
        line.style.color = '#8b949e';
        line.textContent = '💭 ' + (it.text || '');
      } else if (it.item === 'tool_call') {
        line.style.color = '#d29922';
        line.textContent = '🔧 ' + (it.name || '') + '(' + JSON.stringify(it.args || {}) + ')';
      } else if (it.item === 'tool_result') {
        line.style.color = it.is_error ? '#f85149' : '#8b949e';
        line.textContent = '↳ ' + truncateOutput(String(it.output || ''), 300);
      }
      block.appendChild(line);
    }
    container.appendChild(block);
  }
  if (entries.length === 0) {
    const empty = document.createElement('p');
    empty.style.cssText = 'padding:20px;color:#8b949e;';
    empty.textContent = 'Empty session.';
    container.appendChild(empty);
  }
}
```
(`escapeHtml` and `truncateOutput` already exist in the file.)

- [ ] **Step 2: Smoke-test with a real session**

```bash
# Use an existing project that has sessions/, or drive one turn via cc to create a session.
# Start daemon + cc-web (as Task 4), open http://localhost:9901, click "Session 回放",
# click a session card. Expected: role-labelled message blocks with text / 🔧 tool calls /
# ↳ tool results — NOT a raw JSON blob.
```
Expected: readable, role-colored replay.

- [ ] **Step 3: Commit**

```bash
git add static/index.html
git commit -m "fix(cc-web): readable session replay (role + items) instead of raw JSON

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Docs — correct the report, record ADR, update numbers

**Files:**
- Modify: `docs/proof/experiment-report.md`
- Create: `docs/adr/0037-bg-review-gate-and-observability.md`
- Modify: `README.md`, `CLAUDE.md`, `ARCHITECTURE.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Correct the experiment report**

In `docs/proof/experiment-report.md`, edit the problem tables so they reflect reality:
- #1: change "Review gate 未实现 (`bg_gate.rs:295` panic)" → "Review gate 早已实现;局限是仅解析 agent 自报 VERDICT。本轮已升级为独立评审(ADR 0037)。`bg_gate.rs:295` 实为单测断言,非运行时 panic。"
- #2 / #3 / #5: append "(working-as-designed:ADR 0036 / ADR 0033 / config.rs 密钥过滤;刻意保留)".
- Add a one-line note under §4.1: "以下条目经源码核实为误诊或刻意设计,详见 `docs/superpowers/specs/2026-07-25-codecoder-report-fixes-design.md`。"

- [ ] **Step 2: Write ADR 0037**

Create `docs/adr/0037-bg-review-gate-and-observability.md` (follow the format of a recent ADR like 0036):
```markdown
# ADR 0037 — BG Review 门(独立评审)与 headless 可观测性

- **状态**: Accepted
- **日期**: 2026-07-25
- **关联**: ADR 0026(headless runner)、spec 2026-07-25-codecoder-report-fixes-design

## 背景
BG 的 Review-kind 里程碑此前只解析 agent 自报的 `VERDICT:`(background.rs review_runner),
主观且易被乐观自评通过;且 headless 运行期事件只在 turn 结束后浮现,无法实时观察。

## 决策
1. **独立评审门**:Review-kind 里程碑改由独立只读评审子 agent(`AgentLoop::run_review`,
   复用 `review.rs` 漂移评分)客观判定,覆盖自报;取消时降级回自报解析。
2. **实时可观测**:`BgObserver` 把 BG 事件同时写 stderr 与 `<root>/.ccd.bg.ndjson`(每行一 JSON),
   milestone turn 改为工作线程运行、主线程 live-drain。
3. **默认预算**:`bg_max_auto` 默认 3→10(熔断 `bg_circuit_k` 仍兜底)。

## 后果
- 正面:验收更客观;运行可 tail 观察;更大项目默认可跑完。
- 代价:每个 Review 里程碑多一次 LLM 子调用(Command 门不受影响);多一线程与一 NDJSON 产物(已 gitignore)。
- 不做:cc-web 写操作、真实测试热力图、BG 空图自动播种(仍守 ADR 0033/0036)。
```

- [ ] **Step 3: Update README / CLAUDE.md / ARCHITECTURE.md**

- `README.md`: in the env-var table, change `CODECODER_BG_MAX_AUTO` default note to `10`; add a line for `.ccd.bg.ndjson` live BG log. If a test count appears, bump `336` by the number of new tests added (Task 2: +2, Task 3: +1 → `339`).
- `CLAUDE.md`: in the Background Agent paragraph, note `bg_max_auto` default 10 and the `BgObserver` stderr+NDJSON observability; note BG Review gate is now independent review (ADR 0037).
- `ARCHITECTURE.md`: add cc-web read-only capability note (landing on Workgraph, blocked rendering, readable session replay) if cc-web is described there; else skip.

- [ ] **Step 4: Verify docs reference real paths**

Run: `cargo test` one final time (docs changes don't affect tests, but confirm the tree is still green after all tasks).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add docs/ README.md CLAUDE.md ARCHITECTURE.md
git commit -m "docs: correct experiment-report misdiagnoses, add ADR 0037, update numbers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** B→Task 1; C→Task 2; A→Task 3; cc-web #3→Task 4, #1/#6→Task 5, #5→Task 6; report correction + "re-examine intentional" outcome + ADR + doc numbers→Task 7. All spec sections mapped.
- **Out-of-scope preserved:** no task touches ADR 0036 keying, `.ccd.env` filtering, BG auto-seed, or adds cc-web write routes (Global Constraints).
- **Type consistency:** `run_review` returns `(ReviewOutcome, String)` in both Task 3 uses; `drain_bg_events` gains `obs: &mut BgObserver` at all call sites (Task 2 Steps 5/6/8); `NodeStatus`/`Role`/`MessageItem` JSON shapes match Global Constraints.
- **Verify-before-trust:** Task 3 Step 1 notes to confirm `StubClient` constructor; Task 2 notes `tempfile` is already a dev-dep; Task 2 Step 6/8 note the `root`/`title` clone-before-move hazard from threading.
