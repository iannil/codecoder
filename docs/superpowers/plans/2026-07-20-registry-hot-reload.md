# Registry Hot-Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the daemon's shared `Registry` reflect any change to `skills/`/`capabilities/`/`prompts/` within ~3 s — no daemon restart, no `/reload` — so every active session sees the new catalog on its next turn.

**Architecture:** Shared handle `Arc<Registry>` → `Arc<RwLock<Registry>>` (std only). A daemon reload thread polls directory mtimes every 3 s and re-scans on change into the shared `RwLock`. `AgentLoop` rebuilds its cached `system_prompt` from the shared Registry at the top of each `process_turn` (read-lock, trust-gated).

**Tech Stack:** Rust (edition 2024), `std::sync::{Arc, RwLock}`, `std::time::SystemTime`, `std::fs::Metadata::modified`. **No new dependencies.** No async runtime.

**Spec:** `docs/superpowers/specs/2026-07-20-registry-hot-reload-design.md`

---

## Global Constraints

- **No new dependencies** — `std::sync::RwLock` only. Do NOT add `notify`, `arc-swap`, or any watcher crate.
- New `pub` types/functions need doc comments.
- New tests use `std::env::temp_dir().join(format!("cc_<name>_{}", std::process::id()))` (unique suffix per test) and `std::fs::remove_dir_all(&dir).ok()` cleanup — match `registry.rs`'s existing test style.
- The `shared_registry = None` path (sub-agents `new_sub`, background `new_background`) stays behaviorally identical (self-scan in `build()`).
- `refresh_system_prompt_if_shared` is a no-op unless `self.trust == TrustState::Trusted` (mirrors `build()`/`Reload`).
- Existing suite (~203 tests) stays green; `cargo build` warning-free (no unused imports).
- Commit messages: `feat:`/`refactor:` prefix, single line, English.

---

## File Structure

| File | Responsibility | Create/Modify |
|---|---|---|
| `src/registry.rs` | `DirMtimes` struct + pure `mtime_changed` + `tick_reload` + tests | Modify (add) |
| `src/agent.rs` | `shared_registry` type → `Arc<RwLock<Registry>>`; `refresh_system_prompt_if_shared`; call at top of `process_turn`; update `build`/`new_daemon`/`Reload` read sites | Modify |
| `src/daemon/session_manager.rs` | `registry` field + `new()` param → `Arc<RwLock<Registry>>` | Modify |
| `src/daemon/mod.rs` | shared Registry as `Arc<RwLock<Registry>>`; spawn + join reload thread | Modify |

---

### Task 1: `DirMtimes` + `mtime_changed` + `tick_reload` (pure functions, registry.rs)

**Files:**
- Modify: `src/registry.rs` — add `DirMtimes`, `mtime_changed`, `tick_reload` + 4 tests
- Test: inline `#[cfg(test)] mod tests` (already present in registry.rs)

**Interfaces:**
- Consumes: `Registry::scan` (registry.rs:34), `Registry::reload(&mut self, root)` (registry.rs:79), `Registry` derives `Default` (registry.rs:26).
- Produces (used by Task 4):
  - `pub struct DirMtimes { pub skills: Option<SystemTime>, pub capabilities: Option<SystemTime>, pub prompts: Option<SystemTime> }` (`#[derive(Debug, Default, Clone, PartialEq, Eq)]`)
  - `pub fn mtime_changed(root: &Path, last: &mut DirMtimes) -> bool`
  - `pub fn tick_reload(reg: &Arc<RwLock<Registry>>, root: &Path, last: &mut DirMtimes)`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `src/registry.rs` (the module already imports `super::*`; add `use std::sync::{Arc, RwLock};` at the top of the test module if not present):

