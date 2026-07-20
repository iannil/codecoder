# Registry Hot-Reload — Design Spec

**Date:** 2026-07-20
**Branch:** `feat/registry-hot-reload`
**Status:** Approved (brainstormed 2026-07-20)
**Related:** ADR 0020 (Skills/Capabilities Registry), ADR 0032 (client-server architecture), client-server migration plan Task 4 (shared `Arc<Registry>`, stretch item)

## Goal

Make the daemon's shared `Registry` reflect changes to `skills/`, `capabilities/`, and `prompts/` within a few seconds — **without restarting the daemon** and **without issuing `/reload`**. Any file change (agent self-authored via `generate_skill`/`generate_capability`/`promote_prompt`, **or** manual/external edits) propagates to every active session's next turn.

## Background / Current State

After the client-server migration:
- The daemon holds a shared `Arc<Registry>`, scanned **once** at startup (`Daemon::run` → `Registry::scan(&cfg.root)`), threaded into every session via `AgentLoop::new_daemon(..., registry)`.
- `Registry::reload(&mut self, root)` exists (`src/registry.rs`) but is **only called from a test** — the daemon never refreshes the catalog at runtime.
- `AgentLoop.system_prompt: String` is a **cached** field: built once in `build()`, refreshed only on `load_self()` and `AgentCommand::Reload`. It is **not** rebuilt per turn.
- `build_system_prompt_with_registry(root, &Registry)` (agent.rs) renders the catalog into the system prompt.

**Limitation this spec removes:** after a skill/capability is written, other sessions do not see it in their catalog until the daemon restarts.

## Non-goals

