# Workgraph Integrity Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three workgraph data-integrity/honesty bugs — headless empty-graph false success (#1), silent data loss on a corrupt/newer `workgraph.json` (#3), and `reason.rs` bypassing `with_lock` (#5).

**Architecture:** Add `WorkGraph::read_checked` (fallible read that distinguishes absent→empty from present-but-unparseable→Err). Wire it into the headless BG write paths, adding a new `MissionState::EmptyGraph` (exit code 5) for genuinely empty graphs and a backup-then-abort guard for corrupt files. Route `reason.rs::to_milestone`'s write through `with_lock`.

**Tech Stack:** Rust (std, `anyhow`, `serde_json`), `cargo test` with `StubClient` (no API key needed). Existing helpers: `WorkGraph::{read,load,save,with_lock,add}`, `bg_gate::MissionState`, `bg_ledger::mission_exit_code`, `bg_observer::BgObserver`.

## Global Constraints

- New exit code **5** is dedicated to `EmptyGraph`; existing codes stay 0 (CompletedAllReady/Running), 2 (BlockedAt/StuckNeedsFix), 3 (CircuitBreaker), 4 (Error).
- `read_checked` returns `Err` ONLY when `workgraph.json` physically exists but `load()` fails (corrupt JSON or `schema_version` > `WG_SCHEMA_VERSION`). Absent file / unreadable / todos-migration / empty-graph are all `Ok`.
- Keep `WorkGraph::read` (infallible) unchanged — display/probe paths (`render_for_prompt`, `next_ready` probes) still use it.
- The abort path MUST NOT `save` (no overwrite); it only `rename`s the bad file to `workgraph.json.corrupt.<pid>`.
- Backup filename uses `std::process::id()` (no timestamps — `Date::now` is restricted in this environment).
- Full `cargo test` must stay green (master is currently stable, deterministic).
- Do NOT implement auto-seeding (B), id-reuse (#4), or verdict-writeback unification (#6/#7).
- Every commit ends with the `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer. Work on branch `fix/workgraph-integrity`.

## File Structure

- `src/workgraph.rs` — ADD `read_checked`; keep `read`, `load`, `path` (already `pub`) as-is. (Task 1)
- `src/bg_gate.rs` — ADD `MissionState::EmptyGraph` variant. (Task 2)
- `src/bg_ledger.rs` — ADD `MissionState::EmptyGraph => 5` arm to `mission_exit_code`. (Task 2)
- `src/background.rs` — entry guard in `run_background_cfg` workgraph branch (#1 empty + #3 corrupt-abort-backup); swap `read`→`read_checked?` in `advance_one_milestone`/`retry_one_milestone`. (Task 2)
- `src/tool/reason.rs` — route `to_milestone` write through `with_lock`. (Task 3)
- `docs/adr/0033-bg-ledger-and-exit-codes.md`, `docs/superpowers/specs/2026-07-25-workgraph-initialization-analysis.md`, `CLAUDE.md`, `README.md` — docs. (Task 4)

---

### Task 1: `WorkGraph::read_checked` (#3 primitive)

**Files:**
- Modify: `src/workgraph.rs` (add method near `read`, ~line 117; add tests in the `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn read_checked(root: &std::path::Path) -> anyhow::Result<WorkGraph>` — `Ok(default/migrated)` when `workgraph.json` is absent/unreadable or a `todos.json` migrates; `Ok(loaded)` when it parses; `Err(_)` when `workgraph.json` exists and content is read but `load()` fails (corrupt or newer schema).

- [ ] **Step 1: Write the failing tests**

In `src/workgraph.rs` `mod tests`, add:
```rust
    #[test]
    fn read_checked_absent_is_ok_empty() {
        let dir = tempfile::tempdir().unwrap();
        let g = WorkGraph::read_checked(dir.path()).unwrap();
        assert!(g.nodes.is_empty());
    }

    #[test]
    fn read_checked_corrupt_is_err() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("workgraph.json"), "{ not json").unwrap();
        assert!(WorkGraph::read_checked(dir.path()).is_err());
    }

    #[test]
    fn read_checked_newer_schema_is_err() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("workgraph.json"),
            format!(r#"{{"schema_version":{},"nodes":[]}}"#, WG_SCHEMA_VERSION + 1),
        )
        .unwrap();
        assert!(WorkGraph::read_checked(dir.path()).is_err());
    }

    #[test]
    fn read_checked_valid_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = WorkGraph::default();
        g.add("m", "acc", vec![]).unwrap();
        g.save(dir.path()).unwrap();
        let g2 = WorkGraph::read_checked(dir.path()).unwrap();
        assert_eq!(g2.nodes.len(), 1);
    }
