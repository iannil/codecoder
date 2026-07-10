# Compaction tier-2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement ADR 0023 tier-2 — when tier-1 leaves the context still over the model-window threshold, summarize the oldest dialogue span (`[anchor+1 .. last_user]`) into one synthetic `System` message via a single cached LLM call, without ever mutating the persisted Session.

**Architecture:** `compaction::working_set` stays a pure tier-1 function. New pure helpers (`summary_span`, `render_span`, `apply_tier2`) do the span math and rewrite. `AgentLoop` owns the orchestration (`context_working_set`) + provider call (`summarize_span`) + an in-memory cache keyed by the covered span's last message id, so at most one summary call fires per turn. Summary failure degrades silently to tier-1.

**Tech Stack:** Rust; existing `Provider`/`CompletionRequest`; `tiktoken` via `tokenizer::count_tokens`; `serde` messages.

## Global Constraints

- Compaction only shapes the derived working set; **never mutate the persisted Session** (ADR 0023). On-disk `messages` stay full-fidelity.
- tier-1 trigger measures the **full** history; tier-2's second check measures the **tier-1 result** (never fed back into the tier-1 trigger — no oscillation).
- **At most one summary LLM call per turn** (cache keyed by covered-span last message id; stable within a turn because tools append non-`User` messages).
- Summary failure / empty text → **silent degrade to tier-1**, do not cache, do not crash the turn.
- `apply_tier2` slices by **message id**, not raw index (tier-1 may drop Reasoning-only messages and shift indices).
- Message ids are strictly increasing with position (`next_id++` per append), so id order == position order.
- Default `cargo test` suite stays hermetic + green; `compaction.rs` tier-1 tests must not regress.
- Commit messages end with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

## File Structure

- `src/compaction.rs` — add pure helpers `summary_span`, `render_span`, `apply_tier2` (+ unit tests). tier-1 unchanged.
- `src/agent.rs` — add `Tier2Summary` struct + `tier2` field; `context_working_set` + `summarize_span` methods; replace the `working_set` call site; clear cache on `Clear`/`Resume`.
- `tests/l1_compaction.rs` — add tier-2 black-box test + degradation test.
- `docs/adr/0023-context-compaction.md`, `ARCHITECTURE.md`, `CLAUDE.md`, `README.md` — status + counts.

Known types (verified): `MessageId = u64`; `Message::text(id, role, text)`, `Message::new(id, role, items)`, `Message { id, role, items }`; `MessageItem::{Text{text}, Reasoning{text}, ToolCall{id,name,args}, ToolResult{call_id,output,is_error}}`; `Role::{User,Assistant,System,Tool}`; `CompletionRequest { model, messages, max_tokens, temperature, tools }`; `Provider::complete(&self, &CompletionRequest) -> anyhow::Result<Message>`; `compaction::{COMPACTION_THRESHOLD, RECENT_TAIL, should_compact, working_set}`; `tokenizer::count_tokens(model, &[Message]) -> u64`.

---

### Task 1: Pure tier-2 helpers in `compaction.rs`

**Files:**
- Modify: `src/compaction.rs` (add helpers + unit tests; leave tier-1 untouched)

**Interfaces:**
- Produces:
  - `pub fn summary_span(messages: &[Message]) -> Option<(usize, usize)>` — `(anchor+1, last_user_idx)`; `None` if no summarizable middle.
  - `pub fn render_span(span: &[Message]) -> String` — bounded plain-text rendering of the span for the summary prompt.
  - `pub fn apply_tier2(tier1: &[Message], anchor_id: MessageId, covered_last_id: MessageId, summary: &str) -> Vec<Message>` — replace the covered id-range with one `System` summary inserted after the anchor.

- [ ] **Step 1: Write failing unit tests**

Append to the `#[cfg(test)] mod tests` in `src/compaction.rs`:

```rust
    #[test]
    fn summary_span_selects_between_first_and_last_user() {
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),      // anchor
            msg(1, Role::Assistant, vec![MessageItem::Text { text: "a".into() }]),
            msg(2, Role::User, vec![MessageItem::Text { text: "mid".into() }]),
            msg(3, Role::Assistant, vec![MessageItem::Text { text: "b".into() }]),
            msg(4, Role::User, vec![MessageItem::Text { text: "current".into() }]),   // last user
        ];
        assert_eq!(summary_span(&msgs), Some((1, 4)));
    }

    #[test]
    fn summary_span_none_when_single_turn() {
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),
            msg(1, Role::Assistant, vec![MessageItem::Text { text: "a".into() }]),
        ];
        assert_eq!(summary_span(&msgs), None); // only one user → nothing older to summarize
    }

    #[test]
    fn summary_span_none_when_users_adjacent() {
        // anchor at 0, last user at 1 → span (1,1) is empty → None.
        let msgs = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),
            msg(1, Role::User, vec![MessageItem::Text { text: "again".into() }]),
        ];
        assert_eq!(summary_span(&msgs), None);
    }

    #[test]
    fn apply_tier2_replaces_covered_range_by_id_and_keeps_anchor_and_tail() {
        // tier1 with a Reasoning-only message ALREADY dropped (id 2 missing) to prove
        // apply_tier2 works by id, not raw index.
        let tier1 = vec![
            msg(0, Role::User, vec![MessageItem::Text { text: "goal".into() }]),          // anchor
            msg(1, Role::Assistant, vec![MessageItem::Text { text: "OLD".into() }]),       // covered
            // id 2 (Reasoning-only) dropped by tier-1
            msg(3, Role::Tool, vec![MessageItem::ToolResult { call_id: "c".into(), output: "[elided]".into(), is_error: false }]), // covered
            msg(4, Role::User, vec![MessageItem::Text { text: "current".into() }]),        // tail
            msg(5, Role::Assistant, vec![MessageItem::Text { text: "reply".into() }]),     // tail
        ];
        let out = apply_tier2(&tier1, 0, 3, "SUMMARY");
        // anchor kept
        assert!(matches!(&out[0].items[0], MessageItem::Text { text } if text == "goal"));
        // summary inserted right after anchor
        assert!(matches!(&out[1], Message { role: Role::System, .. }));
        assert!(matches!(&out[1].items[0], MessageItem::Text { text } if text.contains("SUMMARY")));
        // covered ids (1, 3) gone
        assert!(!out.iter().any(|m| m.id == 1 || m.id == 3));
        // tail preserved
        assert!(out.iter().any(|m| m.id == 4));
        assert!(out.iter().any(|m| m.id == 5));
    }

    #[test]
    fn render_span_drops_reasoning_and_truncates_tool_results() {
        let span = vec![
            msg(1, Role::Assistant, vec![
                MessageItem::Text { text: "hello".into() },
                MessageItem::Reasoning { text: "SECRET".into() },
            ]),
            msg(2, Role::Tool, vec![MessageItem::ToolResult { call_id: "c".into(), output: "x".repeat(500), is_error: false }]),
        ];
        let s = render_span(&span);
        assert!(s.contains("hello"));
        assert!(!s.contains("SECRET"));           // reasoning omitted
        assert!(s.len() < 400);                   // tool result truncated, not 500 chars
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib compaction 2>&1 | tail -15`
Expected: FAIL — `summary_span`/`render_span`/`apply_tier2` not found.

- [ ] **Step 3: Implement the helpers**

Add to `src/compaction.rs` (after `working_set`, before `#[cfg(test)]`). Add `MessageId` to the import:

```rust
use crate::message::{Message, MessageId, MessageItem, Role};
```