```rust
    use std::sync::{Arc, RwLock};

    #[test]
    fn mtime_changed_first_call_records_and_returns_false() {
        let dir = std::env::temp_dir().join(format!("cc_mtime_first_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let mut last = DirMtimes::default();
        assert!(!mtime_changed(&dir, &mut last), "first call records, no change");
        assert!(last.skills.is_some(), "skills mtime recorded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_reload_picks_up_new_skill() {
        let dir = std::env::temp_dir().join(format!("cc_reload_new_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let reg = Arc::new(RwLock::new(Registry::default()));
        let mut last = DirMtimes::default();
        tick_reload(&reg, &dir, &mut last); // records mtimes, catalog empty
        assert!(reg.read().unwrap().catalog.is_empty());
        std::fs::write(
            dir.join("skills/x.md"),
            "---\nname: x\ndescription: d\n---\nbody",
        ).unwrap();
        tick_reload(&reg, &dir, &mut last); // detects change, rescans
        let cat = &reg.read().unwrap().catalog;
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0].name, "x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_reload_noop_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("cc_reload_noop_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills/x.md"), "---\nname: x\ndescription: d\n---\nb").unwrap();
        let reg = Arc::new(RwLock::new(Registry::default()));
        let mut last = DirMtimes::default();
        tick_reload(&reg, &dir, &mut last); // loads x
        let n = reg.read().unwrap().catalog.len();
        tick_reload(&reg, &dir, &mut last); // no change
        assert_eq!(reg.read().unwrap().catalog.len(), n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_reload_skips_missing_dirs() {
        let dir = std::env::temp_dir().join(format!("cc_reload_missing_{}", std::process::id()));
        // intentionally do NOT create skills/capabilities/prompts
        let reg = Arc::new(RwLock::new(Registry::default()));
        let mut last = DirMtimes::default();
        tick_reload(&reg, &dir, &mut last); // must not panic, must not clobber
        assert!(reg.read().unwrap().catalog.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib registry::tests 2>&1 | tail -20`
Expected: compile errors — `cannot find types/fields 'DirMtimes'`, `cannot find function 'mtime_changed'`, `cannot find function 'tick_reload'`.

- [ ] **Step 3: Implement `DirMtimes`, `mtime_changed`, `tick_reload`**

Add to `src/registry.rs` (after the `impl Registry { ... }` block, before the `fn scan_skills` free functions):

```rust
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

/// Remembered mtimes of the three scanned directories, kept between reload
/// ticks. `Default` is all-`None` (first tick records without reporting change).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirMtimes {
    pub skills: Option<SystemTime>,
    pub capabilities: Option<SystemTime>,
    pub prompts: Option<SystemTime>,
}

/// Stat `skills/`, `capabilities/`, `prompts/` under `root` and compare to
/// `last`. Returns `true` iff any observed mtime differs from the previously
/// remembered value; updates `last` to the just-observed mtimes. The FIRST
/// observation of a dir (slot was `None`) records the mtime but does NOT count
/// as a change (the startup scan already populated the Registry). A failed
/// stat (missing dir) leaves the prior value untouched — the catalog is never
/// clobbered by a transient stat error.
pub fn mtime_changed(root: &Path, last: &mut DirMtimes) -> bool {
    let slots: [(&str, &mut Option<SystemTime>); 3] = [
        ("skills", &mut last.skills),
        ("capabilities", &mut last.capabilities),
        ("prompts", &mut last.prompts),
    ];
    let mut changed = false;
    for (sub, slot) in slots {
        match root.join(sub).metadata().and_then(|m| m.modified()) {
            Ok(mtime) => match *slot {
                None => *slot = Some(mtime),                 // first observation: record, no change
                Some(prev) if prev != mtime => { *slot = Some(mtime); changed = true; }
                _ => {}                                       // unchanged
            },
            Err(_) => {}                                       // keep prior; no change
        }
    }
    changed
}

/// If any scanned dir's mtime changed since `last`, re-scan into the shared
/// Registry under a write lock. Otherwise no-op. Call this on a timer (the
/// daemon reload thread) — it has no internal clock, so it is directly testable.
pub fn tick_reload(reg: &Arc<RwLock<Registry>>, root: &Path, last: &mut DirMtimes) {
    if mtime_changed(root, last) {
        reg.write().unwrap().reload(root);
    }
}
```