- `inotify`/`kqueue`/fs-event watching (we poll mtime — see Rationale).
- Mid-turn propagation (changes appear on a session's **next** turn, not during an in-flight turn).
- Generation/version tracking (per-turn rebuild is cheap enough to skip this).
- Cross-process coordination (single in-process daemon; the Registry is shared memory, not a file lock).

## Design — Approach 1: `Arc<RwLock<Registry>>` + per-turn rebuild

### Shared mutability

- The shared handle changes type from `Arc<Registry>` → `Arc<RwLock<Registry>>` (`std::sync`, **no new dependency**).
- Many sessions read concurrently (RwLock permits concurrent reads); the reload tick write-locks briefly and rarely.

### Change detection: polling directory mtime

Two new **pure functions** in `src/registry.rs` (no timers — directly testable):

```rust
/// The mtimes of the three scanned dirs, remembered between ticks.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirMtimes {
    pub skills: Option<std::time::SystemTime>,
    pub capabilities: Option<std::time::SystemTime>,
    pub prompts: Option<std::time::SystemTime>,
}

/// Stat the three dirs, compare to `last`. On the first call (all `None`),
/// record mtimes and return `false` (startup scan already populated the
/// Registry). On later calls, return `true` iff any mtime changed, and
/// update `last` to the just-observed mtimes.
pub fn mtime_changed(root: &Path, last: &mut DirMtimes) -> bool

/// If mtimes changed since `last`, re-scan into the shared Registry under a
/// write lock. Missing dirs are treated as "no change" (stat fails → keep the
/// prior mtime / skip), so the catalog is never clobbered by a transient
/// stat error.
pub fn tick_reload(
    reg: &Arc<RwLock<Registry>>,
    root: &Path,
    last: &mut DirMtimes,
)
```

### Daemon reload thread

In `src/daemon/mod.rs::Daemon::run()`, alongside the existing supervisor and workgraph tick threads, spawn one more:

- holds clones of the shared `Arc<RwLock<Registry>>` and `cfg.root`;
- loop: `sleep(3s)`; `tick_reload(&reg, &root, &mut last)`; repeat until `shutdown`;
- joined on shutdown (same pattern as the other two threads).

3 seconds is the max latency for a change to enter the shared Registry.

### Per-turn rebuild

- `AgentLoop.shared_registry` type: `Option<Arc<Registry>>` → `Option<Arc<RwLock<Registry>>>`.
- New method:

  ```rust
  /// If this session shares a daemon Registry, rebuild `system_prompt` from
  /// its current contents (read lock). No-op when `shared_registry` is `None`
  /// (sub-agents / background agents self-scan in `build()`), AND no-op unless
  /// `self.trust == TrustedState::Trusted` (mirrors `build()` / `Reload`:
  /// an untrusted/pending project never loads its disk self).
  fn refresh_system_prompt_if_shared(&mut self)
  ```

- **Call site:** the top of `process_turn(text, event_tx)` (agent.rs:650), before the cached `system_prompt` is read/pushed as the System message. This single insertion covers:
  - the run loop's `AgentCommand::ProcessMessage` path (interactive `cc` turns), and
  - `run_one_turn` (background turns — `run_one_turn` → `process_turn`).
- `new_sub` / `new_background` pass `None`, so the refresh is a no-op for them — unchanged behavior.

`build_system_prompt_with_registry(root, &Registry)` keeps its current signature; callers obtain the `&Registry` by read-locking and passing `&*guard`.

### Data flow

1. A file is written under `skills/` / `capabilities/` / `prompts/` (by an agent self-author tool, or by a manual `vim`/external edit).
2. Within ≤3 s, the daemon reload thread stats the dir, sees the mtime change, re-scans, and `write().reload(root)` updates the shared catalog.
3. On the **next turn** of any session, `refresh_system_prompt_if_shared` (called at the top of `process_turn`) read-locks the shared Registry, rebuilds `system_prompt`, which is then pushed as the System message → the LLM sees the new/changed catalog entry.

## Error handling

- **Stat failure** (dir transiently missing): `mtime_changed` treats a failed stat as "no change" (keep the prior mtime), so `tick_reload` skips without clobbering the catalog.
- **Scan errors:** `Registry::scan` already swallows per-file read/parse errors (`unwrap_or_default`), so a single corrupt file does not break the reload.
- **Lock poison:** `reg.read().unwrap()` / `reg.write().unwrap()`, matching the codebase convention (e.g. `capability::services()`).

## Testing

- `registry::tests::tick_reload_picks_up_new_skill` — temp root, empty `Arc<RwLock<Registry>>` (Default); call `tick_reload` once (records mtimes); write a new `skills/x.md`; call `tick_reload` again; read-lock and assert `x` is in the catalog.
- `registry::tests::tick_reload_noop_when_unchanged` — two calls with no file change between them; catalog length/contents unchanged.
- `registry::tests::tick_reload_skips_missing_dirs` — root with no `skills/` dir; `tick_reload` does not panic and leaves the catalog intact.
- `registry::tests::mtime_changed_first_call_records_and_returns_false` — first call returns `false` and populates `last`.
- `agent::tests::refresh_system_prompt_picks_up_new_skill` — construct an `AgentLoop` with `Some(shared)`; write a new skill into `shared` (via `tick_reload` or direct `reload`); call `refresh_system_prompt_if_shared`; assert `system_prompt` now contains the new catalog line.
- **Baseline regression:** the existing suite (~203 tests) stays green. The `shared_registry` type change ripples only through the daemon path; the `None` path (sub-agents, background) is behaviorally unchanged.

## Rationale — why polling mtime

| Option | Latency | New dep | Catches external edits | Code cost | Verdict |
|---|---|---|---|---|---|
| **Polling mtime (chosen)** | ~3 s | none | yes | trivial | chosen |
| `notify` crate | instant | +1 | yes | low | overkill; skill reload doesn't need sub-second latency |
| raw `kqueue`/`inotify` | instant | none | yes | two platform impls | too much platform-specific code for the benefit |

Polling mtime wins: no dependency, catches **all** changes (including manual/external edits), acceptable latency, trivial implementation, and fits the existing OS-threads/no-async kernel ethos. The daemon already runs periodic tick threads (supervisor @1 s, workgraph @30 s); one more at 3 s is consistent.

## Interfaces (contract for the implementation plan)

- `registry::DirMtimes` (struct, `Default`).
- `registry::mtime_changed(root, &mut DirMtimes) -> bool`.
- `registry::tick_reload(reg: &Arc<RwLock<Registry>>, root, &mut DirMtimes)`.
- `AgentLoop::refresh_system_prompt_if_shared(&mut self)` (private).
- `AgentLoop.shared_registry: Option<Arc<RwLock<Registry>>>`.
- `DaemonSessionManager::new(...)` takes `Arc<RwLock<Registry>>` (was `Arc<Registry>`).
- `AgentLoop::new_daemon(...)` takes `Arc<RwLock<Registry>>` (was `Arc<Registry>`).

## Files touched

- `src/registry.rs` — `DirMtimes`, `mtime_changed`, `tick_reload` + tests.
- `src/agent.rs` — `shared_registry` type; `refresh_system_prompt_if_shared`; call at top of `process_turn`; update `build` / `new_daemon` signatures and the `Reload` arm.
- `src/daemon/mod.rs` — shared Registry as `Arc<RwLock<Registry>>`; spawn + join the reload thread.
- `src/daemon/session_manager.rs` — `new()` accepts `Arc<RwLock<Registry>>`; store/clone the `RwLock` handle.
