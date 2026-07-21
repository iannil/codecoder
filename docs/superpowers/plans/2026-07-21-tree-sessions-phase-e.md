# Tree Sessions Phase E Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify the session tree with the `reason` tool's causal tree — a session branch can be linked to the causal node it explores (`reason link`), and abandoning a linked branch writes a structured "ruled out because…" ruling to both the session entry's meta and the causal node; `cc tree` and `reason list` surface the linkage.

**Architecture:** `ToolOutput` gains an optional `session_meta_mark`; `dispatch_tool` (which holds `self.session`) applies it to the current leaf — the side-channel that lets a tool mark the session without `ToolCtx` session access. `reason link node=<id>` uses it to write `{causal_node, status:"hypothesis"}`. The Navigate handler's Phase C abandon path, after summarizing, writes the summary as a ruling to both the abandoned entry's meta (`ruled_out`) and the causal node (`record_ruling`). `TreeNode` + `print_tree` + `reason list` render the linkage.

**Tech Stack:** Rust (edition 2024), `serde_json::Value`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-21-tree-sessions-phase-e-design.md`

---

## Global Constraints

- **No new dependencies.** New `pub` items need doc comments.
- New tests use `std::env::temp_dir().join(format!("cc_<name>{}", std::process::id()))` + `remove_dir_all` cleanup.
- Existing suite stays green; `cargo build` warning-free; **full suite COMPLETES (no hang)**.
- `causal_tree.json` is **unversioned** — adding `CausalNode.ruling` with `#[serde(default, skip_serializing_if = "Option::is_none")]` keeps old files loadable (NO migration).
- `ToolCtx` signature is UNCHANGED (the whole point is to avoid giving tools session access).
- Commit messages: `feat:`/`refactor:` prefix, single line, English.

---

## File Structure

| File | Responsibility | Create/Modify |
|---|---|---|
| `src/tool/mod.rs` | `ToolOutput.session_meta_mark` + `with_session_meta_mark` builder | Modify |
| `src/agent.rs` | `AgentLoop::apply_session_meta_mark`; `dispatch_tool` wires it; Navigate handler writes the ruling | Modify |
| `src/tool/reason.rs` | `link` action; `CausalNode.ruling`; `record_ruling`; `list`/`trace` rendering | Modify |
| `src/daemon/proto.rs` | `TreeNode.causal_node` / `status` | Modify |
| `src/daemon/socket.rs` | `build_tree_nodes` extracts causal_node/status from meta | Modify |
| `src/client/mod.rs` | `print_tree` renders the markers | Modify |
| `skills/debug-causal.md` | add a "link the branch" step | Modify |

---

### Task 1: `ToolOutput.session_meta_mark` + `dispatch_tool` applies it

**Files:**
- Modify: `src/tool/mod.rs` — `ToolOutput.session_meta_mark` field + `with_session_meta_mark` builder
- Modify: `src/agent.rs` — `AgentLoop::apply_session_meta_mark(&mut self, mark)`; `dispatch_tool` calls it
- Test: `src/tool/mod.rs` inline tests + `src/agent.rs` inline tests

**Interfaces:**
- Consumes: `Session::update_meta(id, f)`, `Session::leaf`, `Session::save`.
- Produces (used by Task 2):
  - `ToolOutput { content: String, is_error: bool, session_meta_mark: Option<serde_json::Value> }`
  - `ToolOutput::with_session_meta_mark(self, mark: serde_json::Value) -> Self`
  - `AgentLoop::apply_session_meta_mark(&mut self, mark: serde_json::Value)` (private; applies to current leaf, autosaves)

- [ ] **Step 1: Add the field + builder to `ToolOutput`**

In `src/tool/mod.rs`, change the struct + constructors:

