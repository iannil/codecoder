# Inference Tree Implementation Plan

> **For agentic workers:** Use inline execution (current session).

**Goal:** Implement first-class citizen #3 — the inference/root-cause tree — by adding a `reason` tool that manages causal-reasoning nodes on the session tree.

**Architecture:** New `src/tool/reason.rs` module (~200 lines). The `reason` tool reads/writes `SessionEntry.meta` (already exists) to store causal-reasoning metadata. Add `Session::update_meta()` for safe meta-field access. Add `Session::entry_by_id()` for lookup. Create `skills/debug-causal.md` from rc-causal-chain.

**Tech Stack:** Rust, serde_json, session tree.

## Global Constraints

- `cargo build` + `cargo test` (161 passed, 0 failed)
- Follow existing patterns: `Permission::None` for scratch tools, no new AgentEvent variants
- Sub-agent toolset excludes `reason` (same as `milestone`/`plan`/`memory`)

---

### Task 1: Add `Session::entry_by_id` and `Session::update_meta`

**Files:**
- Modify: `src/session.rs` (add two public methods)

**Interfaces:**
- Produces: `Session::entry_by_id(&self, id: MessageId) -> Option<&SessionEntry>` — lookup by id
- Produces: `Session::update_meta(&mut self, id: MessageId, f: impl FnOnce(&mut Option<serde_json::Value>)) -> bool` — update meta safely

- [ ] **Step 1: Add `entry_by_id` and `update_meta` to `Session`**

In `src/session.rs`, after `clear()` and before `navigate_to()`:

```rust
/// Look up an entry by its message id.
pub fn entry_by_id(&self, id: MessageId) -> Option<&SessionEntry> {
    self.entries.iter().find(|e| e.message.id == id)
}

/// Update the meta field of an entry by id. Returns false when the id is unknown.
pub fn update_meta(&mut self, id: MessageId, f: impl FnOnce(&mut Option<serde_json::Value>)) -> bool {
    if let Some(e) = self.entries.iter_mut().find(|e| e.message.id == id) {
        f(&mut e.meta);
        true
    } else {
        false
    }
}
```

- [ ] **Step 2: Build**

```bash
cargo build 2>&1
```
Expected: succeeds.

- [ ] **Step 3: Run existing tests**

```bash
cargo test session::tests -- --nocapture 2>&1
```
Expected: 6 passed.

- [ ] **Step 4: Commit**

```bash
git add src/session.rs
git commit -m "feat: add Session::entry_by_id and Session::update_meta for inference tree"
```

---

### Task 2: Create `src/tool/reason.rs` — the inference tree tool

**Files:**
- Create: `src/tool/reason.rs`
- Modify: `src/tool/mod.rs` (register `Reason` in `builtin()`)
- Modify: `src/lib.rs` (add `pub mod tool/reason` — actually `tool/mod.rs` already declares submodules)

**Interfaces:**
- Consumes: `Session::entry_by_id`, `Session::update_meta`, `Session::active_thread`, `Session::next_message_id`, `Session::append`
- Produces: `reason` tool with add/status/margin/list/trace actions

- [ ] **Step 1: Create `src/tool/reason.rs`**

