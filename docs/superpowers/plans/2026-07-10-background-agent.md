# Background Agent (headless runner) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a Background Agent — a full-LLM-loop agent that runs one delegated task headless (no TUI, no user present), triggered by `CODECODER_BG_TASK`, using pre-authorized `codecoder.json` permissions with auto-Deny for anything unauthorized, then exits.

**Architecture:** Add a `headless: bool` to `AgentLoop` that only alters the "not pre-authorized" branch of the permission gate (and the `ask_user`/`confirm`/`plan` intercepts) — so interactive TUI behavior is untouched. A new `src/background.rs` drives one turn on the current thread and drains events into a `BgOutcome`. `lib.rs::run_background` wraps it; `main.rs` dispatches on the env var.

**Tech Stack:** Rust; existing `AgentLoop`/`Provider`/`ProjectAllowlist`/`SessionAllowlist`; std mpsc channels.

## Global Constraints

- Background Agent uses `Toolbox::builtin()` (full tools — can write/run), NOT the sub-agent read-only set. The distinction is enforced by the headless permission model, not a restricted toolset.
- Permission with no user: `Permission::Ask { key }` runs only if `key` is in the session OR project allowlist; **else auto-Deny — never send `PermissionRequest`/block on a oneshot** (no one answers).
- `ask_user` / `confirm` / `plan` (PlanApproval) in headless: return a rejection/default `ToolResult` immediately, do not emit the interactive event.
- **Zero regression:** `headless` defaults to `false`; interactive (TUI) permission + ask behavior must be byte-for-byte unchanged. Existing suite stays green.
- Entry: `CODECODER_BG_TASK=<task>` (non-empty) → `run_background`; else → existing `run` (TUI).
- Session persists as usual (ADR 0004); results also printed to stdout. Exit 0 on success, non-zero on LLM/transport error.
- Out of MVP (do not build): SIGINT/cancel wiring, built-in scheduler, multi-runner limits.
- Commit messages end with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

## File Structure

- `src/agent.rs` — add `headless` field (`build` param); gate the Ask "unauthorized" branch + `ask_user`/`confirm`/`plan` on `headless`; add `pub fn new_background(...)` + `pub fn run_one_turn(...)`; unit test.
- `src/background.rs` (new) — `BgOutcome` + `run_background(provider, model, max_tokens, temperature, root, task)`.
- `src/lib.rs` — `mod background;` + `pub fn run_background(cfg, task)`; re-export `BgOutcome`.
- `src/main.rs` — env-var dispatch.
- `tests/l1_background.rs` (new) — 3 black-box tests.
- `docs/adr/0026-background-agent-headless-runner.md` (new); `CONTEXT.md`, `README.md`, `ARCHITECTURE.md`, `CLAUDE.md` — status + counts.

Verified types: `AgentLoop::build(provider: Arc<dyn Provider>, model: String, max_tokens: u32, temperature: f32, root: PathBuf, toolbox: Toolbox, persist: bool) -> Self`; `Toolbox::builtin()`; `process_turn(&mut self, text: String, event_tx: &Sender<AgentEvent>)`; permission gate at agent.rs ~473 already does `if !self.allowlist.allows(&key) && !self.project_allowlist.allows(&key) { <emit PermissionRequest, block on reply_rx> }`; `ToolOutcome::Result(MessageItem::ToolResult { call_id, output, is_error })`; `AgentEvent::{PermissionRequest, AskUser, Confirm, PlanApproval, TurnComplete, ...}`; `Provider`, `CompletionRequest`; `Config`.

---

### Task 1: `headless` mode in `AgentLoop`

**Files:**
- Modify: `src/agent.rs`

**Interfaces:**
- Produces: `AgentLoop::new_background(provider: Arc<dyn Provider>, model: impl Into<String>, max_tokens: u32, temperature: f32, root: PathBuf) -> Self`; `AgentLoop::run_one_turn(&mut self, task: String, event_tx: &Sender<AgentEvent>)`; a `headless: bool` field defaulting to `false` for `new`/`new_sub`.