```rust
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Optional meta to write onto the current session leaf's `SessionEntry.meta`.
    /// Applied by `AgentLoop::dispatch_tool` after the tool runs — tools can't touch
    /// the in-memory session directly (`ToolCtx` has no session access), so this is
    /// the side-channel (Phase E: `reason link` uses it).
    pub session_meta_mark: Option<serde_json::Value>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, session_meta_mark: None }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, session_meta_mark: None }
    }
    /// Attach a session-leaf meta mark (Phase E side-channel). Builder-style.
    pub fn with_session_meta_mark(mut self, mark: serde_json::Value) -> Self {
        self.session_meta_mark = Some(mark);
        self
    }
}
```

- [ ] **Step 2: Write the failing `ToolOutput` unit test**

Add to `src/tool/mod.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn session_meta_mark_defaults_none_and_builder_sets_it() {
        assert!(ToolOutput::ok("x").session_meta_mark.is_none());
        assert!(ToolOutput::err("x").session_meta_mark.is_none());
        let m = serde_json::json!({"causal_node": 5, "status": "hypothesis"});
        let o = ToolOutput::ok("linked").with_session_meta_mark(m.clone());
        assert_eq!(o.session_meta_mark, Some(m));
        assert!(!o.is_error);
    }
```

- [ ] **Step 3: Run — confirm it passes (field exists now)**

Run: `cargo test --lib tool::tests::session_meta_mark_defaults_none_and_builder_sets_it 2>&1 | tail -10`
Expected: PASS. (The rest of the build may fail at `dispatch_tool` if any constructor site is missing the new field — `ok`/`err` are the only constructors and they set it; verify no other `ToolOutput { ... }` literal exists. `grep -n "ToolOutput {" src/` to check.)

- [ ] **Step 4: Add `AgentLoop::apply_session_meta_mark` + write its test**

In `src/agent.rs`, add a private method to `impl AgentLoop` (near `append` or `load_self`):

```rust
    /// Apply a session-leaf meta mark from a tool's `ToolOutput.session_meta_mark`
    /// (Phase E side-channel — tools can't write the session directly). Writes the
    /// mark onto the current leaf's `SessionEntry.meta` and autosaves. No-op if
    /// there is no current leaf.
    fn apply_session_meta_mark(&mut self, mark: serde_json::Value) {
        if let Some(leaf) = self.session.leaf {
            self.session.update_meta(leaf, |m| *m = Some(mark));
            if self.persist {
                let _ = self.session.save(&self.session_path);
            }
        }
    }
```

Add a test in `src/agent.rs`'s `#[cfg(test)] mod tests` (the inline module can access private fields/methods):

```rust
    #[test]
    fn apply_session_meta_mark_writes_current_leaf_meta() {
        use crate::message::{Message, Role};
        let dir = std::env::temp_dir().join(format!("cc_metamark_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut agent = AgentLoop::new(
            std::sync::Arc::new(crate::provider::stub::StubClient),
            "gpt-4o", 4096, 0.7, dir.clone(),
        );
        // give the session a leaf entry (id 0)
        agent.session.append(Message::text(0, Role::User, "hi"));
        assert_eq!(agent.session.leaf, Some(0));

        let mark = serde_json::json!({"causal_node": 5, "status": "hypothesis"});
        agent.apply_session_meta_mark(mark.clone());

        let meta = agent.session.entry_by_id(0).unwrap().meta.clone();
        assert_eq!(meta, Some(mark), "leaf meta must equal the applied mark");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 5: Run — confirm it fails then passes**

Run: `cargo test --lib apply_session_meta_mark_writes_current_leaf_meta 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Wire `dispatch_tool` to apply the mark**

In `src/agent.rs::dispatch_tool`, the tool runs at (current code):

```rust
        let mut ctx = ToolCtx::with_cancel(&self.root, &self.cancel);
        let output = match self.toolbox.get(name).unwrap().run(args, &mut ctx) {
            Ok(o) => o,
            Err(e) => crate::tool::ToolOutput::err(format!("tool error: {e}")),
        };
```

Immediately AFTER that `let output = ...;` block (before the `ToolFinished` event send), add:

```rust
        if let Some(mark) = output.session_meta_mark.take() {
            self.apply_session_meta_mark(mark);
        }
```

