# Tree Sessions Phase B Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the session tree visible and navigable from `cc` — `cc tree` view, `cc fork <id>` (navigate/fork), `cc clone` (copy session) — plus REPL `/tree` `/fork <id>` `/clone` slash intercepts. Zero new agent-side machinery (reuses `AgentCommand::Navigate`, `Session::clone_to`, `active_thread`).

**Architecture:** 3 new `ClientRequest`s (`TreeShow`/`TreeNav{id}`/`TreeClone`) + `ServerEvent::Tree{nodes}` + `TreeNode`. The daemon's `handle_connection` gains 3 arms: `TreeShow` reads the latest session file + builds `TreeNode`s; `TreeNav` dispatches `AgentCommand::Navigate(id)` (reuses the turn drain); `TreeClone` calls `Session::clone_to`. `cc` adds subcommands + REPL slash intercepts + a pure `print_tree` renderer.

**Tech Stack:** Rust (edition 2024), `serde`, `std`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-21-tree-sessions-phase-b-design.md`

---

## Global Constraints

- **No new dependencies.** New `pub` items need doc comments.
- New tests use `std::env::temp_dir().join(format!("cc_<name>{}", std::process::id()))` + `remove_dir_all` cleanup.
- Existing suite stays green; `cargo build` warning-free; **full suite COMPLETES (no hang)**.
- `print_event` must stay exhaustive — adding `ServerEvent::Tree` requires a `Tree` arm (Task 1).
- Commit messages: `feat:` prefix, single line, English.
- **Zero changes to `src/agent.rs`** (the `Navigate` handler already emits `Notice` + `TurnComplete` on both success and unknown-id) and **zero to `src/session.rs`** (`active_thread`/`clone_to`/`SessionManager::last` already exist).

---

## File Structure

| File | Responsibility | Create/Modify |
|---|---|---|
| `src/daemon/proto.rs` | `TreeNode`, 3 `ClientRequest` variants, `ServerEvent::Tree` | Modify (add) |
| `src/client/mod.rs` | `print_tree(&[TreeNode]) -> String` (pure); `print_event` `Tree` arm | Modify (add) |
| `src/daemon/session_manager.rs` | `pub fn navigate(session_id, target) -> Receiver<AgentEvent>` (wraps private `dispatch`) | Modify (add) |
| `src/daemon/socket.rs` | 3 `handle_connection` arms + `build_tree_nodes(&Session) -> Vec<TreeNode>` helper | Modify (add) |
| `src/bin/cc.rs` | subcommand dispatch (`cc tree`/`cc fork <id>`/`cc clone`) + REPL slash intercepts (`/tree` `/fork <id>` `/clone`) | Modify |

---

### Task 1: Protocol additions + `print_tree` renderer

**Files:**
- Modify: `src/daemon/proto.rs` — add `TreeNode`, `ClientRequest::{TreeShow, TreeNav{id}, TreeClone}`, `ServerEvent::Tree`
- Modify: `src/client/mod.rs` — add `print_tree`; add `print_event` `Tree` arm
- Test: `src/daemon/proto.rs` inline tests + `src/client/mod.rs` inline tests

**Interfaces:**
- Consumes: `serde::{Serialize, Deserialize}` (proto.rs already uses).
- Produces (used by Tasks 2 & 3):
  - `proto::TreeNode { id: u64, parent: Option<u64>, role: String, preview: String, is_leaf: bool, on_active_path: bool }`
  - `proto::ClientRequest::{TreeShow, TreeNav { id: u64 }, TreeClone}`
  - `proto::ServerEvent::Tree { nodes: Vec<TreeNode> }`
  - `client::print_tree(nodes: &[TreeNode]) -> String`

- [ ] **Step 1: Add `TreeNode` + the new `ClientRequest`/`ServerEvent` variants**

In `src/daemon/proto.rs`, add `TreeNode` (near the other proto types, before `ClientRequest`):

```rust
/// One node of the session tree, for the `cc tree` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: u64,
    pub parent: Option<u64>,
    /// "user" | "assistant" | "system" | "tool"
    pub role: String,
    /// First non-empty line of the message, truncated.
    pub preview: String,
    /// true iff this entry is the session's current leaf.
    pub is_leaf: bool,
    /// true iff this entry is on the leaf→root active thread.
    pub on_active_path: bool,
}
```

Add to `pub enum ClientRequest` (the `#[serde(tag = "type", rename_all = "snake_case")]` enum):