- [ ] **Step 1: Add the `headless` field + thread it through `build`**

In `pub struct AgentLoop`, add after `cancel: CancelToken,` (and before `tier2`):

```rust
    /// No user is present (Background Agent, ADR 0026). Changes the permission
    /// gate: an Ask-tool not in an allowlist is auto-denied instead of prompting,
    /// and ask_user/confirm/plan short-circuit — there is no one to answer.
    headless: bool,
```

Change `fn build(...)` signature to take `headless`:

```rust
    fn build(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
        toolbox: Toolbox,
        persist: bool,
        headless: bool,
    ) -> Self {
```

Add `headless,` to the struct literal it returns (next to `tier2: None,`).

Update the two existing callers of `build`:
- In `new(...)`: `Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true, false)`
- In `new_sub(...)`: `Self::build(provider, model, max_tokens, temperature, root, Toolbox::read_only_child(), false, false)`

- [ ] **Step 2: Add `new_background` + `run_one_turn`**

Add to an `impl AgentLoop { ... }` block (near `new`):

```rust
    /// A Background Agent (ADR 0026): full builtin toolbox, persists its session,
    /// but runs headless (no user present) — the permission gate auto-denies any
    /// Ask-tool not pre-authorized in an allowlist.
    pub fn new_background(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
    ) -> Self {
        Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true, true)
    }

    /// Drive exactly one turn to completion (headless). Thin public wrapper over
    /// the internal turn loop so a Background runner can invoke it without the
    /// full command-channel run loop.
    pub fn run_one_turn(&mut self, task: String, event_tx: &Sender<AgentEvent>) {
        self.cancel.reset();
        self.process_turn(task, event_tx);
    }
```

- [ ] **Step 3: Write the failing unit test (headless auto-Deny, no PermissionRequest)**

Append to the `#[cfg(test)] mod tests` in `src/agent.rs`. It drives a headless agent with a `StubClient`-like scripted provider that requests one `write_file` (an Ask tool) with no allowlist entry, and asserts: the file is NOT written AND no `PermissionRequest` event was emitted. Use the existing test scaffolding in the module (look at `ask_user_round_trip` for the provider/event pattern). Concretely:

```rust
    #[test]
    fn headless_auto_denies_unauthorized_ask_tool_without_prompting() {
        use std::sync::mpsc::channel;
        // Scripted provider: first reply calls write_file; second reply is bare text
        // so the tool loop terminates.
        struct WriteThenStop { n: std::sync::Mutex<u32> }
        impl Provider for WriteThenStop {
            fn name(&self) -> &str { "write-then-stop" }
            fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Message> {
                let mut n = self.n.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    Ok(Message::new(0, Role::Assistant, vec![MessageItem::ToolCall {
                        id: "c1".into(), name: "write_file".into(),
                        args: serde_json::json!({"path": "hacked.txt", "content": "x"}),
                    }]))
                } else {
                    Ok(Message::text(0, Role::Assistant, "done"))
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("cc_bg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = std::sync::Arc::new(WriteThenStop { n: std::sync::Mutex::new(0) });
        let mut agent = AgentLoop::new_background(provider, "test-model", 4096, 0.0, dir.clone());
        let (tx, rx) = channel();
        agent.run_one_turn("write a file".into(), &tx);
        drop(tx);
        let events: Vec<_> = rx.into_iter().collect();
        // No permission prompt was emitted (no one to answer in headless).
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })),
            "headless must not emit PermissionRequest");
        // The unauthorized write did not happen.
        assert!(!dir.join("hacked.txt").exists(), "unauthorized write_file must be denied");
        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test --lib headless_auto_denies 2>&1 | tail -15`
Expected: FAIL — currently headless has no special gate, so the agent blocks on `reply_rx.recv()` (test hangs) OR (once the field exists but gate unchanged) emits `PermissionRequest`. This proves the gate change is needed.