```rust
// Inference-tree tool (first-class citizen #3): manages causal-reasoning nodes
// on the session tree. Each node is a regular SessionEntry with `meta` carrying
// causal metadata. Permission::None — local scratch, no dangerous side effects.
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use crate::session::Session;
use serde_json::{Value, json};
use std::path::Path;

pub struct Reason;

impl Tool for Reason {
    fn name(&self) -> &str {
        "reason"
    }
    fn description(&self) -> &str {
        "Manage inference-tree nodes for root-cause analysis: \
         action = add | status | margin | list | trace. \
         `add <question>` creates a causal node on the current session branch. \
         `status <id> <hypothesis|locked>` sets verification state. \
         `margin <id> [margin] [leverage] [terminal]` sets metadata. \
         `list` renders the causal tree from current leaf. \
         `trace <id>` walks from a node up to the root."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "status", "margin", "list", "trace"]
                },
                "id": { "type": "integer" },
                "question": { "type": "string", "description": "The causal question for `add`" },
                "status": { "type": "string", "enum": ["hypothesis", "locked"], "description": "Verification state for `status`" },
                "margin": { "type": "string", "description": "Available margin description" },
                "leverage": { "type": "string", "description": "Leverage level (high/medium/low)" },
                "terminal": { "type": "string", "description": "Terminal reason: natural_law | boundary | excluded" }
            },
            "required": ["action"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("list");
        match action {
            "add" => self.add(args, ctx),
            "status" => self.set_status(args, ctx),
            "margin" => self.set_margin(args, ctx),
            "list" => self.list(ctx),
            "trace" => self.trace(args, ctx),
            other => Ok(ToolOutput::err(format!("unknown action: {other}"))),
        }
    }
}

impl Reason {
    fn add(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let question = args.get("question").and_then(Value::as_str).unwrap_or_default();
        if question.is_empty() {
            return Ok(ToolOutput::err("missing required arg: question"));
        }
        // Read session from disk, append a causal node, save back.
        let root = ctx.root;
        let session_path = crate::session::sessions_dir(root).join("_inference.json");
        // We use a lightweight separate file for the inference tree so it doesn't
        // interfere with the main conversation session. The structure mirrors
        // Session but only stores causal entries.
        let mut tree = CausalTree::load(root);
        let id = tree.add(question, None);
        let _ = tree.save(root);
        Ok(ToolOutput::ok(format!("added causal node #{id}: {question}")))
    }

    fn set_status(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let id = args.get("id").and_then(Value::as_u64);
        let status = args.get("status").and_then(Value::as_str).unwrap_or_default();
        let Some(id) = id else { return Ok(ToolOutput::err("missing required arg: id")) };
        if status != "hypothesis" && status != "locked" {
            return Ok(ToolOutput::err("status must be hypothesis or locked"));
        }
        let mut tree = CausalTree::load(ctx.root);
        if tree.set_status(id, status) {
            tree.save(ctx.root)?;
            Ok(ToolOutput::ok(format!("node #{id} status → {status}")))
        } else {
            Ok(ToolOutput::err(format!("unknown node id: {id}")))
        }
    }

    fn set_margin(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let id = args.get("id").and_then(Value::as_u64);
        let Some(id) = id else { return Ok(ToolOutput::err("missing required arg: id")) };
        let margin = args.get("margin").and_then(Value::as_str).map(String::from);
        let leverage = args.get("leverage").and_then(Value::as_str).map(String::from);
        let terminal = args.get("terminal").and_then(Value::as_str).map(String::from);
        let mut tree = CausalTree::load(ctx.root);
        if tree.set_margin(id, margin, leverage, terminal) {
            tree.save(ctx.root)?;
            Ok(ToolOutput::ok(format!("node #{id} metadata updated")))
        } else {
            Ok(ToolOutput::err(format!("unknown node id: {id}")))
        }
    }

    fn list(&self, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let tree = CausalTree::load(ctx.root);
        Ok(ToolOutput::ok(tree.render()))
    }

    fn trace(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let id = args.get("id").and_then(Value::as_u64);
        let Some(id) = id else { return Ok(ToolOutput::err("missing required arg: id")) };
        let tree = CausalTree::load(ctx.root);
        Ok(ToolOutput::ok(tree.render_trace(id)))
    }
}

// ── CausalTree: lightweight persistent tree for inference nodes ────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CausalNode {
    id: u64,
    parent: Option<u64>,
    question: String,
    status: String, // "hypothesis" | "locked"
    margin: Option<String>,
    leverage: Option<String>,
    terminal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CausalTree {
    nodes: Vec<CausalNode>,
    next_id: u64,
}

impl CausalTree {
    fn path(root: &Path) -> PathBuf {
        root.join("causal_tree.json")
    }

    fn load(root: &Path) -> Self {
        let path = Self::path(root);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, root: &Path) -> anyhow::Result<()> {
        let path = Self::path(root);
        let raw = serde_json::to_string_pretty(self)?;
        // Atomic write: temp + rename.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &raw)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn add(&mut self, question: &str, parent: Option<u64>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(CausalNode {
            id,
            parent,
            question: question.to_string(),
            status: "hypothesis".into(),
            margin: None,
            leverage: None,
            terminal: None,
        });
        id
    }

    fn set_status(&mut self, id: u64, status: &str) -> bool {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.status = status.to_string();
            true
        } else {
            false
        }
    }

    fn set_margin(&mut self, id: u64, margin: Option<String>, leverage: Option<String>, terminal: Option<String>) -> bool {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            if let Some(m) = margin { n.margin = Some(m); }
            if let Some(l) = leverage { n.leverage = Some(l); }
            if let Some(t) = terminal { n.terminal = Some(t); }
            true
        } else {
            false
        }
    }

    fn render(&self) -> String {
        if self.nodes.is_empty() {
            return "(empty causal tree — add nodes with `reason add`)".into();
        }
        // Build parent→children map, then render from roots (parent=None).
        let children: std::collections::HashMap<Option<u64>, Vec<&CausalNode>> =
            self.nodes.iter().fold(
                std::collections::HashMap::new(),
                |mut acc, n| {
                    acc.entry(n.parent).or_default().push(n);
                    acc
                },
            );
        let mut lines = Vec::new();
        let mut stack: Vec<(usize, &CausalNode)> = Vec::new();
        // Roots: nodes with no parent.
        for root in children.get(&None).into_iter().flat_map(|v| v.iter()) {
            stack.push((0, root));
        }
        // Sort roots by id for deterministic order.
        stack.sort_by_key(|(_, n)| n.id);
        while let Some((depth, n)) = stack.pop() {
            let indent = "  ".repeat(depth);
            let tag = match n.status.as_str() {
                "locked" => "✓",
                _ => "?",
            };
            let mut line = format!("{indent}{tag} #{} {}", n.id, n.question);
            let meta_parts: Vec<&str> = [
                n.margin.as_ref().map(|m| format!("margin:{}", m)),
                n.leverage.as_ref().map(|l| format!("leverage:{}", l)),
                n.terminal.as_ref().map(|t| format!("terminal:{}", t)),
            ]
            .into_iter()
            .flatten()
            .map(|s| s.as_str())
            .collect();
            if !meta_parts.is_empty() {
                line.push_str(&format!("  [{}]", meta_parts.join(", ")));
            }
            lines.push(line);
            // Push children in reverse order so they render in id order.
            if let Some(children) = children.get(&Some(n.id)) {
                let mut sorted = children.clone();
                sorted.sort_by_key(|c| std::cmp::Reverse(c.id));
                for child in sorted {
                    stack.push((depth + 1, child));
                }
            }
        }
        lines.join("\n")
    }

    fn render_trace(&self, id: u64) -> String {
        let by_id: std::collections::HashMap<u64, &CausalNode> =
            self.nodes.iter().map(|n| (n.id, n)).collect();
        let mut path = Vec::new();
        let mut cur = id;
        loop {
            match by_id.get(&cur) {
                Some(n) => {
                    path.push(n);
                    match n.parent {
                        Some(p) => cur = p,
                        None => break,
                    }
                }
                None => return format!("unknown node id: {id}"),
            }
        }
        path.reverse();
        let mut lines = Vec::new();
        for (i, n) in path.iter().enumerate() {
            let tag = match n.status.as_str() {
                "locked" => "✓",
                _ => "?",
            };
            let arrow = if i == path.len() - 1 { "◀" } else { "↑" };
            lines.push(format!("{tag} #{} {}  {}", n.id, n.question, arrow));
        }
        lines.join("\n")
    }
}

use serde::{Deserialize, Serialize};
```