> If `use std::path::Path;` is already imported at the top of registry.rs (it is — registry.rs:4), do not re-add it. Add only `use std::sync::{Arc, RwLock};` and `use std::time::SystemTime;` if not already present. If the compiler reports them as unused elsewhere, keep them — `tick_reload`/`mtime_changed` use them.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib registry::tests 2>&1 | tail -20`
Expected: 6 tests pass (2 pre-existing + 4 new), 0 failed.

- [ ] **Step 5: Commit**

```bash
git add src/registry.rs
git commit -m "feat: registry mtime polling (DirMtimes, mtime_changed, tick_reload)"
```

---

### Task 2: Shared Registry type → `Arc<RwLock<Registry>>` (mechanical ripple)

**Files:**
- Modify: `src/agent.rs` — `shared_registry` field (line 201), `build` signature (line 269), `new_daemon` (line 229), build body read site (lines 300-301), `Reload` arm read sites (lines 401-409)
- Modify: `src/daemon/session_manager.rs` — `registry` field (line 30), `new()` param (line 47)
- Modify: `src/daemon/mod.rs` — construction (line 24)

**Interfaces:**
- Consumes: Task 1's `Arc<RwLock<Registry>>` type (now defined).
- Produces: all shared-Registry sites use `Arc<RwLock<Registry>>`; reads go through `.read().unwrap()`. Behavior unchanged (no reload thread, no per-turn refresh yet — those are Tasks 3 & 4).

**No new behavior in this task** — it is a pure type refactor. The existing test `build_system_prompt_uses_provided_registry` (agent.rs:2039) and the full suite are the verification.

- [ ] **Step 1: `src/agent.rs` — change the field type and signatures**

Line 201 — change:
```rust
    shared_registry: Option<Arc<Registry>>,
```
to:
```rust
    shared_registry: Option<Arc<std::sync::RwLock<Registry>>>,
```

`new_daemon` (starts at line 229) — change its last parameter from `registry: Arc<Registry>` to `registry: Arc<std::sync::RwLock<Registry>>`. The body passes `Some(registry)` to `build`; that stays the same.

`build` signature (line 269) — change `shared_registry: Option<Arc<Registry>>` to `shared_registry: Option<Arc<std::sync::RwLock<Registry>>>`.

- [ ] **Step 2: `src/agent.rs` — update the two read sites to read-lock**

Build body, lines 300-301 — change:
```rust
        match &shared_registry {
            Some(reg) => build_system_prompt_with_registry(&root, reg),
            None => build_system_prompt(&root),
        }
```
to:
```rust
        match &shared_registry {
            Some(reg) => build_system_prompt_with_registry(&root, &reg.read().unwrap()),
            None => build_system_prompt(&root),
        }
```

`Reload` arm (starts line 397) — the `n` count and the `system_prompt` rebuild both read `self.shared_registry`. Change:
```rust
                        let n = match &self.shared_registry {
                            Some(reg) => reg.catalog.len(),
                            None => Registry::scan(&self.root).catalog.len(),
                        };
                        self.system_prompt = match &self.shared_registry {
                            Some(reg) => build_system_prompt_with_registry(&self.root, reg),
                            None => build_system_prompt(&self.root),
                        };
```
to:
```rust
                        let n = match &self.shared_registry {
                            Some(reg) => reg.read().unwrap().catalog.len(),
                            None => Registry::scan(&self.root).catalog.len(),
                        };
                        self.system_prompt = match &self.shared_registry {
                            Some(reg) => build_system_prompt_with_registry(
                                &self.root,
                                &reg.read().unwrap(),
                            ),
                            None => build_system_prompt(&self.root),
                        };