> If the test HANGS (blocks on the oneshot) rather than fails cleanly, that is the exact bug this task fixes — proceed to Step 5; after the fix it will terminate.

- [ ] **Step 5: Gate the permission "unauthorized" branch on `headless`**

In `dispatch_tool`, the permission gate currently reads:

```rust
        if let Permission::Ask { key } = tool.permission(&args, &self.root) {
            if !self.allowlist.allows(&key) && !self.project_allowlist.allows(&key) {
                let (reply_tx, reply_rx) = channel();
                let _ = event_tx.send(AgentEvent::PermissionRequest { ... });
                match reply_rx.recv() { ... }
            }
        }
```

Insert a headless short-circuit as the FIRST thing inside the `if !allowed` block (before creating the channel):

```rust
            if !self.allowlist.allows(&key) && !self.project_allowlist.allows(&key) {
                if self.headless {
                    return ToolOutcome::Result(MessageItem::ToolResult {
                        call_id: call_id.to_string(),
                        output: format!("denied: no user present; '{key}' not in project allowlist"),
                        is_error: true,
                    });
                }
                let (reply_tx, reply_rx) = channel();
                // ... existing interactive path unchanged ...
```

Leave the entire existing interactive `match reply_rx.recv()` path untouched below the new short-circuit.

- [ ] **Step 6: Short-circuit `ask_user` / `confirm` / `plan` in headless**

In `dispatch_tool`, at the intercept block, guard each interactive tool. Change:

```rust
        if name == "ask_user" {
            return self.ask_user(call_id, &args, event_tx);
        }
        if name == "plan" {
            return self.plan(call_id, &args, event_tx);
        }
        if name == "confirm" {
            let prompt = ...;
            let (reply_tx, reply_rx) = channel();
            let _ = event_tx.send(AgentEvent::Confirm { prompt, reply_tx });
            let yes = reply_rx.recv().unwrap_or(false);
            return ToolOutcome::Result(...);
        }
```

to short-circuit when headless (add these three guards right before each existing intercept):

```rust
        if self.headless && (name == "ask_user" || name == "confirm" || name == "plan") {
            return ToolOutcome::Result(MessageItem::ToolResult {
                call_id: call_id.to_string(),
                output: format!("denied: '{name}' requires a user, none present (headless)"),
                is_error: true,
            });
        }
```

Place this guard ABOVE the `if name == "ask_user"` block. (The `agent` sub-agent tool is NOT guarded — a Background Agent may still delegate to a read-only sub-agent.)

- [ ] **Step 7: Run the test + full suite**

Run: `cargo test --lib headless_auto_denies 2>&1 | tail -8`
Expected: PASS (no PermissionRequest, no file written, test terminates).
Run: `cargo test 2>&1 | tail -6`
Expected: whole suite green — interactive behavior unchanged (headless defaults to false everywhere).

- [ ] **Step 8: Commit**

```bash
git add src/agent.rs
git commit -m "feat(agent): headless mode — auto-deny unauthorized Ask tools, no prompt

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `background.rs` runner + lib/main wiring

**Files:**
- Create: `src/background.rs`
- Modify: `src/lib.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `AgentLoop::{new_background, run_one_turn}`, `AgentEvent`, `Provider`, `Config`, `select_provider`.
- Produces: `background::BgOutcome { final_text, tool_calls, denied, events }`; `background::run_background(provider, model, max_tokens, temperature, root, task) -> anyhow::Result<BgOutcome>`; `lib::run_background(cfg: Config, task: String) -> anyhow::Result<()>`.

- [ ] **Step 1: Write `src/background.rs`**

```rust
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

/// Run one task to completion on the CURRENT thread, then drain events into a
/// BgOutcome. Same-thread + post-turn drain keeps it deterministic (no interleave).
pub fn run_background(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    task: String,
) -> anyhow::Result<BgOutcome> {
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root);
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(task, &tx);
    drop(tx); // close the sender so the drain below terminates

    let mut out = BgOutcome::default();
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
    Ok(out)
}
```