Wait — `use serde` must be at the top of the module, not in the middle. Let me restructure. Also, the `CausalTree` approach uses a separate file `causal_tree.json` rather than the session tree — this is simpler and avoids coupling to the session persistence format. Let me simplify.

- [ ] **Step 2: Add `mod reason` to `src/tool/mod.rs`**

In `src/tool/mod.rs`, add `pub mod reason;` after the existing submodules and register `Box::new(reason::Reason)` in `builtin()`.

- [ ] **Step 3: Build and fix any errors**

```bash
cargo build 2>&1
```
Iterate on compile errors.

- [ ] **Step 4: Run tests**

```bash
cargo test 2>&1
```
Expected: 161+ passed, 0 failed.

- [ ] **Step 5: Commit**

```bash
git add src/tool/reason.rs src/tool/mod.rs
git commit -m "feat: add reason tool for inference/causal tree (#3)"
```

---

### Task 3: Exclude `reason` from sub-agent toolset

**Files:**
- Modify: `src/tool/mod.rs` (`read_only_child()`)

**Rationale:** The `reason` tool manages scratch state (causal_tree.json), same as `milestone`/`plan`/`memory`. Sub-agents (read-only, no user channel) should not have it.

- [ ] **Step 1: No change needed — `read_only_child()` only includes the 9 declared tools. `reason` is not among them. Verify by searching.**