```rust
    /// 显示活动 session 的会话树（`cc tree`）。
    TreeShow,
    /// 导航活动 session 的 leaf 到 id（`cc fork <id>`；下次 append 即分叉）。
    TreeNav { id: u64 },
    /// 复制活动 session 为新 session 文件（`cc clone`）。
    TreeClone,
```

Add to `pub enum ServerEvent`:

```rust
    /// 会话树视图（响应 `TreeShow`）。
    Tree { nodes: Vec<TreeNode> },
```

- [ ] **Step 2: Write the failing proto serde tests**

Add to `src/daemon/proto.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn tree_node_and_variants_serde_roundtrip() {
        let n = TreeNode {
            id: 5, parent: Some(2), role: "assistant".into(), preview: "hi".into(),
            is_leaf: true, on_active_path: true,
        };
        let j = serde_json::to_string(&n).unwrap();
        let back: TreeNode = serde_json::from_str(&j).unwrap();
        assert_eq!(n, back);

        let reqs = vec![
            ClientRequest::TreeShow,
            ClientRequest::TreeNav { id: 7 },
            ClientRequest::TreeClone,
        ];
        for r in reqs {
            let j = serde_json::to_string(&r).unwrap();
            assert_eq!(r, serde_json::from_str::<ClientRequest>(&j).unwrap());
        }
        let ev = ServerEvent::Tree { nodes: vec![n.clone()] };
        let j = serde_json::to_string(&ev).unwrap();
        let back: ServerEvent = serde_json::from_str(&j).unwrap();
        assert_eq!(ev, back);
        assert!(j.contains("\"type\":\"tree\""));
    }
```

- [ ] **Step 3: Run the proto test — confirm it passes (the types now exist)**