```
(`tempfile` is a dev-dependency; `WG_SCHEMA_VERSION` is `pub`/in-scope in this module.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test read_checked`
Expected: FAIL — `no function or associated item named read_checked`.

- [ ] **Step 3: Implement `read_checked`**

In `src/workgraph.rs`, immediately after the `read` method (ends ~line 117), add:
```rust
    /// Like [`read`], but distinguishes an absent/empty graph (Ok) from a
    /// present-but-unreadable `workgraph.json` (Err). Used by write paths so a
    /// corrupt or newer-than-supported file is never silently treated as empty
    /// and overwritten (data-loss guard, spec 2026-07-25). Absent file, a
    /// `todos.json` migration, and a genuinely empty graph are all `Ok`.
    pub fn read_checked(root: &Path) -> anyhow::Result<WorkGraph> {
        match std::fs::read_to_string(path(root)) {
            // File present and read: content is authoritative — parse or error.
            Ok(raw) => Self::load(&raw).map_err(|e| {
                anyhow::anyhow!("workgraph.json at {} is unreadable: {e}", path(root).display())
            }),
            // No readable workgraph.json: legacy todos migration, else empty (all legal).
            Err(_) => {
                if let Some(wg) = std::fs::read_to_string(root.join("todos.json"))
                    .ok()
                    .and_then(|raw| migrate_todos(&raw))
                {
                    Ok(wg)
                } else {
                    Ok(WorkGraph::default())
                }
            }
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test read_checked`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/workgraph.rs
git commit -m "feat(workgraph): add read_checked distinguishing absent from corrupt