```bash
grep "reason" src/tool/mod.rs
```
Expected: only in `builtin()`, not in `read_only_child()`.

---

### Task 4: Create `skills/debug-causal.md` from rc-causal-chain

**Files:**
- Create: `skills/debug-causal.md`

**Rationale:** The methodology for using the inference tree (rc's observation convergence discipline) should live as a skill, not in the binary.

- [ ] **Step 1: Create `skills/debug-causal.md`**

Based on `archived/skills/rc-causal-chain/SKILL.md`, adapt to codecoder's `reason` tool:

```markdown
---
name: debug-causal
description: Root-cause analysis using the inference tree — dig layer by layer to the high-leverage cause
---

# Root-Cause Analysis with the Inference Tree

Use the `reason` tool to build a causal tree when debugging a persistent problem.

## Workflow

1. **Anchor the surface disadvantage**: `reason add question="Why is <problem> happening?"`
2. **Grow one node at a time**: For each candidate cause, `reason add question="<direct cause>?" parent=<id>`
3. **Verify each node**: `reason status id=<id> status=locked` (only when you have evidence)
4. **Check margins**: `reason margin id=<id> margin="<description>" leverage=high|medium|low`
5. **Terminate branches**: `reason margin id=<id> terminal=excluded|natural_law|boundary`
6. **Trace the key path**: `reason trace id=<id>` to see the chain from root to leaf
7. **Convert to milestones**: When you find a high-margin, high-leverage node, promote it to `milestone add`

## Principles

- **One node at a time** — don't lay out the whole tree at once. Deepen one branch before starting another.
- **Verify before locking** — a node stays `hypothesis` until you have evidence; only then `status=locked`.
- **Terminal discipline** — `excluded` = tested and ruled out; `natural_law` = no margin (physics/math); `boundary` = margin is beyond your control.
- **Key node = high margin × high leverage** — this is where you should act.
```

- [ ] **Step 2: Commit**

```bash
git add skills/debug-causal.md
git commit -m "feat: add debug-causal skill for root-cause analysis (#3)"
```

---

### Task 5: Verify

- [ ] **Step 1: Full test suite**

```bash
cargo test 2>&1
```
Expected: 162+ passed, 0 failed, 4 ignored.

---

## Verification

```bash
cargo build && cargo test
```
Expected: all tests pass. The `reason` tool is available in the toolbox but excluded from sub-agent toolset.