Run: `cargo test --lib proto::tests::tree_node_and_variants_serde_roundtrip 2>&1 | tail -10`
Expected: PASS. (The build will FAIL elsewhere — `print_event` in `src/client/mod.rs` is no longer exhaustive over `ServerEvent` — that's expected; Step 4 fixes it.)

- [ ] **Step 4: Add the `print_tree` renderer + `print_event` `Tree` arm**

In `src/client/mod.rs`, add `print_tree` (a pure function returning the rendered string, so it's unit-testable without stdout):

```rust
/// 渲染会话树为字符串：按 parent 链缩进，active 路径前缀 ►，leaf 前缀 ●，废弃分支无标记。
/// 纯函数（不打印），便于单测。
pub fn print_tree(nodes: &[crate::daemon::proto::TreeNode]) -> String {
    use std::collections::HashMap;
    let by_id: HashMap<u64, &crate::daemon::proto::TreeNode> =
        nodes.iter().map(|n| (n.id, n)).collect();
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut roots: Vec<u64> = Vec::new();
    for n in nodes {
        match n.parent {
            None => roots.push(n.id),
            Some(p) => children.entry(p).or_default().push(n.id),
        }
    }
    // 稳定排序：按 id 升序，保证渲染确定性
    for v in children.values_mut() { v.sort(); }
    roots.sort();

    let mut out = String::new();
    fn rec(
        id: u64,
        depth: usize,
        by_id: &HashMap<u64, &crate::daemon::proto::TreeNode>,
        children: &HashMap<u64, Vec<u64>>,
        out: &mut String,
    ) {
        let n = by_id.get(&id).copied().unwrap();
        let prefix = if n.is_leaf { "●" }
            else if n.on_active_path { "►" }
            else { " " };
        let indent = "  ".repeat(depth);
        out.push_str(&format!("{indent}{prefix} [{}] {}: {}\n", n.id, n.role, n.preview));
        if let Some(kids) = children.get(&id) {
            for c in kids { rec(*c, depth + 1, by_id, children, out); }
        }
    }
    for r in roots { rec(r, 0, &by_id, &children, &mut out); }
    out
}
```

Add a `Tree` arm to `print_event` (keeps the match exhaustive; renders via `print_tree`):

```rust
        ServerEvent::Tree { nodes } => {
            print!("{}", print_tree(nodes));
            let _ = std::io::stdout().flush();
            false
        }
```

(Place it among the other `ServerEvent` arms. `print_tree` must be in scope — it's in the same module, so a bare call works.)

- [ ] **Step 5: Write the `print_tree` unit test**

Add to `src/client/mod.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn print_tree_marks_active_path_leaf_and_abandoned() {
        use crate::daemon::proto::TreeNode;
        // root(0) → A(1) → leaf(3)   [active]
        //        → B(2)               [abandoned]
        let nodes = vec![
            TreeNode { id: 0, parent: None,      role: "user".into(),      preview: "root".into(),      is_leaf: false, on_active_path: true },
            TreeNode { id: 1, parent: Some(0),   role: "assistant".into(), preview: "A".into(),         is_leaf: false, on_active_path: true },
            TreeNode { id: 2, parent: Some(0),   role: "assistant".into(), preview: "B".into(),         is_leaf: false, on_active_path: false },
            TreeNode { id: 3, parent: Some(1),   role: "user".into(),      preview: "leaf".into(),      is_leaf: true,  on_active_path: true },
        ];
        let rendered = print_tree(&nodes);
        // active path nodes (0,1,3) get ► or ●; abandoned (2) gets a space prefix
        assert!(rendered.contains("► [0]"));
        assert!(rendered.contains("► [1]"));
        assert!(rendered.contains("● [3]"));
        assert!(rendered.contains("  [2]")); // abandoned: leading space, no marker
        // indentation depth: leaf at depth 2
        assert!(rendered.contains("      ● [3]"));
    }
```

- [ ] **Step 6: Build + run focused tests**

Run: `cargo build 2>&1 | tail -10`
Expected: warning-free (the `print_event` match is exhaustive again).

Run: `cargo test --lib proto::tests::tree_node_and_variants_serde_roundtrip client::tests::print_tree_marks_active_path_leaf_and_abandoned 2>&1 | tail -15`
Expected: both PASS.

Run: `cargo test 2>&1 | tail -20`
Expected: 0 failed, 3 ignored. The full suite still completes (no hang).

- [ ] **Step 7: Commit**

```bash
git add src/daemon/proto.rs src/client/mod.rs
git commit -m "feat: TreeNode + Tree* protocol variants + print_tree renderer"
```

---

### Task 2: Daemon `TreeShow` / `TreeNav` / `TreeClone` arms

**Files:**
- Modify: `src/daemon/session_manager.rs` — add `pub fn navigate(session_id, target) -> Receiver<AgentEvent>`
- Modify: `src/daemon/socket.rs` — add 3 `handle_connection` arms + `build_tree_nodes(&Session) -> Vec<TreeNode>` helper
- Test: `src/daemon/socket.rs` inline tests (3 integration tests)

**Interfaces:**
- Consumes: Task 1's `TreeNode`/`ClientRequest::Tree*`/`ServerEvent::Tree`; `session::{Session, SessionManager, sessions_dir}`; `AgentCommand::Navigate`; `DaemonSessionManager::{list, create, dispatch (via navigate)}`.
- Produces: `DaemonSessionManager::navigate(session_id: &str, target: u64) -> anyhow::Result<Receiver<AgentEvent>>`; `socket::build_tree_nodes(&Session) -> Vec<TreeNode>`.

- [ ] **Step 1: Add `DaemonSessionManager::navigate`**

In `src/daemon/session_manager.rs`, alongside `send_message`/`resume` (which wrap the private `dispatch`), add:

```rust
    /// 导航活动 session 的 leaf 到 target（`cc fork <id>`）。复用 `AgentCommand::Navigate`：
    /// agent 改 leaf + Phase C 摘要废弃分支 + 自动落盘，发 Notice + TurnComplete。
    pub fn navigate(&mut self, session_id: &str, target: u64) -> anyhow::Result<Receiver<AgentEvent>> {
        self.dispatch(session_id, AgentCommand::Navigate(target))
    }
```

(`dispatch` is the existing private `fn dispatch(&mut self, id: &str, cmd: AgentCommand) -> anyhow::Result<Receiver<AgentEvent>>` at line ~110.)

- [ ] **Step 2: Add `build_tree_nodes` helper + the 3 `handle_connection` arms**

In `src/daemon/socket.rs`, add a free helper (near `drain_agent_events`) that converts a `Session` to wire `TreeNode`s:

```rust
/// 从 Session 构造树视图节点：active_thread 标 on_active_path，leaf 标 is_leaf。
fn build_tree_nodes(session: &crate::session::Session) -> Vec<super::proto::TreeNode> {
    use crate::message::{Message, MessageItem};
    let active: std::collections::HashSet<u64> =
        session.active_thread().iter().map(|m| m.id).collect();
    let leaf = session.leaf;
    let truncate = |s: &str, n: usize| -> String {
        let first = s.lines().next().unwrap_or("").trim();
        if first.chars().count() <= n {
            first.to_string()
        } else {
            format!("{}…", first.chars().take(n).collect::<String>())
        }
    };
    let preview_of = |msg: &Message| -> String {
        for item in &msg.items {
            match item {
                MessageItem::Text { text } | MessageItem::Reasoning { text } => return truncate(text, 60),
                MessageItem::ToolCall { name, .. } => return format!("{}(…)", truncate(name, 50)),
                MessageItem::ToolResult { output, .. } => return truncate(output, 60),
            }
        }
        String::new()
    };
    session.entries.iter().map(|e| {
        let id = e.message.id;
        super::proto::TreeNode {
            id,
            parent: e.parent,
            role: format!("{:?}", e.message.role).to_lowercase(),
            preview: preview_of(&e.message),
            is_leaf: leaf == Some(id),
            on_active_path: active.contains(&id),
        }
    }).collect()
}
```

In `handle_connection`'s `match req` loop, add 3 arms (alongside `SendMessage`/`Resume`/`ListSessions`/…). Note: control one-shots (`TreeShow`/`TreeClone`) end with a `TurnComplete` so the `cc` one-shot loop terminates (see spec §Error handling / known issue):

```rust
            ClientRequest::TreeShow => {
                // 读最新 session 文件 → 建树视图。（单活动 session 场景≈活动 session。）
                let ev = match crate::session::SessionManager::new(&root_for_tree).last() {
                    None => super::proto::ServerEvent::Error { message: "no session".into() },
                    Some(id) => {
                        let path = crate::session::sessions_dir(&root_for_tree).join(format!("{id}.json"));
                        match std::fs::read_to_string(&path)
                            .map_err(anyhow::Error::from)
                            .and_then(|raw| crate::session::Session::load(&raw))
                        {
                            Ok(s) => super::proto::ServerEvent::Tree { nodes: build_tree_nodes(&s) },
                            Err(e) => super::proto::ServerEvent::Error { message: format!("load: {e}") },
                        }
                    }
                };
                let _ = body_tx.send(ev);
                let _ = body_tx.send(super::proto::ServerEvent::TurnComplete);
            }
            ClientRequest::TreeNav { id } => {
                // 与 SendMessage 同样的活动 session 选择 + dispatch/drain；command = Navigate(id)。
                let mut g = mgr.lock().unwrap();
                let sid = match g.list().first().cloned() { Some(s) => s, None => g.create() };
                let rx = g.navigate(&sid, id)?;
                drop(g);
                let _turn_guard = turn_token.lock().unwrap();
                drain_agent_events(rx, &mut reader, body_tx)?;
            }
            ClientRequest::TreeClone => {
                let ev = match crate::session::SessionManager::new(&root_for_tree).last() {
                    None => super::proto::ServerEvent::Error { message: "no session".into() },
                    Some(id) => {
                        let path = crate::session::sessions_dir(&root_for_tree).join(format!("{id}.json"));
                        match std::fs::read_to_string(&path)
                            .map_err(anyhow::Error::from)
                            .and_then(|raw| crate::session::Session::load(&raw))
                            .and_then(|s| s.clone_to(&root_for_tree))
                        {
                            Ok(new_path) => {
                                let new_id = new_path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                                super::proto::ServerEvent::SessionCreated { id: new_id }
                            }
                            Err(e) => super::proto::ServerEvent::Error { message: format!("clone: {e}") },
                        }
                    }
                };
                let _ = body_tx.send(ev);
                let _ = body_tx.send(super::proto::ServerEvent::TurnComplete);
            }
```

`root_for_tree` is the daemon root. `handle_connection` doesn't currently have `root` in scope — it has `mgr` (which owns the root) but not the raw path. Add it by reading from the session manager: the cleanest is to capture root in `handle_connection`. **Read the current `handle_connection` signature/body first** — it takes `(stream, mgr, shutdown, turn_token, bus)`. The root is inside `mgr`'s `DaemonSessionManager` (a field). Expose it: add `pub fn root(&self) -> &Path` to `DaemonSessionManager` (it stores `root: PathBuf`), and in `handle_connection` do `let root_for_tree = mgr.lock().unwrap().root().to_path_buf();` once near the top (after the writer-thread spawn, before the loop). Update the Files list mentally: this touches `session_manager.rs` (add `root()` accessor) — fold into Step 1.

- [ ] **Step 3: Write the failing daemon integration tests**

Add to `src/daemon/socket.rs`'s `#[cfg(test)] mod tests`. These build a forked `Session` on disk, drive `handle_connection`, and assert the responses. Model them on the existing `client_sendmessage_roundtrips_through_socket` test (construct `mgr`/`bus`/`turn_token`, spawn a server thread calling `handle_connection`, connect a client, write a request line, read response lines).

```rust
    fn forked_session_file(dir: &std::path::Path) -> std::path::PathBuf {
        use crate::message::{Message, Role};
        use crate::session::{sessions_dir, Session};
        std::fs::create_dir_all(sessions_dir(dir)).unwrap();
        let mut s = Session::new("gpt-4o");
        s.append(Message::text(0, Role::User, "fix bug"));         // id 0, leaf 0
        s.append(Message::text(1, Role::Assistant, "check X"));    // id 1, parent 0, leaf 1
        s.navigate_to(0).unwrap();                                  // leaf -> 0
        s.append(Message::text(2, Role::Assistant, "check Y"));    // id 2, parent 0 (fork), leaf 2
        let path = sessions_dir(dir).join("session-fork.json");
        s.save(&path).unwrap();
        path
    }

    #[test]
    fn treeshow_returns_tree_with_active_path() {
        let dir = std::env::temp_dir().join(format!("cc_treeshow_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");
        forked_session_file(&dir);

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let turn_token = mgr.lock().unwrap().turn_token();
        let bus = Arc::new(crate::daemon::bus::EventBus::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        let mgr_c = Arc::clone(&mgr);
        let bus_c = Arc::clone(&bus);
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            handle_connection(s, &mgr_c, &shutdown_c, &turn_token, &bus_c).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));

        let mut conn = UnixStream::connect(&sock).unwrap();
        use std::io::Write;
        writeln!(conn, "{}", serde_json::to_string(&ClientRequest::TreeShow).unwrap()).unwrap();
        conn.flush().unwrap();

        let mut r = BufReader::new(conn.try_clone().unwrap());
        let mut tree_nodes: Option<Vec<crate::daemon::proto::TreeNode>> = None;
        loop {
            let mut buf = String::new();
            if r.read_line(&mut buf).unwrap() == 0 { break; }
            if let Ok(ServerEvent::Tree { nodes }) = serde_json::from_str(buf.trim()) { tree_nodes = Some(nodes); }
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(buf.trim()) { break; }
        }
        drop(conn);
        h.join().unwrap();
        let nodes = tree_nodes.expect("TreeShow must return Tree");
        assert_eq!(nodes.len(), 3, "forked session has 3 entries");
        // leaf is id 2; active path = {2, 0}; entry 1 is abandoned
        let by_id: std::collections::HashMap<u64, &crate::daemon::proto::TreeNode> =
            nodes.iter().map(|n| (n.id, n)).collect();
        assert!(by_id[&2].is_leaf, "id 2 is the leaf");
        assert!(by_id[&2].on_active_path);
        assert!(by_id[&0].on_active_path);
        assert!(!by_id[&1].on_active_path, "id 1 is the abandoned branch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn treenav_changes_leaf() {
        let dir = std::env::temp_dir().join(format!("cc_treenav_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");
        forked_session_file(&dir); // leaf = 2

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let turn_token = mgr.lock().unwrap().turn_token();
        let bus = Arc::new(crate::daemon::bus::EventBus::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        let mgr_c = Arc::clone(&mgr);
        let bus_c = Arc::clone(&bus);
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            handle_connection(s, &mgr_c, &shutdown_c, &turn_token, &bus_c).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));

        let mut conn = UnixStream::connect(&sock).unwrap();
        use std::io::Write;
        // navigate to entry 0 (the root) — TreeNav goes through the in-memory session,
        // so first force a session to exist by sending TreeShow? No: TreeNav auto-creates
        // a session via g.list().first(). But the in-memory session is EMPTY (no entries),
        // so navigating to id 0 finds nothing. Instead, resume the on-disk session first.
        // Simplest: send a Resume so the in-memory session loads the forked file, THEN TreeNav.
        writeln!(conn, "{}", serde_json::to_string(&ClientRequest::Resume { id: "session-fork".into() }).unwrap()).unwrap();
        conn.flush().unwrap();
        // drain Resume's events to TurnComplete
        let mut r = BufReader::new(conn.try_clone().unwrap());
        loop { let mut b = String::new(); if r.read_line(&mut b).unwrap() == 0 { break; }
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(b.trim()) { break; } }
        // now TreeNav to id 0
        writeln!(conn, "{}", serde_json::to_string(&ClientRequest::TreeNav { id: 0 }).unwrap()).unwrap();
        conn.flush().unwrap();
        loop { let mut b = String::new(); if r.read_line(&mut b).unwrap() == 0 { break; }
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(b.trim()) { break; } }
        drop(conn);
        h.join().unwrap();

        // re-read the file: leaf should now be 0
        let raw = std::fs::read_to_string(crate::session::sessions_dir(&dir).join("session-fork.json")).unwrap();
        let s = crate::session::Session::load(&raw).unwrap();
        assert_eq!(s.leaf, Some(0), "TreeNav must move the leaf to id 0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn treeclone_creates_new_session_file() {
        let dir = std::env::temp_dir().join(format!("cc_treeclone_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");
        forked_session_file(&dir);
        let before = std::fs::read_dir(crate::session::sessions_dir(&dir)).unwrap().count();

        let server = SocketServer::bind(&sock).unwrap();
        let registry = Arc::new(std::sync::RwLock::new(Registry::scan(&dir)));
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(), registry,
        )));
        let turn_token = mgr.lock().unwrap().turn_token();
        let bus = Arc::new(crate::daemon::bus::EventBus::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let mgr_c = Arc::clone(&mgr);
        let bus_c = Arc::clone(&bus);
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            handle_connection(s, &mgr_c, &shutdown_c, &turn_token, &bus_c).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));

        let mut conn = UnixStream::connect(&sock).unwrap();
        use std::io::Write;
        writeln!(conn, "{}", serde_json::to_string(&ClientRequest::TreeClone).unwrap()).unwrap();
        conn.flush().unwrap();
        let mut r = BufReader::new(conn.try_clone().unwrap());
        let mut got_created = false;
        loop { let mut b = String::new(); if r.read_line(&mut b).unwrap() == 0 { break; }
            if let Ok(ServerEvent::SessionCreated { .. }) = serde_json::from_str(b.trim()) { got_created = true; }
            if let Ok(ServerEvent::TurnComplete) = serde_json::from_str(b.trim()) { break; } }
        drop(conn);
        h.join().unwrap();
        assert!(got_created, "TreeClone must return SessionCreated");
        let after = std::fs::read_dir(crate::session::sessions_dir(&dir)).unwrap().count();
        assert_eq!(after, before + 1, "TreeClone must create one new session file");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

> **Note on the `treenav` test:** `TreeNav` operates on the in-memory session. The daemon auto-creates an EMPTY session if none exists (`g.list().first()` is empty → `g.create()`), and navigating an empty session to id 0 is a no-op. So the test first sends `Resume { id: "session-fork" }` to load the forked file into the in-memory session, THEN `TreeNav { id: 0 }`. If `Resume` doesn't populate the in-memory session's entries from disk (verify the `resume` path in `session_manager.rs`/`agent.rs` — it routes `AgentCommand::Resume` which calls `resume_latest`), adjust the test to use whichever request loads the disk session. The assertion (leaf moved to 0 after TreeNav) is the invariant.

- [ ] **Step 4: Run the daemon tests — confirm pass**

Run: `cargo test --lib daemon::socket::tests::tree 2>&1 | tail -25`
Expected: all 3 new tests PASS. If `treenav` fails because the in-memory session wasn't loaded, adjust per the note (the invariant is: after TreeNav to id 0, the saved file's leaf is 0).

Run: `cargo build 2>&1 | tail -10` → warning-free.
Run: `cargo test 2>&1 | tail -20` → 0 failed, 3 ignored, **full suite completes (no hang)**.

- [ ] **Step 5: Commit**

```bash
git add src/daemon/session_manager.rs src/daemon/socket.rs
git commit -m "feat: daemon TreeShow/TreeNav/TreeClone arms (tree view, navigate, clone)"
```

---

### Task 3: `cc` subcommands + REPL slash intercepts

**Files:**
- Modify: `src/bin/cc.rs` — subcommand dispatch (`cc tree`/`cc fork <id>`/`cc clone`) + REPL `/tree` `/fork <id>` `/clone` intercepts
- Test: covered by compile + Task 1's `print_tree` test + Task 2's daemon integration tests (the command wiring is UX; `print_event`'s `Tree` arm already renders via `print_tree`).

**Interfaces:**
- Consumes: Task 1's `print_tree`/`print_event` (Tree arm); Task 2's `ClientRequest::Tree*`/`ServerEvent::Tree`.
- Produces: `cc tree` / `cc fork <id>` / `cc clone` subcommands; REPL `/tree` / `/fork <id>` / `/clone`.

- [ ] **Step 1: Add subcommand dispatch in `main`**

In `src/bin/cc.rs::main`, the existing `match args.as_slice()` dispatches `sessions`/`status`/`shutdown`/default(SendMessage). Add `tree`/`fork`/`clone` arms. Read the current `main` first; add alongside the existing arms:

```rust
        [one] if one == "tree" => send_one(&sock, ClientRequest::TreeShow),
        [one, id] if one == "fork" => {
            let id: u64 = id.parse().map_err(|e| anyhow::anyhow!("fork <id>: {e}"))?;
            send_one(&sock, ClientRequest::TreeNav { id })
        }
        [one] if one == "clone" => send_one(&sock, ClientRequest::TreeClone),
```

(Place these among the existing `[one] if one == "sessions"` arms. The `[one, id]` fork arm must come before the catch-all `[msg @ ..]` SendMessage arm.)

- [ ] **Step 2: `send_one` already handles the responses**

`send_one` loops `conn.next_event()` and calls `print_event`. Task 1 added `print_event`'s `Tree` arm (renders via `print_tree`) and `SessionCreated` already has an arm. The daemon's `TreeShow`/`TreeClone` arms send a trailing `TurnComplete` (Task 2), so `send_one`'s loop terminates (it breaks on `TurnComplete`). `cc fork <id>` dispatches `TreeNav` → drains through `print_event` (Notice + TurnComplete). **No change to `send_one` needed** — verify by reading it.

- [ ] **Step 3: Add REPL slash intercepts**

In `src/bin/cc.rs::repl`, the main loop currently intercepts `/exit`/`/quit` (around the `if trimmed == "/exit" ...` line). Add `/tree`, `/fork <id>`, `/clone` intercepts BEFORE the SendMessage send. The REPL main thread owns the shared `Arc<Mutex<ConnectionWriter>>`; for `/tree`/`/clone` it sends the request and the reader thread prints the response (the reader is always draining). For `/fork <id>` it sends `TreeNav` and waits for the turn-done signal (like SendMessage).

Read the current `repl` body first (it has the reader-thread + `done_rx` structure from the event-bus Task 5). Add, just after the `/exit`/`/quit` check and before the `writer.lock().send(SendMessage)` line:

```rust
            if trimmed == "/tree" {
                writer.lock().unwrap().send(&ClientRequest::TreeShow)?;
                let _ = done_rx.recv(); // TreeShow -> Tree + TurnComplete -> reader signals done
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("/fork ") {
                let id: u64 = rest.trim().parse().map_err(|e| anyhow::anyhow!("/fork <id>: {e}"))?;
                writer.lock().unwrap().send(&ClientRequest::TreeNav { id })?;
                let _ = done_rx.recv(); // Navigate -> Notice + TurnComplete
                continue;
            }
            if trimmed == "/clone" {
                writer.lock().unwrap().send(&ClientRequest::TreeClone)?;
                let _ = done_rx.recv();
                continue;
            }
```

(Adapt the exact placement + variable names to the current `repl`. The reader thread prints the `Tree`/`SessionCreated`/`Notice` events as they arrive; `done_rx.recv()` waits for the trailing `TurnComplete` that the reader signals.)

- [ ] **Step 4: Build + run the full suite**

Run: `cargo build 2>&1 | tail -10`
Expected: warning-free, BOTH binaries build (`codecoder` + `cc`).

Run: `cargo test 2>&1 | tail -20`
Expected: 0 failed, 3 ignored, **full suite completes (no hang)**.

- [ ] **Step 5: Manual smoke (the tree UX end-to-end)**

```bash
ROOT=$(mktemp -d)
CODECODER_ROOT=$ROOT CODECODER_DAEMON=1 cargo run --quiet 2>/dev/null &
DPID=$!
sleep 1
CODECODER_ROOT=$ROOT cargo run --bin cc --quiet -- "start a task" 2>/dev/null   # turn 1
CODECODER_ROOT=$ROOT cargo run --bin cc --quiet -- "another step" 2>/dev/null     # turn 2
CODECODER_ROOT=$ROOT cargo run --bin cc --quiet -- tree 2>/dev/null               # view tree
echo "--- clone ---"
CODECODER_ROOT=$ROOT cargo run --bin cc --quiet -- clone 2>/dev/null
kill $DPID 2>/dev/null; rm -rf $ROOT
```
Expected: `cc tree` prints an indented tree with the active path marked (`►`/`●`); `cc clone` prints `· session <new-id>`. (The daemon tests are authoritative; this is an informational end-to-end check.)

- [ ] **Step 6: Commit**

```bash
git add src/bin/cc.rs
git commit -m "feat: cc tree/fork/clone subcommands + REPL slash intercepts"
```

---

## Self-Review (run after writing — notes for the implementer)

- **Spec coverage:** every spec section maps to a task — proto `TreeNode`/variants + `ServerEvent::Tree` (Task 1); `print_tree` renderer (Task 1); daemon `TreeShow`/`TreeNav`/`TreeClone` arms + `build_tree_nodes` (Task 2); `cc` subcommands + REPL slash intercepts (Task 3). Error handling (no session → Error, unknown id → Notice via Navigate handler, corrupt file → Error) covered in Task 2. Testing: proto round-trip + print_tree unit + 3 daemon integration tests (Task 2) — all 5 spec tests present.
- **Type consistency:** `TreeNode` fields (id/parent/role/preview/is_leaf/on_active_path) match between Task 1 (defined) and Task 2 (`build_tree_nodes`) and Task 1's `print_tree`. `ClientRequest::{TreeShow, TreeNav{id}, TreeClone}` / `ServerEvent::Tree{nodes}` consistent everywhere. `DaemonSessionManager::navigate(session_id, target)` matches the `handle_connection` TreeNav arm. `print_tree(&[TreeNode]) -> String` matches the `print_event` Tree arm.
- **No placeholders:** every step has complete code or exact commands. (Task 2 Step 2 notes the `root_for_tree` plumbing via a `DaemonSessionManager::root()` accessor — fold into Step 1; Task 2 Step 3 notes the `treenav` test's Resume-first invariant explicitly.)
- **Known follow-ups (not blocking):** existing control arms (`ListSessions`/`NewSession`/`Status`) still send no trailing `TurnComplete` → `cc sessions`/`cc status` one-shots hang (latent pre-existing bug, out of Phase B scope; the NEW tree arms terminate correctly). Multi-session `TreeShow` targeting (shows latest file) is a documented limitation.