```rust
/// The oldest summarizable dialogue span: everything between the anchor (first
/// user message = original goal) and the current turn (last user message). Returns
/// half-open indices `[anchor+1, last_user)`, or `None` when there is no earlier
/// turn to summarize (only one user message, or the two users are adjacent).
pub fn summary_span(messages: &[Message]) -> Option<(usize, usize)> {
    let anchor = messages.iter().position(|m| m.role == Role::User)?;
    let last_user = messages.iter().rposition(|m| m.role == Role::User)?;
    let start = anchor + 1;
    if start >= last_user {
        return None;
    }
    Some((start, last_user))
}

/// Render a span into bounded plain text for the summary prompt: drop `Reasoning`,
/// mark `ToolCall`s, and truncate each `ToolResult` body so a huge old span cannot
/// blow the summary call's own token budget.
pub fn render_span(span: &[Message]) -> String {
    fn role_str(r: Role) -> &'static str {
        match r {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        }
    }
    let mut s = String::new();
    for m in span {
        let role = role_str(m.role);
        for it in &m.items {
            match it {
                MessageItem::Text { text } => {
                    s.push_str(role);
                    s.push_str(": ");
                    s.push_str(text);
                    s.push('\n');
                }
                MessageItem::Reasoning { .. } => {}
                MessageItem::ToolCall { name, .. } => {
                    s.push_str(&format!("{role}: [tool_call {name}]\n"));
                }
                MessageItem::ToolResult { output, .. } => {
                    let snippet: String = output.chars().take(200).collect();
                    s.push_str(&format!("tool: [result: {snippet}]\n"));
                }
            }
        }
    }
    s
}

/// Rewrite the tier-1 result: drop every message whose id is in
/// `(anchor_id, covered_last_id]` and insert one synthetic `System` summary right
/// after the anchor. Works by **id** because tier-1 may have dropped Reasoning-only
/// messages, so raw indices no longer line up with the original history.
pub fn apply_tier2(
    tier1: &[Message],
    anchor_id: MessageId,
    covered_last_id: MessageId,
    summary: &str,
) -> Vec<Message> {
    let mut out = Vec::with_capacity(tier1.len());
    let mut inserted = false;
    for m in tier1 {
        if m.id > anchor_id && m.id <= covered_last_id {
            if !inserted {
                out.push(Message::text(
                    MessageId::MAX,
                    Role::System,
                    format!("先前对话摘要：\n{summary}"),
                ));
                inserted = true;
            }
            continue; // covered span replaced by the summary
        }
        out.push(m.clone());
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib compaction 2>&1 | tail -15`
Expected: PASS — all tier-2 helper tests + the existing tier-1 tests green.

- [ ] **Step 5: Commit**

```bash
git add src/compaction.rs
git commit -m "feat(compaction): pure tier-2 helpers (summary_span, render_span, apply_tier2)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Wire tier-2 orchestration into `AgentLoop`

**Files:**
- Modify: `src/agent.rs` (struct field, build init, two methods, call site, cache clears)

**Interfaces:**
- Consumes: `compaction::{should_compact, working_set, summary_span, render_span, apply_tier2}`, `tokenizer::count_tokens`, `CompletionRequest`.
- Produces: `AgentLoop::context_working_set(&mut self, event_tx: &Sender<AgentEvent>) -> Vec<Message>` (tier-1 + tier-2 with caching + degrade).

- [ ] **Step 1: Add the cache struct + field**

In `src/agent.rs`, add the struct near the top-level types (e.g. after `CancelToken`):

```rust
/// In-memory tier-2 summary cache (ADR 0023). Keyed by the covered span's last
/// message id: stable within a turn (tools append non-User messages), so at most
/// one summary LLM call fires per turn. Not persisted — recomputed after /resume.
struct Tier2Summary {
    covered_last_id: MessageId,
    text: String,
}
```

Add the field to `pub struct AgentLoop` (after `cancel: CancelToken,`):

```rust
    /// Derived tier-2 summary overlay (ADR 0023); never persisted.
    tier2: Option<Tier2Summary>,