> Note: `final_text` is assembled from `StreamDelta`s. If the scaffold provider returns the assembled Message rather than streaming deltas, `final_text` may be empty and the assistant text arrives via a non-delta path — Task 3's test asserts on `tool_calls`/`denied`/disk rather than requiring `final_text`, so this is robust either way. If `AgentEvent` carries the final assistant text through another variant in this codebase, also capture it; inspect `handle_agent` in `src/tui/run.rs` for how the TUI assembles assistant text and mirror it.

- [ ] **Step 2: Wire `lib.rs`**

Add `pub mod background;` to the module list in `src/lib.rs`, and re-export near the other `pub use`:

```rust
pub use background::{BgOutcome, run_background as run_background_inner};
```

Add the public wrapper (after `pub fn run(...)`):

```rust
/// Headless Background Agent entry (ADR 0026): pick the provider, run one task,
/// print a report to stdout, persist the session. Scheduling is external.
pub fn run_background(cfg: Config, task: String) -> anyhow::Result<()> {
    let provider = select_provider(&cfg);
    let outcome = background::run_background(
        provider,
        cfg.model.clone(),
        cfg.max_tokens,
        cfg.temperature,
        cfg.root.clone(),
        task,
    )?;
    println!("=== background agent result ===");
    if !outcome.final_text.trim().is_empty() {
        println!("{}", outcome.final_text.trim());
    }
    if !outcome.tool_calls.is_empty() {
        println!("tools executed: {}", outcome.tool_calls.join(", "));
    }
    if !outcome.denied.is_empty() {
        println!("denied/errors: {}", outcome.denied.join(" | "));
    }
    println!("=== summary: {} tools, {} denied ===", outcome.tool_calls.len(), outcome.denied.len());
    Ok(())
}
```

- [ ] **Step 3: Dispatch in `main.rs`**

Replace `src/main.rs` with:

```rust
// CodeCoder — autonomous AI agent. Entry shim; wiring lives in lib.rs.
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    // Background Agent (ADR 0026): CODECODER_BG_TASK=<task> runs headless, no TUI.
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    codecoder::run(cfg)
}
```

- [ ] **Step 4: Build + run the suite**

Run: `cargo build && cargo test 2>&1 | tail -6`
Expected: compiles; suite green (no behavior change to existing paths).

- [ ] **Step 5: Manual smoke (no key → StubClient, deterministic)**

Run: `CODECODER_BG_TASK="say hello" CODECODER_ROOT=$(mktemp -d) cargo run 2>&1 | tail -8`
Expected: prints `=== background agent result ===` … `=== summary: 0 tools, 0 denied ===` and exits 0 (StubClient returns one text reply, no tools). No TUI is entered.

- [ ] **Step 6: Commit**

```bash
git add src/background.rs src/lib.rs src/main.rs
git commit -m "feat(background): headless one-shot runner + CODECODER_BG_TASK dispatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: L1 black-box tests + docs

**Files:**
- Create: `tests/l1_background.rs`
- Modify: `docs/adr/0026-background-agent-headless-runner.md` (new), `CONTEXT.md`, `README.md`, `ARCHITECTURE.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: `codecoder::{run_background... }` — but tests call the inner runner directly for observability. Use `testkit::{ScriptedProvider, Workspace, assistant_text, assistant_tool_call}` and `codecoder::background::run_background`.

- [ ] **Step 1: Write the three black-box tests**

Create `tests/l1_background.rs`:

```rust
// L1 — Background Agent (ADR 0026). Black-box: drive the headless runner with a
// ScriptedProvider and assert on the returned BgOutcome + disk side effects.
mod testkit;
use testkit::*;
use serde_json::json;
use std::sync::Arc;

#[test]
fn background_runs_task_and_reports_tool_use() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    ws.write("data.txt", "PAYLOAD_BG");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "read_file", json!({"path": "data.txt"})),
        assistant_text("I read PAYLOAD_BG"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "read data.txt".into(),
    ).expect("bg run");
    assert!(out.tool_calls.iter().any(|t| t == "read_file"), "read_file should run: {:?}", out.tool_calls);
    // A session was persisted (ADR 0004).
    assert!(ws.root().join("sessions").read_dir().map(|mut d| d.next().is_some()).unwrap_or(false),
        "a session file must be written");
}

#[test]
fn background_denies_unauthorized_write() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "write_file", json!({"path": "hacked.txt", "content": "x"})),
        assistant_text("tried to write"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "write a file".into(),
    ).expect("bg run");
    // Core safety property: unauthorized Ask-tool is denied, no file, turn still completes.
    assert!(!ws.exists("hacked.txt"), "unauthorized write must be denied");
    assert!(out.denied.iter().any(|d| d.contains("write_file")), "denial must be recorded: {:?}", out.denied);
}

#[test]
fn background_allows_preauthorized_write() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // Pre-authorize write_file in the project allowlist (as a prior session would).
    ws.write("codecoder.json", "{\"allowlist\":[\"write_file\"]}");
    let (p, _rec) = ScriptedProvider::new(vec![
        assistant_tool_call("c1", "write_file", json!({"path": "ok.txt", "content": "WROTE_BG"})),
        assistant_text("wrote it"),
    ]);
    let out = codecoder::background::run_background(
        p as Arc<dyn codecoder::Provider>,
        "test-model".into(), 4096, 0.0, ws.root(), "write a file".into(),
    ).expect("bg run");
    assert!(ws.exists("ok.txt"), "pre-authorized write must succeed");
    assert_eq!(ws.read("ok.txt"), "WROTE_BG");
    assert!(!out.denied.iter().any(|d| d.contains("write_file")), "authorized write must not be denied");
}
```

> `codecoder::background::run_background` must be reachable: ensure `pub mod background;` in lib.rs (Task 2 Step 2) and that `BgOutcome`'s fields are `pub`. `ScriptedProvider::new` returns `(Arc<ScriptedProvider>, Recorder)`; the `as Arc<dyn codecoder::Provider>` cast adapts it. If the cast fails to infer, bind `let p: Arc<dyn codecoder::Provider> = p;` first.

- [ ] **Step 2: Run the background tests**

Run: `cargo test --test l1_background 2>&1 | tail -15`
Expected: all three PASS. If `background_denies_unauthorized_write` hangs, the headless gate (Task 1 Step 5) is not short-circuiting — that's a real integration bug; report it, don't weaken the test.

- [ ] **Step 3: Full suite**

Run: `cargo test 2>&1 | tail -6`
Expected: whole suite green.

- [ ] **Step 4: Write ADR 0026**

Create `docs/adr/0026-background-agent-headless-runner.md`:

```markdown
# Background Agent: headless one-shot runner

A Background Agent is a full-LLM-loop agent that runs autonomously with **no user
present** (CONTEXT.md). v1 ships the minimal shape: a **headless one-shot runner**
triggered by `CODECODER_BG_TASK=<task>`, which drives exactly one turn and exits.
Scheduling is external (cron/CI).

## Permission model (the "no user present" problem)

Only the top-level interactive agent owns a user-facing channel (see
[[0016-channel-topology-and-event-model]]); a Background Agent has none, so a
permission prompt would have no one to answer it. Rather than queue prompts, the
headless gate resolves them at authorization time:

- An `Ask { key }` tool runs **only if `key` is already in the session or the
  persisted project allowlist** (`codecoder.json`, see [[0005-permission-scope-and-session-allowlist]]).
- Otherwise it is **auto-denied** — an error `ToolResult`, never a blocking prompt.
- `ask_user` / `confirm` / `plan` (which need a user) short-circuit to a denial.

The user pre-authorizes by editing `codecoder.json` before launch. This turns
"who answers the prompt?" into "what was authorized up front?", eliminating the
runtime responder.

## Not a sub-agent

Unlike a Sub-agent ([[0019-sub-agent-capability-boundary]], read-only, user
present, synchronously awaited), a Background Agent has the **full builtin
toolbox** and may write/run — bounded only by the pre-authorized allowlist. It is
`headless`, a boolean on `AgentLoop` that only alters the unauthorized-Ask branch
and the interactive-tool intercepts; interactive behavior is unchanged.

## Deferred (named hard problems, not in v1)

SIGINT/cancel wiring, a built-in scheduler, and multi-runner resource limits are
out of scope; the external scheduler bounds concurrency.
```