```

> `build_system_prompt_with_registry(root, &Registry)` (line 1227) signature is UNCHANGED — callers obtain the `&Registry` by read-locking and passing `&*guard` (here done inline as `&reg.read().unwrap()`).

- [ ] **Step 3: `src/daemon/session_manager.rs` — field + param type**

Line 30 — change `registry: Arc<Registry>,` to `registry: Arc<std::sync::RwLock<Registry>>,`.
Line 47 (`new`'s param) — change `registry: Arc<Registry>,` to `registry: Arc<std::sync::RwLock<Registry>>,`.
Line 80 (`self.registry.clone()` passed to `new_daemon`) — unchanged (cloning an `Arc<RwLock<…>>` is the same call).

- [ ] **Step 4: `src/daemon/mod.rs` — wrap the startup scan in `RwLock`**

Line 24 — change:
```rust
        let registry = Arc::new(crate::registry::Registry::scan(&self.cfg.root));
```
to:
```rust
        let registry = Arc::new(std::sync::RwLock::new(crate::registry::Registry::scan(&self.cfg.root)));
```

(The value is moved into `DaemonSessionManager::new` on line 25-32 unchanged.)

- [ ] **Step 5: Build + run the full suite**

Run: `cargo build 2>&1 | tail -15`
Expected: compiles with no warnings.

Run: `cargo test 2>&1 | tail -25`
Expected: 0 failed, 3 ignored (the standard L2/L3/Docker gates). In particular `build_system_prompt_uses_provided_registry` (agent.rs:2039) still passes — the daemon/client integration tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/agent.rs src/daemon/session_manager.rs src/daemon/mod.rs
git commit -m "refactor: shared Registry becomes Arc<RwLock<Registry>> (behavior unchanged)"
```

---

### Task 3: `refresh_system_prompt_if_shared` + per-turn call

**Files:**
- Modify: `src/agent.rs` — add `refresh_system_prompt_if_shared` method; call it at the top of `process_turn` (line 676); add a test.

