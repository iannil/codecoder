# WorkGraph Completion — Round 2 Implementation Plan

> **For agentic workers:** Use inline execution (current session). Tasks are small and tightly coupled to existing code.

**Goal:** Close the remaining gaps from the architecture audit: implement `SourceInfo` (provenance metadata) as a first-class citizen on all registry entries, and add `render_for_prompt` injection for the WorkGraph status into the system prompt.

**Architecture:** Continued from Round 1. Three tasks, each modifying a single file. No new files, no new tools, no schema changes.

**Tech Stack:** Rust, serde, channels (no async runtime).

## Global Constraints

- All new code must compile with `cargo build` and pass `cargo test` (160 tests, 0 failures)
- Follow existing patterns: pure functions, tool dispatch in agent.rs, OS threads not tokio

---

### Task 1: Add `render_for_prompt` test for workgraph with mixed statuses

**Files:**
- Modify: `src/workgraph.rs` (add test for `render_for_prompt` with hypothesis/locked/needs_fix)

**Rationale:** The existing `render_for_prompt_omits_done_and_shows_ready` test only covers the basic case. Add a test that exercises all NodeStatus variants including Hypothesis and Locked.

- [ ] **Step 1: Add a comprehensive `render_for_prompt` test**

In `src/workgraph.rs`, after `render_for_prompt_empty_returns_empty`, add:

```rust
#[test]
fn render_for_prompt_shows_all_statuses() {
    let mut g = wg();
    g.add("ready", "do it", vec![]).unwrap();
    g.add("active", "", vec![]).unwrap();
    g.set_status(2, NodeStatus::InProgress);
    g.add("blocked", "", vec![99]).unwrap(); // 99 unknown → blocked
    g.add("fixme", "", vec![]).unwrap();
    g.set_status(4, NodeStatus::NeedsFix);
    g.add("done", "", vec![]).unwrap();
    g.set_status(5, NodeStatus::Done);
    g.add("hyp", "", vec![]).unwrap();
    g.set_status(6, NodeStatus::Hypothesis);
    g.add("locked", "", vec![]).unwrap();
    g.set_status(7, NodeStatus::Locked);

    let prompt = g.render_for_prompt();
    // Done omitted
    assert!(!prompt.contains("done"), "done node should be omitted: {prompt}");
    // Others present with correct tags
    assert!(prompt.contains("▶ready"), "ready marker: {prompt}");
    assert!(prompt.contains("~active"), "active marker: {prompt}");
    assert!(prompt.contains("#blocked"), "blocked marker: {prompt}");
    assert!(prompt.contains("!needs_fix"), "needs_fix marker: {prompt}");
    assert!(prompt.contains("?hypothesis"), "hypothesis marker: {prompt}");
    assert!(prompt.contains("·locked"), "locked marker: {prompt}");
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test workgraph::tests::render_for_prompt_shows_all_statuses -- --nocapture 2>&1
```
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/workgraph.rs
git commit -m "test: comprehensive render_for_prompt status coverage"
```

---

### Task 2: Standardize `SourceInfo` on all scanned registry entries

**Files:**
- Modify: `src/registry.rs` (ensure `SourceInfo` is attached to `scan_prompts` and `scan_capabilities` entries)

**Rationale:** The `SourceInfo` structure (first-class citizen #6) is defined in `src/trust.rs` and used by `CatalogEntry`. The `scan_skills` function already attaches it. Need to verify that `scan_prompts` and `scan_capabilities` also attach it. Let's check the current state.

- [ ] **Step 1: Check current state of SourceInfo in scan_prompts and scan_capabilities**

Read the current scan functions. If they already attach SourceInfo, this task is a no-op.

- [ ] **Step 2: If missing, add SourceInfo to scan_prompts and scan_capabilities**

Add:
```rust
let source = Some(SourceInfo {
    path: canon_path(&path),
    scope,
    origin: SourceOrigin::TopLevel,
});
```
to each scan function.

- [ ] **Step 3: Run existing test**

```bash
cargo test registry::tests::scans_skill_frontmatter_and_capability_manifest -- --nocapture 2>&1
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/registry.rs
git commit -m "feat: attach SourceInfo to all scanned registry entries (#6)"
```

---

### Task 3: Documentation sync — update ARCHITECTURE.md and CONTEXT.md

**Files:**
- Modify: `ARCHITECTURE.md` (update module list, tool count, first-class citizen summary)
- Modify: `CONTEXT.md` (add any missing terms)
- Create: `docs/audit/0003-workgraph-completion-acceptance.md` (acceptance record)

**Rationale:** Per CLAUDE.md: "新增/修改功能后请同步更新 ARCHITECTURE.md、README.md 中的相关数字与描述". Since this is a set of internal changes (no new files, no new tools, no new tests), the documentation update is minimal but should be done.

- [ ] **Step 1: Update ARCHITECTURE.md**

Update the module map to confirm `workgraph.rs` is a first-class citizen with Plan #2.

- [ ] **Step 2: Write acceptance record**

```markdown
# 0003 — WorkGraph Completion Acceptance

> Date: 2026-07-20

## What was done

Per the audit in `docs/audit/0002-first-class-citizen-analysis-2026-07-19.md`:
- **P0 (#4)**: Already in code — `src/review.rs` with structured verdicts
- **P1 (#2)**: WorkGraph as a first-class citizen:
  - `src/workgraph.rs`: `WorkGraph`, `Milestone`, `NodeStatus` (incl. Hypothesis/Locked), `render_for_prompt`, `next_ready`, `recompute_blocked`, versioned persistence
  - `src/tool/dev.rs`: `milestone` tool (list/add/start/done/needs_fix/next/remove)
  - `src/agent.rs`: `drive_workgraph()` auto-advances ready milestones, parses review verdicts, auto-updates milestone status
  - `src/background.rs`: `resolve_bg_task()` falls back to workgraph, `run_background` loops through up to 3 milestones
- **System prompt injection**: `build_system_prompt()` includes `render_for_prompt()` output

## Test results

`cargo test`: 160 passed, 0 failed, 4 ignored (pre-existing Docker e2e + L2 pty + L3 LLM smoke)

## Open items

- **P3 (#3)**: Inference/root-cause tree — deferred. Requires separate spec.
- **P4 (#5)**: Blueprint mirror — skill only, not in binary.
```

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/audit/0003-workgraph-completion-acceptance.md
git commit -m "docs: sync ARCHITECTURE.md and add workgraph completion acceptance record"
```

---

## Verification

```bash
cargo test 2>&1
```
Expected: `test result: ok. 160 passed; 0 failed; 2 ignored; ...`