```

In `fn build(...)`'s struct literal (after `cancel: CancelToken::default(),`):

```rust
            tier2: None,
```

Ensure `MessageId` is imported in `agent.rs` (it comes via `crate::message::…`; add to the existing `use crate::message::{...}` if absent).

- [ ] **Step 2: Add `summarize_span` + `context_working_set` methods**

Add inside an `impl AgentLoop { ... }` block (near `process_turn`):

```rust
    /// One-shot LLM summary of a rendered span (ADR 0023 tier-2). Provider-neutral
    /// request with no tools; returns Err on transport failure or empty output.
    fn summarize_span(&self, rendered: &str) -> anyhow::Result<String> {
        let system = "You are compacting an agent's conversation history. Summarize the \
            following earlier messages into a concise brief that preserves the task/goals, \
            decisions made, key facts and file paths, tool outcomes, and open threads. Omit \
            chit-chat. Output plain prose, no preamble.";
        let req = CompletionRequest {
            model: self.model.clone(),
            messages: vec![
                Message::text(0, Role::System, system),
                Message::text(1, Role::User, rendered.to_string()),
            ],
            max_tokens: 1024,
            temperature: 0.0,
            tools: vec![],
        };
        let reply = self.provider.complete(&req)?;
        let text: String = reply
            .items
            .iter()
            .filter_map(|it| match it {
                MessageItem::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            anyhow::bail!("empty summary");
        }
        Ok(text)
    }

    /// Derive the Context Working Set: tier-1 always; tier-2 (summarize the oldest
    /// span) only when tier-1 is still over the window threshold. Degrades to tier-1
    /// if the summary call fails. Caches the summary in-memory (one call per turn).
    fn context_working_set(&mut self, _event_tx: &Sender<AgentEvent>) -> Vec<Message> {
        let tier1 = compaction::working_set(&self.model, &self.session.messages, self.model_window);
        if !compaction::should_compact(
            crate::tokenizer::count_tokens(&self.model, &tier1),
            self.model_window,
        ) {
            return tier1;
        }
        let Some((start, end)) = compaction::summary_span(&self.session.messages) else {
            return tier1;
        };
        let anchor_id = self.session.messages[start - 1].id;
        let covered_last_id = self.session.messages[end - 1].id;

        // Reuse cache if it still covers the same span; else summarize once.
        let cached = self
            .tier2
            .as_ref()
            .filter(|s| s.covered_last_id == covered_last_id)
            .map(|s| s.text.clone());
        let text = match cached {
            Some(t) => t,
            None => {
                let rendered = compaction::render_span(&self.session.messages[start..end]);
                match self.summarize_span(&rendered) {
                    Ok(t) => {
                        self.tier2 = Some(Tier2Summary { covered_last_id, text: t.clone() });
                        t
                    }
                    Err(_) => return tier1, // graceful degrade
                }
            }
        };
        compaction::apply_tier2(&tier1, anchor_id, covered_last_id, &text)
    }
```

- [ ] **Step 3: Replace the call site**

In `process_turn`, change the tier-1-only line:

```rust
            let working = compaction::working_set(&self.model, &self.session.messages, self.model_window);
```

to:

```rust
            let working = self.context_working_set(event_tx);
```

- [ ] **Step 4: Clear the cache on history replacement**

In the `AgentCommand::Clear` arm, after `self.session.messages.clear();` add:

```rust
                    self.tier2 = None;
```

In `resume_latest`, after `self.session = session;` add:

```rust
        self.tier2 = None;
```

- [ ] **Step 5: Build + run the existing suite (zero regression)**

Run: `cargo build && cargo test 2>&1 | tail -6`
Expected: compiles; existing tests all pass (tier-1 behavior unchanged since tier-2 only activates when tier-1 is still over threshold and there is an older span).

- [ ] **Step 6: Commit**

```bash
git add src/agent.rs
git commit -m "feat(compaction): tier-2 orchestration in AgentLoop (cached summary + degrade)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: L1 black-box tests + docs

**Files:**
- Modify: `tests/l1_compaction.rs` (add two tests)
- Modify: `docs/adr/0023-context-compaction.md`, `ARCHITECTURE.md`, `CLAUDE.md`, `README.md`

**Interfaces:**
- Consumes: `testkit::{ScriptedProvider, Workspace, run_steps, Step, PermPolicy, assistant_text}`.

- [ ] **Step 1: Write the tier-2 black-box test**

Append to `tests/l1_compaction.rs` (it already has `mod testkit; use testkit::*;` and a `dump`-style helper is not needed — assert via `format!("{:?}", ...)` on recorded messages, consistent with the file's existing style):

```rust
#[test]
fn tier2_summarizes_oldest_span_into_a_system_message() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    // ~40k tokens each; 3 in history (~120k) exceeds the 96k threshold (0.75 * 128k
    // default window for "test-model") even after tier-1 (Text is not elided).
    let big = "LOREM ".repeat(20_000);
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_text(&format!("{big} OLD_TURN_MARK")), // turn 1 reply (in the middle)
        assistant_text(&big),                             // turn 2 reply
        assistant_text(&big),                             // turn 3 reply
        assistant_text("SUMMARY_MARK a concise summary"), // turn 4: the summary call
        assistant_text("final reply"),                    // turn 4: the actual turn reply
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![
            Step::Msg("ORIGINAL_GOAL".into()),
            Step::Msg("t2".into()),
            Step::Msg("t3".into()),
            Step::Msg("current CURRENT_MARK".into()),
        ],
        PermPolicy::GrantOnce,
    );

    // The summary request (issued inside context_working_set) must have seen the old span.
    let summary_req = out
        .requests
        .iter()
        .find(|r| format!("{:?}", r.messages).contains("OLD_TURN_MARK"))
        .expect("a summary request should render the old span");
    assert!(
        format!("{:?}", summary_req.messages).contains("compacting an agent's conversation"),
        "the request seeing OLD_TURN_MARK should be the summary call"
    );

    // The final actual-turn request replaces the middle with the summary System message.
    let last = out.requests.last().expect("at least one request");
    let dump = format!("{:?}", last.messages);
    assert!(dump.contains("SUMMARY_MARK"), "summary must be injected into the turn context");
    assert!(dump.contains("ORIGINAL_GOAL"), "anchor (first goal) must survive");
    assert!(!dump.contains("OLD_TURN_MARK"), "the summarized middle must be gone from the turn request");
}
```

- [ ] **Step 2: Run it**

Run: `cargo test --test l1_compaction tier2_summarizes 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 3: Write the degradation test**

```rust
#[test]
fn tier2_degrades_to_tier1_when_summary_is_empty() {
    let ws = Workspace::new();
    ws.write("AGENTS.md", "x");
    let big = "LOREM ".repeat(20_000);
    let (p, rec) = ScriptedProvider::new(vec![
        assistant_text(&format!("{big} OLD_TURN_MARK")),
        assistant_text(&big),
        assistant_text(&big),
        assistant_text(""),            // turn 4: summary call returns EMPTY → degrade
        assistant_text("final reply"), // turn 4: actual reply
    ]);
    let out = run_steps(
        ws.root(),
        p,
        rec,
        vec![
            Step::Msg("ORIGINAL_GOAL".into()),
            Step::Msg("t2".into()),
            Step::Msg("t3".into()),
            Step::Msg("current CURRENT_MARK".into()),
        ],
        PermPolicy::GrantOnce,
    );

    // Empty summary → no System summary injected; the turn still completes on tier-1.
    let last = out.requests.last().expect("at least one request");
    let dump = format!("{:?}", last.messages);
    assert!(!dump.contains("先前对话摘要"), "empty summary must not be injected");
    assert!(
        out.events.iter().any(|e| matches!(e, codecoder::AgentEvent::TurnComplete)),
        "the turn must still complete under graceful degrade"
    );
}
```

- [ ] **Step 4: Run both + full suite**

Run: `cargo test --test l1_compaction 2>&1 | tail -12`
Expected: all l1_compaction tests PASS.
Run: `cargo test 2>&1 | tail -6`
Expected: whole suite green.

- [ ] **Step 5: Update docs + ADR status**

In `docs/adr/0023-context-compaction.md`, change the status line from tier-2 deferred to implemented:

```markdown
**Status**: Accepted; **tier 1 and tier 2 implemented**. `compaction.rs::working_set` applies tier 1 (drop `Reasoning`, elide old `ToolResult` bodies); when the tier-1 result is still over threshold, `AgentLoop::context_working_set` applies tier 2 — one cached LLM call summarizes the oldest span (`[anchor+1 .. last_user]`) into a synthetic `System` message. The persisted Session stays full-fidelity.
```

In `CLAUDE.md`, update the "已知未实现" bullet: tier-2 is now implemented (remove it from the not-implemented list or mark it done). Update the test count (add the 5 new tests from Tasks 1 & 3: 3 unit + 2 L1 → new total). In `ARCHITECTURE.md` update the `compaction.rs` row + test count. In `README.md` update the test count. Run `cargo test 2>&1 | grep -E "test result:" | awk '{p+=$4; i+=$8} END {print p" passed, "i" ignored"}'` to get the exact numbers first, then edit the docs to match.

- [ ] **Step 6: Commit**

```bash
git add tests/l1_compaction.rs docs/adr/0023-context-compaction.md ARCHITECTURE.md CLAUDE.md README.md
git commit -m "test(l1)+docs: tier-2 black-box coverage; mark ADR 0023 tier-2 implemented

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- §3.1 two-level trigger → Task 2 `context_working_set` (tier-1 then re-measure) ✅
- §3.2 span `[anchor+1 .. last_user)` + turn-stable cache → Task 1 `summary_span` + Task 2 cache-by-`covered_last_id` ✅
- §3.3 `[anchor, System(summary), tail]` → Task 1 `apply_tier2` ✅
- §3.4 empty-span degrade → `summary_span` `None` path ✅
- §4.1 pure helpers → Task 1 ✅; §4.2 orchestration + `summarize_span` → Task 2 ✅; §4.3 cache clear on Clear/Resume → Task 2 Step 4 ✅
- §5 correlation safety (contiguous span at User boundaries) → inherent in `apply_tier2` id-range drop ✅
- §6 graceful degrade → Task 2 `Err(_) => return tier1` ✅
- §7 id-alignment → Task 1 `apply_tier2` by id + test with a dropped Reasoning id ✅
- §8 tests → Task 1 unit + Task 3 L1 (tier-2 + degrade + persisted-fidelity is covered by the existing `compaction_does_not_mutate_persisted_session` test, unaffected) ✅
- §9 docs/ADR → Task 3 Step 5 ✅

Note on the spec's "先经 tier-1 处理 then render": implemented equivalently by `render_span` doing its own bounded elision (drop Reasoning, truncate ToolResult) — same intent (bounded summary input), simpler than mapping the span onto the tier-1 result. Documented in Task 1.

**2. Placeholder scan:** none — every code step has complete code; doc step gives the exact command to compute counts before editing.

**3. Type consistency:** `summary_span -> Option<(usize,usize)>`, `render_span(&[Message]) -> String`, `apply_tier2(&[Message], MessageId, MessageId, &str) -> Vec<Message>`, `context_working_set(&mut self, &Sender<AgentEvent>) -> Vec<Message>`, `Tier2Summary { covered_last_id: MessageId, text: String }` — consistent across tasks. `covered_last_id` used identically in cache key and struct.
