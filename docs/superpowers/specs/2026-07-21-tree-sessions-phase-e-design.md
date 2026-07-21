# Tree Sessions Phase E — Reasoning-Tree Integration (Design Spec)

**Date:** 2026-07-21
**Branch:** `feat/tree-sessions-phase-e`
**Status:** Approved (brainstormed 2026-07-21)
**Related:** `docs/design/2026-07-19-tree-sessions.md` (Phase E — reasoning-tree semantics), ADR 0023 (compaction tier-2), the `reason` tool (`src/tool/reason.rs`), the `debug-causal` skill (`skills/debug-causal.md`), Phase B (`docs/superpowers/specs/2026-07-21-tree-sessions-phase-b-design.md`)

## Goal

Unify the **session tree** (conversation branches, Phases A–D) with the **causal tree** (`reason` tool's per-project `causal_tree.json`). Today these are disconnected: the `reason` tool + `debug-causal` skill deliver a standalone reasoning tree, but when an agent forks a session branch to test a hypothesis, that branch carries no link to the causal node it explores, and abandoning the branch records no structured "this cause was ruled out because…" ruling. Phase E connects them: a session branch can be linked to the causal node it explores, and abandoning a linked branch writes a structured ruling to **both** the session entry's meta and the causal node — so the agent (and user) never re-explores a ruled-out dead end, and the two trees stay coherent.

## Background / Current State

- `src/tool/reason.rs`: the `reason` tool manages `causal_tree.json` (per-project). Actions: `add` (node with question/parent), `status` (hypothesis/locked), `margin` (margin/leverage/terminal), `list`, `trace`, `to_milestone`. `CausalNode { id, question, parent, status, margin, leverage, terminal, ... }` — NO `ruling` field yet. `CausalTree::load/save` — **no schema versioning** (plain serialize).
- `src/session.rs`: `SessionEntry { message, parent, meta: Option<serde_json::Value> }`. `Session::update_meta(id, f)` exists. `meta` is already read by `collect_cross_session_hypotheses` (filters `meta.status == "hypothesis"`).
- `src/agent.rs`: `AgentCommand::Navigate(id)` handler (line ~456) does Phase C abandoned-branch summarization (tier-2 `summarize_span`) + autosaves (added in Phase B's fix). It has `&mut self.session` AND `self.root` (so it can read/write `causal_tree.json`). `dispatch_tool` (line ~909) constructs `ToolCtx::with_cancel(&self.root, &self.cancel)` — tools do NOT get session access.
- `src/tool/mod.rs`: `ToolOutput { content, is_error }`; `::ok`/`::err` constructors.
- `skills/debug-causal.md`: drives the root-cause workflow via the `reason` tool.
- Phase B: `cc tree` / `cc fork <id>` / `cc clone` + `TreeNode` + `print_tree`.

**The constraint that shaped this design:** tools cannot write the in-memory `Session` (`ToolCtx` has only `root`/`cancel`); file-level session writes get clobbered by the next autosave. So a tool that wants to mark the current session leaf must do it via a value returned to `dispatch_tool`, which applies it (it holds `self.session`).

## Non-goals

- Replacing the `reason` tool's `causal_tree.json` with session-tree-based reasoning. The two trees stay distinct; Phase E only LINKS them and propagates rulings. (YAGNI — the `reason` tool is a good focused artifact.)
- Automatic hypothesis detection (the agent auto-marking nodes without calling `reason link`). Linking is explicit (agent or user driven).
- Prompting the user for a ruling on abandon (Approach 2 from brainstorming — rejected as disruptive). The ruling is auto-derived from the Phase C tier-2 summary.
- A graphical reasoning-tree view. `cc tree` shows text markers only.
- Phase D (typed entries / `ModelChange`).

## Design — Approach 1': tool-returns-meta-mark, dispatch_tool applies

### `ToolOutput` gains `session_meta_mark` (`src/tool/mod.rs`)

```rust
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Optional meta to write onto the current session leaf's `SessionEntry.meta`.
    /// Applied by `AgentLoop::dispatch_tool` after the tool runs (tools can't
    /// touch the in-memory session directly; this is the side-channel).
    #[serde(skip)] // ToolOutput isn't serialized, but be explicit
    pub session_meta_mark: Option<serde_json::Value>,
}
```

`ToolOutput::ok` / `::err` set `session_meta_mark: None`. Add a builder `ToolOutput::ok(content).with_session_meta_mark(value)` (or a constructor) for the `reason link` action to use.

### `dispatch_tool` applies the mark (`src/agent.rs`)

After `let result = tool.run(args, &mut ctx)?;` (inside `dispatch_tool`), before returning the `ToolOutcome`:

```rust
if let Some(mark) = result.session_meta_mark.take() {
    if let Some(leaf) = self.session.leaf {
        self.session.update_meta(leaf, |m| *m = Some(mark.clone()));
        if self.persist {
            let _ = self.session.save(&self.session_path);
        }
    }
}
```

(Best-effort: no leaf → skip. Autosave so the mark survives.)

### `reason link node=<id>` action (`src/tool/reason.rs`)

Add `link` to the action enum + the wire schema + the dispatch match. It verifies the causal node exists in `causal_tree.json`, then returns a ToolOutput whose `session_meta_mark = Some(json!({"causal_node": id, "status": "hypothesis"}))` plus a confirmation message. If the node doesn't exist → `ToolOutput::err("unknown causal node #N")` (no mark).

### Session-leaf meta shape

```json
{ "causal_node": <u64>, "status": "hypothesis" | "ruled_out", "ruling"?: <string> }
```

`reason link` writes `{causal_node, status:"hypothesis"}`. The Navigate-abandon path (below) updates to `{..., status:"ruled_out", ruling:<summary>}`.

### `CausalNode.ruling` field (`src/tool/reason.rs`)

```rust
struct CausalNode {
    // ...existing fields...
    /// Set when a session branch exploring this node was abandoned (Phase E):
    /// the tier-2 summary of the abandoned branch becomes the ruling.
    #[serde(default)]
    ruling: Option<String>,
}
```

`#[serde(default)]` keeps old `causal_tree.json` files loadable (no schema-version migration needed — the file is unversioned).

### Phase C abandon → structured ruling (`src/agent.rs` Navigate handler)

Today the Navigate handler, when a branch is abandoned, summarizes it via tier-2 and emits a `Notice`. Enhance: after the summary, if the abandoned branch's entries include one whose `meta.causal_node` is set, write the ruling BOTH places:

1. **Session side:** `self.session.update_meta(<that entry's id>, |m| { if let Some(obj) = m.as_object_mut() { obj.insert("status", "ruled_out".into()); obj.insert("ruling", summary.clone().into()); } })` + autosave.
2. **Causal side:** call a new `reason` module function `record_ruling(root, causal_node_id, ruling)` that loads `causal_tree.json`, sets `node.ruling = Some(ruling)`, saves. Best-effort (file missing/corrupt → skip, log via Notice).

(Only ONE causal_node per abandoned branch is the expected case — a branch explores one hypothesis. If multiple, record the ruling on each's causal node.)

### `cc tree` surfaces the link (`src/daemon/proto.rs` + `socket.rs` + `client/mod.rs`)

`TreeNode` gains two optional fields:
```rust
pub causal_node: Option<u64>,
pub status: Option<String>,   // "hypothesis" | "ruled_out" | "locked"
```
`build_tree_nodes` (`src/daemon/socket.rs`) extracts them from each entry's `meta` (`meta.causal_node`, `meta.status`). `print_tree` (`src/client/mod.rs`) renders: a linked node shows `→H#M` + status (`(hypothesis)` / `(✗ ruled_out)`). Abandoned-ruled-out nodes keep their existing ` ` (space) prefix and gain the `✗` marker.

### `reason list` / `trace` surface the ruling (`src/tool/reason.rs`)

When rendering a causal node that has `ruling: Some(...)`, append ` [ruled out: <ruling>]` to its line. So `reason list` shows which hypotheses were explored and ruled out (and why).

### Data flow

1. Agent debugs: `reason add`/`status`/`margin`/`trace` build `causal_tree.json`.
2. Agent explores causal-node N in the current session branch: `reason link node=N` → `dispatch_tool` writes `{causal_node:N, status:"hypothesis"}` to the current leaf meta (autosaved).
3. User/agent forks away (`cc fork <ancestor>`): Navigate handler Phase C-summarizes the abandoned branch → sees the abandoned entry's `meta.causal_node` → writes `ruled_out`+ruling to session meta + `ruling` to causal node N → `Notice`.
4. `cc tree`: TreeNode carries causal_node/status; `print_tree` shows `→H#M (hypothesis)` / `(✗ ruled_out)`.
5. `reason list`: causal node N shows `[ruled out: <ruling>]`.

## Error handling

- `reason link` unknown node → `err` (no mark written).
- Navigate abandon with no `meta.causal_node` on the abandoned branch → existing Phase C summary only (no ruling write). The common case.
- `causal_tree.json` missing/corrupt during abandon → skip the causal-side ruling write (best-effort); session-side summary + meta update still happen. Emit a Notice if the causal write fails.
- `session_meta_mark` returned but no current leaf (`self.session.leaf == None`) → skip (best-effort).
- `CausalNode.ruling` absent in old files → `#[serde(default)]` → loads as `None`.

## Testing

- `tool::tests`: `ToolOutput::ok(x).with_session_meta_mark(v)` carries the mark; `ok`/`err` default to `None`.
- `agent::tests`: a fake tool returns a `session_meta_mark`; after `dispatch_tool`, assert the current leaf's `meta` == the mark (+ autosaved). No leaf → mark skipped.
- `tool::tests` (reason): `link` with an existing node → ToolOutput has the mark `{causal_node, status:"hypothesis"}`; unknown node → `err`, no mark.
- `agent::tests`: Navigate abandoning a branch whose leaf-meta has `causal_node` → after Navigate, that entry's meta is `{status:"ruled_out", ruling:<summary>}` AND `causal_tree.json`'s node has `ruling == Some(<summary>)`. Navigate with no causal link → no ruling write (just the summary).
- `socket` test: `build_tree_nodes` extracts `causal_node`/`status` from a session with meta; `cc tree` (via the existing Phase B test harness) shows the marker.
- `client::tests`: `print_tree` renders `→H#M (hypothesis)` / `(✗ ruled_out)` for linked nodes.
- `tool::tests` (reason): `list`/`trace` output includes `[ruled out: …]` for a node with `ruling`.
- `tool::tests` (reason): an old `causal_tree.json` WITHOUT `ruling` fields still loads (backward compat).
- Existing suite stays green; `ToolOutput` field addition ripples to constructors/callers (all `::ok`/`::err` still work; the new field defaults to `None`).

## Known limitations (acceptance)

- Linking is explicit (`reason link`) — the agent must call it (the `debug-causal` skill guides when). No auto-detection of "this branch is a hypothesis exploration."
- One ruling per abandoned branch is the expected case; multiple `causal_node` links in one branch record the ruling on each (rare).
- The ruling is the tier-2 summary (auto), not a user-authored reason. Good enough for "don't re-explore this dead end"; a prompted richer ruling is a follow-up.

## Interfaces (contract for the implementation plan)

- `tool::ToolOutput { content, is_error, session_meta_mark: Option<serde_json::Value> }`; `ToolOutput::with_session_meta_mark(self, v) -> Self`.
- `AgentLoop::dispatch_tool` applies `result.session_meta_mark` to the current leaf.
- `reason` tool: new `link` action; `CausalNode.ruling: Option<String>` (`#[serde(default)]`).
- `reason::record_ruling(root, causal_node_id, ruling) -> anyhow::Result<()>` (loads/save `causal_tree.json`).
- `AgentLoop` Navigate handler: on abandon, if abandoned entry has `meta.causal_node`, write session-meta ruling + `reason::record_ruling`.
- `proto::TreeNode { ..., causal_node: Option<u64>, status: Option<String> }`; `build_tree_nodes` extracts from meta; `print_tree` renders markers.
- `reason` `list`/`trace` rendering appends `[ruled out: <ruling>]`.

## Files touched

- `src/tool/mod.rs` — `ToolOutput.session_meta_mark` + builder.
- `src/agent.rs` — `dispatch_tool` applies the mark; Navigate handler writes the ruling (session meta + `reason::record_ruling`).
- `src/tool/reason.rs` — `link` action; `CausalNode.ruling`; `record_ruling`; `list`/`trace` rendering.
- `src/daemon/proto.rs` — `TreeNode.causal_node`/`status`.
- `src/daemon/socket.rs` — `build_tree_nodes` extracts causal_node/status from meta.
- `src/client/mod.rs` — `print_tree` renders the markers.
- `skills/debug-causal.md` — add a "link the branch" step to the workflow.