- [ ] **Step 5: Update CONTEXT.md + docs/counts**

In `CONTEXT.md`, the **Background Agent** entry: change "**Post-v1 (named concept, not yet built)**: no runner exists in v1 …" to reflect the shipped headless runner. Replace that sentence with:

```markdown
**v1 ships a headless one-shot runner** (`CODECODER_BG_TASK=<task>`, see [[0026-background-agent-headless-runner]]): a full-loop agent runs one task with no user present, using pre-authorized `codecoder.json` permissions (any un-authorized Ask-tool is auto-denied, never prompted). Scheduling is external; SIGINT/scheduler/multi-runner limits remain deferred.
```

In `CLAUDE.md`: remove Background Agent from the "已知未实现" list (it is now built); note the runner. Update module count (23 → 24, `background.rs` added) and test count.

In `ARCHITECTURE.md`: add a `background.rs` row to the module map + update the test count.

In `README.md`: update the test count and mention `CODECODER_BG_TASK` in the env-var section.

Compute exact counts first: `cargo test 2>&1 | grep -E "test result:" | awk '{p+=$4; i+=$8} END {print p" passed, "i" ignored"}'` — then edit docs to match (do not assume).

- [ ] **Step 6: Commit**

```bash
git add tests/l1_background.rs docs/adr/0026-background-agent-headless-runner.md CONTEXT.md README.md ARCHITECTURE.md CLAUDE.md
git commit -m "test(l1)+docs: Background Agent coverage; ADR 0026; mark runner shipped

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- §4 permission model (allowlist-or-Deny, no prompt) → Task 1 Steps 5-6 + Task 3 test 2/3 ✅
- §5.1 `BgOutcome` + `run_background` → Task 2 Step 1 ✅
- §5.2 `headless` field + gate + `new_background`/`run_one_turn` → Task 1 ✅
- §5.3 lib wrapper → Task 2 Step 2 ✅; §5.4 main dispatch → Task 2 Step 3 ✅
- §6 report to stdout + session persist → Task 2 Step 2 (println) + persist inherited (`persist=true` in `new_background`) ✅
- §7 degrade (StubClient smoke) → Task 2 Step 5 ✅; sub-agent still allowed → Task 1 Step 6 note (agent tool not guarded) ✅
- §8 tests → Task 1 unit + Task 3 three L1 ✅
- §9 docs (CONTEXT/ADR/counts) → Task 3 Steps 4-5 ✅

**2. Placeholder scan:** none — all code steps have complete code. The one soft spot (`final_text` assembly depends on whether the scaffold streams deltas) is explicitly de-risked: Task 3 tests assert on `tool_calls`/`denied`/disk, never requiring `final_text`.

**3. Type consistency:** `new_background(provider, model: impl Into<String>, max_tokens, temperature, root)`, `run_one_turn(&mut self, task: String, &Sender<AgentEvent>)`, `build(..., toolbox, persist, headless)`, `BgOutcome { final_text, tool_calls, denied, events }`, `run_background(provider, model: String, max_tokens, temperature, root, task) -> Result<BgOutcome>` — consistent across tasks. Note `new_background` takes `impl Into<String>` (matches `new`) while the free `run_background` takes `String` (matches call site passing `.into()`/owned) — intentional and consistent with each caller.