Ok for absent/empty/todos-migration; Err only when workgraph.json exists but
load() fails (corrupt JSON or newer schema_version). Data-loss guard primitive.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `EmptyGraph` state + exit code 5 + BG entry guard (#1 + #3 wiring)

**Files:**
- Modify: `src/bg_gate.rs` (add `MissionState::EmptyGraph`)
- Modify: `src/bg_ledger.rs` (`mission_exit_code` arm)
- Modify: `src/background.rs` (`run_background_cfg` workgraph-branch entry ~line 153; `advance_one_milestone` and `retry_one_milestone` read sites)

**Interfaces:**
- Consumes: `WorkGraph::read_checked` (Task 1).
- Produces: `MissionState::EmptyGraph` (unit variant); `mission_exit_code(EmptyGraph) == 5`.

- [ ] **Step 1: Write the failing tests**

In `src/bg_ledger.rs` tests, add:
```rust
    #[test]
    fn empty_graph_exit_code_is_5() {
        assert_eq!(mission_exit_code(&crate::bg_gate::MissionState::EmptyGraph), 5);
    }
```
In `src/background.rs` `mod tests`, add:
```rust
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
```
(Empty `task` string routes into the workgraph branch; `MissionState` and `StubClient`/`Arc` are already imported in the test module.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test empty_graph_exit_code_is_5 workgraph_empty_graph_yields_empty_state workgraph_corrupt_file_aborts_and_backs_up`
Expected: FAIL — `no variant named EmptyGraph` (compile error).

- [ ] **Step 3: Add the `EmptyGraph` variant**

In `src/bg_gate.rs`, inside `enum MissionState`, add after the `CompletedAllReady` variant:
```rust
    /// 图中无任何里程碑：headless 无事可做。区别于 CompletedAllReady 的"真完成"
    /// （曾有里程碑且全部到达终态），避免空图 exit 0 假报成功（spec 2026-07-25）。
    EmptyGraph,
```

- [ ] **Step 4: Add the exit-code arm**

In `src/bg_ledger.rs` `mission_exit_code`, add an arm (the `match` is exhaustive, so the compiler already requires this):
```rust
        MissionState::EmptyGraph => 5,
```
Place it after the `CompletedAllReady | Running => 0` arm.

- [ ] **Step 5: Add the entry guard in the workgraph branch**

In `src/background.rs`, in `run_background_cfg`, replace the workgraph-branch reset line:
```rust
    // Reset the NDJSON event stream once at run start; per-milestone observers append.
    drop(crate::bg_observer::BgObserver::start_run(&root));
    out.mission_state = crate::bg_gate::MissionState::Running;
```
with:
```rust
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
    // #1 honesty: a genuinely empty graph is not "success" — nothing to advance.
    if graph.nodes.is_empty() {
        obs.emit("empty", "empty workgraph — nothing to advance; seed workgraph.json first");
        out.mission_state = crate::bg_gate::MissionState::EmptyGraph;
        return Ok(out);
    }
    out.mission_state = crate::bg_gate::MissionState::Running;
```

- [ ] **Step 6: Route `advance`/`retry` reads through `read_checked`**

In `src/background.rs` `advance_one_milestone`, change its first graph read:
```rust
        let g = WorkGraph::read(&root);
```
to (propagating a corrupt-file error out of the fn, which already returns `anyhow::Result`):
```rust
        let g = WorkGraph::read_checked(&root)?;
```
Do the identical swap in `retry_one_milestone`'s first graph read. (These cover the daemon idle-advance path, which calls `advance_one_milestone` directly. Leave the read-only `ready_id` probe in `run_background_cfg` as `WorkGraph::read` — the entry guard already validated the file.)

- [ ] **Step 7: Run the targeted tests, then the full suite**

Run: `cargo test empty_graph_exit_code_is_5 workgraph_empty_graph_yields_empty_state workgraph_corrupt_file_aborts_and_backs_up`
Expected: PASS.
Then run: `cargo test 2>&1 | grep -E "test result:|FAILED" | grep -v "0 failed" || echo GREEN`
Expected: `GREEN`. If the compiler flags any other non-exhaustive `match` on `MissionState` (e.g. in cc-web or elsewhere), add an `EmptyGraph` arm mirroring the nearest "incomplete/failure" handling (do NOT map it to success). Search first: `grep -rn "MissionState::" src/ | grep -v "EmptyGraph"`.

- [ ] **Step 8: Commit**

```bash
git add src/bg_gate.rs src/bg_ledger.rs src/background.rs
git commit -m "feat(background): honest EmptyGraph state (exit 5) + corrupt-file abort

Empty workgraph no longer exits 0 as false success; a present-but-unreadable
workgraph.json is backed up to .corrupt.<pid> and aborts (Error) instead of
being silently overwritten. advance/retry now read_checked (covers daemon).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `reason.rs::to_milestone` via `with_lock` (#5)

**Files:**
- Modify: `src/tool/reason.rs` (the `to_milestone` write block, ~lines 172-183)

**Interfaces:**
- Consumes: `WorkGraph::with_lock(root, |g| ...)` (existing).

- [ ] **Step 1: Confirm the existing test covers the behavior**

Run: `cargo test to_milestone_creates_milestone_from_locked_node`
Expected: PASS (this is the regression guard; behavior must not change).

- [ ] **Step 2: Route the write through `with_lock`**

In `src/tool/reason.rs`, replace the write block:
```rust
        let mut wg = crate::workgraph::WorkGraph::read(ctx.root);
        match wg.add(&title, &acceptance, vec![]) {
            Ok(new_id) => {
                wg.save(ctx.root)?;
                Ok(ToolOutput::ok(format!(
                    "converted inference node #{id} → workgraph milestone #{new_id}: {title}"
                )))
            }
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
```
with (mirrors the `milestone` tool in `src/tool/dev.rs`, which mutates under `with_lock`):
```rust
        match crate::workgraph::WorkGraph::with_lock(ctx.root, |g| g.add(&title, &acceptance, vec![])) {
            Ok(new_id) => Ok(ToolOutput::ok(format!(
                "converted inference node #{id} → workgraph milestone #{new_id}: {title}"
            ))),
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
```
(`with_lock` does the `read → mutate → save` atomically under the file lock and returns the closure's value — here the new id. The earlier read-only `tree`/`node` lookup above this block is unchanged.)

- [ ] **Step 3: Run the test to verify unchanged behavior**

Run: `cargo test to_milestone_creates_milestone_from_locked_node`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/tool/reason.rs
git commit -m "fix(reason): to_milestone writes via with_lock (ADR 0035 consistency)

Was bare read+add+save, could lose updates concurrent with a daemon tick or
the milestone tool. Now atomic under the workgraph lock, matching dev.rs.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Docs

**Files:**
- Modify: `docs/adr/0033-bg-ledger-and-exit-codes.md`, `docs/superpowers/specs/2026-07-25-workgraph-initialization-analysis.md`, `CLAUDE.md`, `README.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update ADR 0033**

In `docs/adr/0033-bg-ledger-and-exit-codes.md`, add exit code 5 to the exit-code table/list: `EmptyGraph → 5`（图中无任何里程碑，headless 无事可做的诚实失败，区别于 CompletedAllReady 的真完成）. Keep the existing 0/2/3/4 rows.

- [ ] **Step 2: Annotate the analysis spec**

In `docs/superpowers/specs/2026-07-25-workgraph-initialization-analysis.md` §九, mark foot-guns #1, #3, #5 as `已修（见 docs/superpowers/specs/2026-07-25-workgraph-integrity-fixes-design.md）`. Leave #2/#4/#6/#7 unchanged (still open).

- [ ] **Step 3: Update CLAUDE.md / README.md exit-code references**

Search both for BG exit-code mentions: `grep -n "退出码\|exit code\|exit 0/2/3/4\|0/2/3/4" CLAUDE.md README.md`. Wherever the `0/2/3/4` exit-code set is described, add `5=EmptyGraph（空图，需先 seed）`. If neither file enumerates exit codes, note that in the report and make no change there.

- [ ] **Step 4: Verify the tree is still green**

Run: `cargo test 2>&1 | grep -E "test result:|FAILED" | grep -v "0 failed" || echo GREEN`
Expected: `GREEN` (docs don't affect tests; confirm nothing else drifted).

- [ ] **Step 5: Commit**

```bash
git add docs/ CLAUDE.md README.md
git commit -m "docs: record EmptyGraph exit code 5 and mark foot-guns #1/#3/#5 fixed

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** #1 → Task 2 (EmptyGraph variant + exit 5 + entry empty-guard); #3 → Task 1 (read_checked) + Task 2 (entry corrupt-abort-backup + advance/retry swap); #5 → Task 3; docs → Task 4. All spec sections mapped.
- **Out-of-scope preserved:** no task adds auto-seeding, touches `next_id`, or changes verdict-writeback semantics (Global Constraints).
- **Type consistency:** `read_checked(&Path) -> anyhow::Result<WorkGraph>` used identically in Tasks 1 and 2; `MissionState::EmptyGraph` (unit variant) defined in Task 2 Step 3 and consumed in Step 4/5 and the ledger test; backup filename `workgraph.json.corrupt.<pid>` identical in the abort code (Task 2 Step 5) and its test (Step 1).
- **Exhaustive-match hazard flagged:** Task 2 Step 7 greps for other `MissionState::` matches so a new variant doesn't silently map to success.
- **Verify-before-trust:** Task 1 notes `tempfile` is a dev-dep; Task 3 keeps the existing regression test as the behavior guard.