**Interfaces:**
- Consumes: Task 2's `Arc<RwLock<Registry>>` shared field; `build_system_prompt_with_registry` (agent.rs:1227); `TrustState::Trusted`.
- Produces: `fn refresh_system_prompt_if_shared(&mut self)` (private). Called internally at the top of `process_turn`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/agent.rs` (the module already has access to private fields/methods):

```rust
    #[test]
    fn refresh_system_prompt_picks_up_new_skill() {
        use std::sync::{Arc, RwLock};
        let dir = std::env::temp_dir().join(format!("cc_refresh_shared_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(
            dir.join("skills/old.md"),
            "---\nname: old\ndescription: o\n---\nbody",
        ).unwrap();
        let reg = Arc::new(RwLock::new(crate::registry::Registry::scan(&dir)));
        let mut agent = AgentLoop::new_daemon(
            Arc::new(crate::provider::stub::StubClient),
            "gpt-4o".into(),
            4096,
            0.7,
            dir.clone(),
            reg.clone(),
        );
        agent.trust = crate::agent::TrustState::Trusted;
        // shared registry gains a new skill AFTER construction
        std::fs::write(
            dir.join("skills/new.md"),
            "---\nname: new\ndescription: n\n---\nbody",
        ).unwrap();
        reg.write().unwrap().reload(&dir);
        agent.refresh_system_prompt_if_shared();
        assert!(agent.system_prompt.contains("new"), "refresh must pick up the new skill");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib refresh_system_prompt_picks_up_new_skill 2>&1 | tail -15`
Expected: compile error — `no method named refresh_system_prompt_if_shared found`.

- [ ] **Step 3: Implement `refresh_system_prompt_if_shared`**

Add this method to `impl AgentLoop` (place it right after `load_self`, which ends around line 335 — anywhere inside the `impl AgentLoop` block that has access to `self.shared_registry` / `self.system_prompt` / `self.root` / `self.trust`):

```rust
    /// Rebuild `system_prompt` from the shared daemon Registry if this session
    /// has one and is trusted. Called at the top of every `process_turn` so a
    /// skill/capability written by another session (or a manual edit) shows up
    /// on this session's next turn. No-op for sub-agents / background agents
    /// (`shared_registry` is `None`) and for untrusted/pending projects.
    fn refresh_system_prompt_if_shared(&mut self) {
        if self.trust != TrustState::Trusted {
            return;
        }
        if let Some(reg) = &self.shared_registry {
            let g = reg.read().unwrap();
            self.system_prompt = build_system_prompt_with_registry(&self.root, &g);
        }
    }
```

- [ ] **Step 4: Call it at the top of `process_turn`**

`process_turn` starts at line 676:
```rust
    fn process_turn(&mut self, text: String, event_tx: &Sender<AgentEvent>) {
```
Make the FIRST statement inside its body (before anything else — in particular before the cached `system_prompt` is read and pushed as the System message around line 699):

```rust
    fn process_turn(&mut self, text: String, event_tx: &Sender<AgentEvent>) {
        self.refresh_system_prompt_if_shared();
        // ... rest of existing body unchanged ...
```

- [ ] **Step 5: Run test to verify it passes + full suite green**

Run: `cargo test --lib refresh_system_prompt_picks_up_new_skill 2>&1 | tail -10`
Expected: PASS.

Run: `cargo test 2>&1 | tail -25`
Expected: 0 failed, 3 ignored. The sub-agent / background paths (`shared_registry = None`) are unaffected — `refresh_system_prompt_if_shared` returns early.

- [ ] **Step 6: Commit**

```bash
git add src/agent.rs
git commit -m "feat: AgentLoop rebuilds system_prompt from shared Registry each turn"
```

---

### Task 4: Daemon reload thread (3 s mtime poll)

**Files:**
- Modify: `src/daemon/mod.rs` — clone the shared `Arc<RwLock<Registry>>` before it moves into the manager; spawn the reload thread; join it on shutdown.
- Test: add an integration-style test verifying the thread + `tick_reload` + shutdown picks up a written skill.

**Interfaces:**
- Consumes: Task 1's `tick_reload`/`DirMtimes`; Task 2's `Arc<RwLock<Registry>>` shared handle; the existing shutdown `AtomicBool` + thread-join pattern (see `sup_handle` at daemon/mod.rs:47 and `wg_handle` at :65, joined at :104-105).

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)] mod tests` block at the bottom of `src/daemon/mod.rs` (the file currently has tests? — `Daemon` has a `daemon_constructs_with_temp_root` test; add to that module. If none exists, create one.):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{tick_reload, DirMtimes, Registry};
    use std::sync::{Arc, RwLock};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn reload_loop_picks_up_written_skill() {
        let dir = std::env::temp_dir().join(format!("cc_reload_thread_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let reg = Arc::new(RwLock::new(Registry::scan(&dir)));
        let reg_for_thread = Arc::clone(&reg);
        let root_for_thread = dir.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_thread = shutdown.clone();
        let handle = std::thread::spawn(move || {
            let mut last = DirMtimes::default();
            while !shutdown_for_thread.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50)); // fast tick for the test
                tick_reload(&reg_for_thread, &root_for_thread, &mut last);
            }
        });
        // write a new skill AFTER the loop started
        std::fs::write(
            dir.join("skills/x.md"),
            "---\nname: x\ndescription: d\n---\nbody",
        ).unwrap();
        // allow at least one tick after the write
        std::thread::sleep(std::time::Duration::from_millis(200));
        shutdown.store(true, Ordering::SeqCst);
        handle.join().unwrap();
        let has_x = reg.read().unwrap().catalog.iter().any(|e| e.name == "x");
        assert!(has_x, "reload loop must pick up the written skill");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib daemon::tests::reload_loop_picks_up_written_skill 2>&1 | tail -15`
Expected: this test is self-contained (it does not call `Daemon::run`), so it should PASS immediately once `tick_reload`/`DirMtimes` exist (they do, from Task 1). If it passes already, that confirms the loop logic; proceed. (The real wiring under test is Step 3 — verified by `cargo build` + existing daemon tests.)

> Note: this test exercises the same loop body the production thread runs, just on a 50 ms cadence. The production thread uses 3 s (Step 3).

- [ ] **Step 3: Wire the reload thread into `Daemon::run`**

In `src/daemon/mod.rs::run()`, after building `registry` (line 24, now `Arc<RwLock<Registry>>`) but BEFORE it moves into `DaemonSessionManager::new` (line 25), clone it for the reload thread:

```rust
        let registry = Arc::new(std::sync::RwLock::new(crate::registry::Registry::scan(&self.cfg.root)));
        let registry_for_reload = Arc::clone(&registry);
        let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
            provider,
            self.cfg.model.clone(),
            self.cfg.max_tokens,
            self.cfg.temperature,
            self.cfg.root.clone(),
            registry,
        )));
```

Then, alongside the supervisor thread (`sup_handle`, ~line 47) and the workgraph thread (`wg_handle`, ~line 65), add the reload thread (after `shutdown` is created on line 37 and `turn_token` wiring, but before the accept loop). Place it right after the `wg_handle` spawn block:

```rust
        let root_for_reload = self.cfg.root.clone();
        let shutdown_for_reload = Arc::clone(&shutdown);
        let reload_handle = std::thread::spawn(move || {
            let mut last = crate::registry::DirMtimes::default();
            while !shutdown_for_reload.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_secs(3));
                crate::registry::tick_reload(&registry_for_reload, &root_for_reload, &mut last);
            }
        });
