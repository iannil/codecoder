# Tree Sessions Phase B — Client-Server Tree UX (Design Spec)

**Date:** 2026-07-21
**Branch:** `feat/tree-sessions-phase-b`
**Status:** Approved (brainstormed 2026-07-21)
**Related:** `docs/design/2026-07-19-tree-sessions.md` (Wave 2 / roadmap #8 — the full A→E spec), ADR 0004 (session persistence), ADR 0032 (client-server architecture), ADR 0029 (steering/Navigate)

## Goal

Make the session tree **visible and navigable from `cc`**. Phase A landed the full tree data model (`SessionEntry.parent`, `Session.leaf`, `active_thread`, v2 migration); Phase C landed abandoned-branch summarization in the `AgentCommand::Navigate` path; `Session::clone_to` exists. But after the client-server migration removed the TUI, there is **no user-facing surface** — `cc` users cannot view the tree, navigate to a historical point (fork), or clone a session. This spec wires the existing tree primitives to the daemon wire protocol + `cc`, with **zero new agent-side machinery**.

This is the first of two sub-projects for "tree-session deepening" (the user chose B + E). Phase E (reasoning-tree integration) gets its own spec after B lands.

## Background / Current State

- `src/session.rs`: `Session { schema_version:2, model, token_count, entries: Vec<SessionEntry>, leaf }`; `SessionEntry { message, parent, meta }`; `active_thread()`, `append()`, `navigate_to(id) -> bool`, `abandoned_branch(id)`, `clone_to(root) -> Result<PathBuf>`, `SessionManager::last()`.
- `src/agent.rs`: `AgentCommand::Navigate(u64)` exists (line 31), handled at line 456 — navigates `leaf = id`, summarizes the abandoned branch via tier-2 `summarize_span` (Phase C), autosaves.
- `src/daemon/socket.rs`: `handle_connection` is a persistent loop (post-event-bus) dispatching `ClientRequest` arms (`SendMessage`/`Resume`/`ListSessions`/`NewSession`/`Shutdown`/`Status`/`PromptReply`); drains turns via `drain_agent_events` into a `combined` channel written by a writer thread.
- `src/daemon/session_manager.rs`: `DaemonSessionManager` with `dispatch(id, cmd)` (returns raw `Receiver<AgentEvent>`), `send_message`, `resume`, `disk_sessions`, `create`, `list`.
- `src/daemon/proto.rs`: `ClientRequest` / `ServerEvent` tagged enums; `PromptBody` / `PromptAnswer`.
- `src/bin/cc.rs`: `send_one` (one-shot) + `repl` (REPL with reader thread + `Arc<Mutex<ConnectionWriter>>` + turn-done signal; `/exit`/`/quit` slash intercepts).
- `src/client/mod.rs`: `Connection`, `print_event`, `prompt_user`, `Connection::split`.

**The gap:** no `ClientRequest` for tree operations; no `cc tree`/`fork`/`clone`; the tree exists in the session file but is invisible to the user.

## Non-goals

- Phase E (reasoning-tree agent integration: marking hypotheses, leveraging summaries during debug) — separate spec.
- Phase D (typed entries / `ModelChange`) — YAGNI per the original spec.
- A graphical/interactive tree picker (arrow-key navigation). `cc tree` renders read-only; `cc fork <id>` takes an explicit id. (YAGNI — a dev tool.)
- Precise "this client's session" targeting for `TreeShow` (see Known limitations). `cc tree` shows the latest session file.
- Per-session tree when multiple sessions are live (multi-session disambiguation) — follow-up.

## Design — Approach 1: mixed data source + subcommand/REPL dual entry

### Protocol additions (`src/daemon/proto.rs`)

```rust
/// One node of the session tree, for the `cc tree` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: u64,
    pub parent: Option<u64>,
    pub role: String,           // "user" | "assistant" | "tool" | "system"
    pub preview: String,        // first non-empty line of the message, truncated
    pub is_leaf: bool,          // true iff id == session.leaf
    pub on_active_path: bool,   // true iff on the leaf→root active thread
}
```

Add to `ClientRequest` (`#[serde(tag="type", rename_all="snake_case")]`):
- `TreeShow` — request the active session's tree.
- `TreeNav { id: u64 }` — navigate the active session's leaf to `id` (fork point; next append branches).
- `TreeClone` — copy the active session to a new session file.

Add to `ServerEvent`:
- `Tree { nodes: Vec<TreeNode> }` — the tree view payload.

### Daemon wiring (`src/daemon/socket.rs::handle_connection`)

Three new arms in the `match req` loop, alongside `SendMessage`/`Resume`/…:

- **`TreeShow`**: read the latest session file via `session::SessionManager::new(&root).last()`, parse `Session::load`, build `Vec<TreeNode>` (compute `active_thread` ids once for `on_active_path`), send `ServerEvent::Tree { nodes }` to `combined_tx`. No session file → `ServerEvent::Error { "no session" }`.
- **`TreeNav { id }`**: same active-session selection as `SendMessage` (`g.list().first().cloned().unwrap_or_else(|| g.create())`), then `g.dispatch(id, AgentCommand::Navigate(id))` returning a turn-rx; `drop(g)`; lock `turn_token`; `drain_agent_events(rx, &mut reader, &combined_tx)` (the agent navigates, runs Phase C abandoned-branch summary, autosaves, emits `Notice` + `TurnComplete`). Identical to the `SendMessage` arm except the command is `Navigate(id)`.
- **`TreeClone`**: read latest session file, `Session::load`, `session.clone_to(&root)` → new path; derive the new session id (file stem); send `ServerEvent::SessionCreated { id }` (reuse the existing variant). No session file → `Error`.

`fork` is **not** a separate request — `cc fork <id>` sends `TreeNav { id }`; the branch forms on the user's next message.

### `cc` UX (`src/bin/cc.rs` + `src/client/mod.rs`)

**Subcommands** (argv dispatch in `main`, alongside `sessions`/`status`/`shutdown`):
- `cc tree` → `send_one(sock, ClientRequest::TreeShow)`; on `ServerEvent::Tree { nodes }` call `print_tree(&nodes)`.
- `cc fork <id>` → `send_one(sock, ClientRequest::TreeNav { id })`; drain events (Notice/TurnComplete) via `print_event`.
- `cc clone` → `send_one(sock, ClientRequest::TreeClone)`; on `ServerEvent::SessionCreated { id }` print `cloned to <id> (use: cc --session <id>)`.

**REPL slash intercepts** (in `repl`, alongside `/exit`/`/quit`): `/tree`, `/fork <id>`, `/clone` — the main thread sends the corresponding `ClientRequest` via the shared `Arc<Mutex<ConnectionWriter>>`, then (for `/tree`/`/clone`) waits briefly / (for `/fork`) waits for the turn-done signal. The reader thread prints the resulting events as usual.

**`print_tree(&[TreeNode])`** (`src/client/mod.rs`): a pure function (extracted so it's unit-testable without stdout). Builds `parent → Vec<child>`; roots = `parent == None`; DFS with depth-based indentation; prefixes `►` for `on_active_path`, `●` for `is_leaf`, nothing for abandoned nodes; line format `{prefix} [{id}] {role}: {preview}`. Returns the rendered `String` (the caller prints it). Example:
```
● [1] user: fix the login bug
  ► [2] assistant: I'll check the auth module
    ► [3] tool(read_file): src/auth.rs
      ► [5] user: try the token refresh path
    [4] assistant: maybe it's the DB pool
```

### Data flow

- `cc tree` → `TreeShow` → daemon reads latest session file → builds `TreeNode`s (with `active_thread` for `on_active_path`) → `ServerEvent::Tree` → `cc` `print_tree`.
- `cc fork <id>` → `TreeNav{id}` → daemon `dispatch(Navigate(id))` → agent sets `leaf=id`, summarizes abandoned branch (Phase C, already wired), autosaves → `Notice`+`TurnComplete` → `cc` prints; the user's next message appends from `id` (new branch).
- `cc clone` → `TreeClone` → daemon `clone_to` → new session file → `SessionCreated{id}` → `cc` prints how to resume it.

## Error handling

- **No session file** (`SessionManager::last()` is `None`): `TreeShow`/`TreeClone` → `ServerEvent::Error { "no session" }`.
- **`TreeNav` unknown id**: `AgentLoop::navigate_to` returns `false`; the agent emits a `Notice` ("unknown id") + `TurnComplete` (no crash). Confirm the Navigate handler already does this; if it silently no-ops, add a Notice.
- **Corrupt session file**: `Session::load` fails → `Error { message }`.
- **Lock poisoning**: `.unwrap()` per codebase convention.

## Testing

- `proto::tests`: `TreeNode` + `ServerEvent::Tree` + the 3 new `ClientRequest` variants serde round-trip.
- `socket` integration test **`treeshow_returns_tree_with_active_path`**: write a v2 session file with a fork (root + two children, leaf = one child); `TreeShow` → assert `ServerEvent::Tree` nodes match (count, parent links, `on_active_path` true only on the leaf's branch, `is_leaf` true only on the leaf).
- `socket` integration test **`treenav_changes_leaf`**: write a session; `TreeNav{ <non-leaf id> }`; drain to TurnComplete; re-read the session file; assert `leaf` changed to the requested id.
- `socket` integration test **`treeclone_creates_new_session_file`**: write a session; `TreeClone`; assert `SessionCreated{id}` returned AND a second session file now exists on disk.
- `client::tests::print_tree_renders_active_path_and_leaf`: construct `TreeNode`s for a small forked tree, call `print_tree`, assert the returned string contains `►` for the active-path node, `●` for the leaf, and the abandoned node unmarked.
- Existing suite stays green; the new `ClientRequest`/`ServerEvent` variants' exhaustiveness ripples to `print_event` (add a `Tree { .. }` arm — render by calling `print_tree` and printing the result, returning `false`).

## Known limitations (acceptance)

- **TreeShow targets the latest session file** (`SessionManager::last()`), not a specific client's session. In the typical single-active-session daemon this is the active session (it autosaves on every append → most-recent mtime). In a multi-session daemon, `cc tree` shows the most-recently-modified session. Precise per-session targeting (exposing `AgentLoop::session_path` through `DaemonSession`) is a follow-up if multi-session use demands it.
- `cc tree` is read-only (no arrow-key picker). Navigation is by explicit id via `cc fork <id>` / `/fork <id>`.

## Interfaces (contract for the implementation plan)

- `proto::TreeNode { id: u64, parent: Option<u64>, role: String, preview: String, is_leaf: bool, on_active_path: bool }`.
- `proto::ClientRequest::{ TreeShow, TreeNav { id: u64 }, TreeClone }`.
- `proto::ServerEvent::Tree { nodes: Vec<TreeNode> }`.
- `socket::handle_connection` gains 3 match arms (no signature change — `bus`/`mgr`/`shutdown`/`turn_token` already in scope).
- `client::print_tree(&[TreeNode]) -> String` (pure, testable).
- `cc.rs`: subcommand dispatch (`cc tree`/`cc fork <id>`/`cc clone`) + REPL slash intercepts (`/tree`/`/fork <id>`/`/clone`).

## Files touched

- `src/daemon/proto.rs` — `TreeNode`, 3 `ClientRequest` variants, `ServerEvent::Tree`.
- `src/daemon/socket.rs` — 3 `handle_connection` arms; a small helper `build_tree_nodes(&Session) -> Vec<TreeNode>`.
- `src/client/mod.rs` — `print_tree`; `print_event` `Tree` arm.
- `src/bin/cc.rs` — subcommand dispatch + REPL slash intercepts.
- (No change to `src/agent.rs` — `Navigate` already exists; no change to `src/session.rs` — `clone_to`/`active_thread` already exist.)