(`.take()` leaves `output.session_meta_mark` as `None` so the mark isn't re-applied; the `ToolFinished` event below uses `output.content`/`output.is_error`, unaffected.)

- [ ] **Step 7: Build + full suite**

Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -20` → 0 failed, 3 ignored, full suite completes (no hang).

- [ ] **Step 8: Commit**

```bash
git add src/tool/mod.rs src/agent.rs
git commit -m "feat: ToolOutput.session_meta_mark side-channel (tools mark the session leaf)"
```

---

### Task 2: `reason link` action + `CausalNode.ruling` + `record_ruling` + list rendering

**Files:**
- Modify: `src/tool/reason.rs` — `link` action; `CausalNode.ruling`; `pub fn record_ruling`; `list`/`trace` show ruling
- Test: `src/tool/reason.rs` inline tests

**Interfaces:**
- Consumes: Task 1's `ToolOutput::with_session_meta_mark`; `CausalTree::load/save`.
- Produces (used by Task 3):
  - `reason` tool `link` action: `reason action=link id=<causal_node>` → `ToolOutput::ok(...).with_session_meta_mark({"causal_node": id, "status": "hypothesis"})` (or `err` if unknown node)
  - `CausalNode.ruling: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`)
  - `pub fn record_ruling(root: &Path, causal_node_id: u64, ruling: &str) -> anyhow::Result<()>` — loads `causal_tree.json`, sets the node's `ruling`, saves

- [ ] **Step 1: Add `CausalNode.ruling` (backward-compatible)**

In `src/tool/reason.rs`, add the field to `struct CausalNode` (after `terminal`):

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ruling: Option<String>,
```

(No migration — the file is unversioned; old files load with `ruling: None`.)

- [ ] **Step 2: Write the failing tests**

Add to `src/tool/reason.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn link_returns_session_meta_mark_for_known_node() {
        let dir = std::env::temp_dir().join(format!("cc_reasonlink_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = crate::tool::ToolCtx::new(&dir);
        // create a causal node first
        let reason = ReasonTool;
        let _ = reason.run(serde_json::json!({"action":"add","question":"why?"}), &mut ctx.clone());
        // link node 1
        let out = reason.run(serde_json::json!({"action":"link","id":1}), &mut ctx.clone());
        let out = out.unwrap();
        assert!(!out.is_error, "link on known node should succeed");
        assert_eq!(
            out.session_meta_mark,
            Some(serde_json::json!({"causal_node": 1, "status": "hypothesis"})),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_unknown_node_errors_without_mark() {
        let dir = std::env::temp_dir().join(format!("cc_reasonlink2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = crate::tool::ToolCtx::new(&dir);
        let out = ReasonTool.run(serde_json::json!({"action":"link","id":99}), &mut ctx).unwrap();
        assert!(out.is_error, "link on unknown node should error");
        assert!(out.session_meta_mark.is_none(), "no mark on error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_ruling_writes_to_causal_node() {
        let dir = std::env::temp_dir().join(format!("cc_recordruling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = crate::tool::ToolCtx::new(&dir);
        let _ = ReasonTool.run(serde_json::json!({"action":"add","question":"why?"}), &mut ctx);
        super::record_ruling(&dir, 1, "ruled out: too slow").unwrap();
        let tree = CausalTree::load(&dir);
        let node = tree.nodes.iter().find(|n| n.id == 1).unwrap();
        assert_eq!(node.ruling.as_deref(), Some("ruled out: too slow"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn causal_tree_loads_old_file_without_ruling() {
        // an old causal_tree.json with no `ruling` fields still loads
        let dir = std::env::temp_dir().join(format!("cc_oldruling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("causal_tree.json"),
            r#"{"nodes":[{"id":1,"question":"q","status":"hypothesis"}]}"#,
        ).unwrap();
        let tree = CausalTree::load(&dir);
        assert_eq!(tree.nodes.len(), 1);
        assert!(tree.nodes[0].ruling.is_none(), "old file: ruling defaults to None");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

> Adjust the `ReasonTool`/`ToolCtx` construction to match the actual struct names (read the top of `src/tool/reason.rs` — the tool struct + how tests construct it). If `ToolCtx::new(&dir)` needs a lifetime tweak, adapt. The `ctx.clone()` calls are because `run` takes `&mut ToolCtx`; if `ToolCtx` isn't `Clone`, construct fresh `ToolCtx::new(&dir)` per call instead.

- [ ] **Step 3: Run — confirm they fail**

Run: `cargo test --lib reason::tests 2>&1 | tail -20`
Expected: failures — `link` action unknown (`no match for "link"`), `record_ruling` undefined.

- [ ] **Step 4: Add `record_ruling` + the `link` action + list rendering**

In `src/tool/reason.rs`:

(a) Add a free function (near `CausalTree::save`):

```rust
/// Phase E: record a ruling on a causal node when a session branch exploring it
/// is abandoned. Loads `causal_tree.json`, sets the node's `ruling`, saves.
pub fn record_ruling(root: &std::path::Path, causal_node_id: u64, ruling: &str) -> anyhow::Result<()> {
    let mut tree = CausalTree::load(root);
    let node = tree.nodes.iter_mut().find(|n| n.id == causal_node_id)
        .ok_or_else(|| anyhow::anyhow!("unknown causal node #{causal_node_id}"))?;
    node.ruling = Some(ruling.to_string());
    tree.save(root)
}
```

(b) Add `"link"` to the action enum in the wire schema (the `"enum": [...]` array) and to the dispatch match:

```rust
            "link" => self.link(args, ctx),
```

and the method:

```rust
    fn link(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let id = args.get("id").and_then(Value::as_u64).unwrap_or(0);
        let tree = CausalTree::load(ctx.root);
        if !tree.nodes.iter().any(|n| n.id == id) {
            return Ok(ToolOutput::err(format!("unknown causal node #{id}")));
        }
        Ok(ToolOutput::ok(format!("session leaf linked to causal node #{id} (status: hypothesis)"))
            .with_session_meta_mark(serde_json::json!({"causal_node": id, "status": "hypothesis"})))
    }
```

(c) In `list`/`trace` rendering, where each node's line is built, append the ruling if present. Find the node-line construction (in `list`/`trace`/`render`); add:

```rust
        if let Some(r) = &n.ruling {
            // append to the node's rendered line
            line.push_str(&format!(" [ruled out: {r}]"));
        }
```

(Read the current rendering helper first; append the ruling suffix wherever the node's status/margin is rendered.)

- [ ] **Step 5: Run — confirm pass + full suite**

Run: `cargo test --lib reason::tests 2>&1 | tail -20` → all reason tests pass (the 4 new + existing).
Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -20` → 0 failed, 3 ignored, no hang.

- [ ] **Step 6: Commit**

```bash
git add src/tool/reason.rs
git commit -m "feat: reason link action + CausalNode.ruling + record_ruling + list rendering"
```

---

### Task 3: Navigate-abandon writes the structured ruling (session meta + causal node)

**Files:**
- Modify: `src/agent.rs` — Navigate handler's Phase C summary `Ok(summary) =>` arm writes the ruling to linked abandoned entries' meta + `reason::record_ruling`
- Test: `src/agent.rs` inline tests

**Interfaces:**
- Consumes: Task 2's `record_ruling`; `Session::update_meta` / `entry_by_id`.
- Produces: the Navigate abandon path now produces `ruled_out` session meta + causal `ruling` for abandoned entries that carry `meta.causal_node`.

- [ ] **Step 1: Write the failing test**

Add to `src/agent.rs`'s `#[cfg(test)] mod tests`. This test builds an AgentLoop whose session has a branch linked to a causal node, navigates away from it, and asserts the ruling is written to both session meta and causal node:

```rust
    #[test]
    fn navigate_abandon_records_ruling_on_linked_branch() {
        use crate::message::{Message, Role};
        let dir = std::env::temp_dir().join(format!("cc_navruling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // seed a causal tree with node 1
        std::fs::write(
            dir.join("causal_tree.json"),
            r#"{"nodes":[{"id":1,"question":"why slow?","status":"hypothesis"}]}"#,
        ).unwrap();

        let mut agent = AgentLoop::new(
            std::sync::Arc::new(crate::provider::stub::StubClient),
            "gpt-4o", 4096, 0.7, dir.clone(),
        );
        // build a session tree: root(0) -> A(1, linked to causal node 1) ; navigate to 0 then append B(2)
        agent.session.append(Message::text(0, Role::User, "root"));
        agent.session.append(Message {
            id: 1, role: Role::Assistant,
            items: vec![crate::message::MessageItem::Text { text: "trying hypothesis".into() }],
        });
        // mark entry 1's leaf as linked (simulate `reason link`)
        agent.session.leaf = Some(1);
        agent.session.update_meta(1, |m| *m = Some(serde_json::json!({"causal_node":1,"status":"hypothesis"})));
        // navigate to root (0): abandons entry 1
        let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
        agent.run_one_turn(String::new(), &tx); // ensure trust/cancel reset (no-op-ish); then Navigate:
        // Actually call Navigate directly via the command path:
        // run_one_turn drives a turn; for Navigate, exercise it through the same process_turn the agent uses.
        // Simplest: send AgentCommand::Navigate via a tiny run loop. But AgentLoop::run consumes self.
        // Instead, directly invoke the navigate logic by appending at root then asserting.
        // NOTE: see Step 2 for the precise way to drive Navigate in a test (it's an AgentCommand).
        let _ = rx;
        // After navigation away from entry 1 (see Step 2), entry 1's meta should be ruled_out:
        let m = agent.session.entry_by_id(1).unwrap().meta.clone();
        assert!(matches!(m, Some(v) if v.get("status") == Some(&serde_json::json!("ruled_out"))));
        let _ = std::fs::remove_dir_all(&dir);
    }
```

> **The test-driving detail (resolve in Step 2):** `AgentCommand::Navigate` is handled inside the run loop / `process_turn`, not as a directly-callable public method. To drive it in a test, either (a) add a thin test-only helper that invokes the Navigate handling, or (b) route through `run_one_turn` with a steering mechanism. The CLEANEST: read the Navigate handler — if its body can be extracted into a small `fn handle_navigate(&mut self, id, event_tx)` method, do that extraction as part of this task (it's a targeted refactor that improves testability, in the spirit of "improve code you're working in"), then the test calls `agent.handle_navigate(0, &tx)` and asserts. If extraction is awkward, drive it via a minimal run loop with `cmd_tx.send(AgentCommand::Navigate(0))`. The invariant the test verifies: after navigating away from a linked leaf, entry 1's meta is `{status: "ruled_out", ruling: ...}` AND `causal_tree.json`'s node 1 has `ruling == Some(...)`.

- [ ] **Step 2: Resolve the test-driving + run — confirm it fails**

Read the Navigate handler (`src/agent.rs`, the `AgentCommand::Navigate(id) =>` arm). Decide the cleanest test drive (extract `handle_navigate` OR a minimal run loop). Implement the test accordingly. Run: `cargo test --lib navigate_abandon_records_ruling_on_linked_branch 2>&1 | tail -15` → expected FAIL (the ruling write isn't there yet).

- [ ] **Step 3: Add the ruling write to the Navigate handler's Phase C summary arm**

In `src/agent.rs`, the Navigate handler's Phase C block currently does (inside the `Ok(summary) =>` arm):

```rust
                                    Ok(summary) => {
                                        let _ = event_tx.send(AgentEvent::Notice(
                                            format!("abandoned branch summarized: {summary}"),
                                        ));
                                    }
```

Replace that arm with ruling-writing logic — for each abandoned entry whose meta has a `causal_node`, write the ruling to session meta + causal node:

```rust
                                    Ok(summary) => {
                                        let _ = event_tx.send(AgentEvent::Notice(
                                            format!("abandoned branch summarized: {summary}"),
                                        ));
                                        // Phase E: structured ruling on linked causal nodes.
                                        for entry_id in &abandoned {
                                            let causal_node = self.session.entry_by_id(*entry_id)
                                                .and_then(|e| e.meta.as_ref())
                                                .and_then(|m| m.get("causal_node"))
                                                .and_then(|v| v.as_u64());
                                            if let Some(cn) = causal_node {
                                                let s = summary.clone();
                                                self.session.update_meta(*entry_id, |m| {
                                                    let obj = m.get_or_insert(serde_json::json!({}));
                                                    if let Some(o) = obj.as_object_mut() {
                                                        o.insert("status".into(), "ruled_out".into());
                                                        o.insert("ruling".into(), s.into());
                                                    }
                                                });
                                                if let Err(e) = crate::tool::reason::record_ruling(&self.root, cn, &summary) {
                                                    let _ = event_tx.send(AgentEvent::Notice(
                                                        format!("causal ruling write failed for node #{cn}: {e}"),
                                                    ));
                                                }
                                            }
                                        }
                                    }
```

(The `abandoned` Vec<MessageId> is already in scope in this block. The navigate_to + autosave that follows persists the updated session meta. The session-meta `update_meta` is best-effort; the causal `record_ruling` failure is logged via Notice, not fatal.)

- [ ] **Step 4: Run — confirm pass + full suite**

Run: `cargo test --lib navigate_abandon_records_ruling_on_linked_branch 2>&1 | tail -15` → PASS.
Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -20` → 0 failed, 3 ignored, no hang.

- [ ] **Step 5: Commit**

```bash
git add src/agent.rs
git commit -m "feat: Navigate-abandon writes structured ruling to session meta + causal node"
```

---

### Task 4: `cc tree` + `reason list` surface the linkage (TreeNode + print_tree)

**Files:**
- Modify: `src/daemon/proto.rs` — `TreeNode.causal_node` / `status`
- Modify: `src/daemon/socket.rs` — `build_tree_nodes` extracts causal_node/status from meta
- Modify: `src/client/mod.rs` — `print_tree` renders `→H#M (hypothesis)` / `(✗ ruled_out)`
- Modify: `skills/debug-causal.md` — add a "link the branch" step
- Test: `src/daemon/socket.rs` + `src/client/mod.rs` inline tests

**Interfaces:**
- Consumes: the meta shape `{causal_node, status, ruling?}` from Tasks 1–3.
- Produces: `TreeNode { ..., causal_node: Option<u64>, status: Option<String> }`; `print_tree` markers.

- [ ] **Step 1: Add `causal_node` / `status` to `TreeNode`**

In `src/daemon/proto.rs`, add two fields to `TreeNode`:

```rust
    pub causal_node: Option<u64>,
    pub status: Option<String>,   // "hypothesis" | "ruled_out" | "locked"
```

(`#[serde(default)]` so existing `TreeNode` serde tests / payloads without them still parse — add `#[serde(default)]` to both fields.)

- [ ] **Step 2: Write the failing tests**

(a) `src/daemon/socket.rs` — extend an existing `build_tree_nodes`-driven test (or add a unit test on `build_tree_nodes` if it's testable in isolation). Simplest: add a `proto` round-trip asserting the new fields serialize, AND a `socket` test that a session with linked meta surfaces `causal_node`/`status` in the `Tree` response. Add to `src/daemon/socket.rs` tests a variant of `treeshow_returns_tree_with_active_path` that writes a session entry with `meta = {"causal_node":3,"status":"hypothesis"}` and asserts the returned `TreeNode` has `causal_node == Some(3)` + `status == Some("hypothesis")`.

(b) `src/client/mod.rs` — extend `print_tree_marks_active_path_leaf_and_abandoned` (or add a new test) with a node carrying `causal_node: Some(5), status: Some("hypothesis")` and assert the rendered string contains `→H#5` (and a `ruled_out` node shows `✗`).

- [ ] **Step 3: Run — confirm fail**

Run: `cargo test --lib daemon::socket::tests client::tests 2>&1 | tail -20` → the new assertions fail (fields not extracted / rendered).

- [ ] **Step 4: Extract in `build_tree_nodes` + render in `print_tree`**

(a) `src/daemon/socket.rs::build_tree_nodes` — when building each `TreeNode`, extract from the entry's meta:

```rust
        let (causal_node, status) = match &e.meta {
            Some(m) => (
                m.get("causal_node").and_then(|v| v.as_u64()),
                m.get("status").and_then(|v| v.as_str()).map(String::from),
            ),
            None => (None, None),
        };
        // ... add causal_node, status to the TreeNode { ... }
```

(b) `src/client/mod.rs::print_tree` — extend the node line. Where it currently does `format!("{indent}{prefix} [{id}] {role}: {preview}\n", ...)`, append a causal marker when present:

```rust
        let causal = match (n.causal_node, n.status.as_deref()) {
            (Some(cn), Some("ruled_out")) => format!(" (✗H#{cn} ruled_out)"),
            (Some(cn), Some(st)) => format!(" (→H#{cn} {st})"),
            (Some(cn), None) => format!(" (→H#{cn})"),
            _ => String::new(),
        };
        // line becomes: format!("{indent}{prefix} [{id}] {role}: {preview}{causal}\n", ...)
```

(c) `skills/debug-causal.md` — add a step after "逐节点展开":

```markdown
3. **链接会话分支**: 当你在一个 session 分支里探一个假设时，`reason link id=<causal_node>` 把当前分支标记为「探索该 causal 节点」。离开该分支时（`cc fork <祖先>`）会自动记录排除裁决。
```

(Place it as step 3, renumber the following steps.)

- [ ] **Step 5: Run — confirm pass + full suite**

Run: `cargo test --lib daemon::socket::tests client::tests 2>&1 | tail -20` → PASS.
Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -20` → 0 failed, 3 ignored, no hang.

- [ ] **Step 6: Commit**

```bash
git add src/daemon/proto.rs src/daemon/socket.rs src/client/mod.rs skills/debug-causal.md
git commit -m "feat: cc tree + reason list surface causal-node linkage and rulings"
```

---

## Self-Review (run after writing — notes for the implementer)

- **Spec coverage:** every spec section maps to a task — `ToolOutput.session_meta_mark` + `dispatch_tool` applies (Task 1); `reason link` + `CausalNode.ruling` + `record_ruling` + `list`/`trace` (Task 2); Navigate-abandon双写 ruling (Task 3); `TreeNode`/`build_tree_nodes`/`print_tree` surface + `debug-causal` skill update (Task 4). Error handling (unknown node → err; no causal link → summary only; causal file missing → skip; no leaf → skip) covered across Tasks 1–3. Testing: all 7 spec test categories present (ToolOutput mark, dispatch applies, reason link ok/err, record_ruling, Navigate双写, build_tree_nodes/print_tree surface, backward-compat load).
- **Type consistency:** `ToolOutput.session_meta_mark: Option<Value>` (Task 1) matches `reason link`'s `with_session_meta_mark(json!({...}))` (Task 2) and `dispatch_tool`'s `.take()` (Task 1). `record_ruling(root, id, &str)` (Task 2) matches the Navigate handler call (Task 3). `TreeNode.causal_node/status` (Task 4) match `build_tree_nodes` extraction + `print_tree` rendering. The meta shape `{causal_node, status, ruling?}` is consistent across `reason link` (writes causal_node+status), Navigate handler (writes status=ruled_out+ruling), and `build_tree_nodes` (reads causal_node+status).
- **No placeholders:** every step has complete code or exact commands. (Task 3 Step 1 flags the test-driving decision — extract `handle_navigate` vs minimal run loop — and specifies the invariant; Task 4 Step 2 says "model on the existing treeshow test." Both are concrete instructions, not "TODO.")
- **Known follow-ups (not blocking):** the ruling is the auto tier-2 summary (not a user-authored richer reason); linking is explicit (`reason link`, no auto-detection). Both are documented spec limitations.