```

And join it at shutdown — right after the existing joins (line 104-105):

```rust
        let _ = sup_handle.join();
        let _ = wg_handle.join();
        let _ = reload_handle.join();
        crate::capability::shutdown_all();
        Ok(())
```

- [ ] **Step 4: Build + full suite**

Run: `cargo build 2>&1 | tail -15`
Expected: compiles, no warnings.

Run: `cargo test 2>&1 | tail -25`
Expected: 0 failed, 3 ignored. The new `reload_loop_picks_up_written_skill` test passes.

- [ ] **Step 5: Manual smoke (optional, confirms end-to-end latency)**

```bash
ROOT=$(mktemp -d)
mkdir -p $ROOT/skills
CODECODER_ROOT=$ROOT CODECODER_DAEMON=1 cargo run --quiet 2>/dev/null &
DPID=$!
sleep 2
echo '---\nname: later\ndescription: added at runtime\n---\nhi' > $ROOT/skills/later.md
sleep 4   # wait > 3s for the reload tick
CODECODER_ROOT=$ROOT cargo run --bin cc --quiet -- "list skills" 2>/dev/null | head
kill $DPID 2>/dev/null; rm -rf $ROOT
```
Expected: the daemon does not need a restart; within ~3 s of writing `later.md` the catalog reflects it. (Whether the reply mentions "later" depends on the provider; the absence of a daemon restart is the key signal. This step is informational — the unit/integration tests are authoritative.)

- [ ] **Step 6: Commit**

```bash
git add src/daemon/mod.rs
git commit -m "feat: daemon reload thread polls skills/capabilities/prompts mtimes every 3s"
```

---

## Self-Review (already run — notes for the implementer)

- **Spec coverage:** every spec section maps to a task — shared mutability (Task 2), mtime detection (Task 1), daemon thread (Task 4), per-turn rebuild (Task 3), error handling (stat-fail in `mtime_changed` Task 1 Step 3; lock-poison `.unwrap()` consistent with codebase), testing (all 5 spec tests present: 4 in registry Task 1 + 1 in agent Task 3; plus the daemon loop test Task 4).
- **Type consistency:** `Arc<RwLock<Registry>>` used uniformly across Tasks 2-4; `DirMtimes`/`mtime_changed`/`tick_reload` signatures match between Task 1 (defined) and Task 4 (called). `build_system_prompt_with_registry(root, &Registry)` unchanged everywhere.
- **No placeholders:** every step has complete code or exact commands.
