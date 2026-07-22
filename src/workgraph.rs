// Work Graph 工作图 (first-class citizen #2; see
// docs/design/2026-07-19-plan-work-graph.md). A durable, dependency-ordered
// graph of Milestone nodes — the "事前构造之图" half opposite the Session's
// "事后记录树". Persisted to `workgraph.json` with the SAME versioned,
// atomic-write, forward-migration discipline as Session (ADR 0004).
//
// The scheduling/validation logic here is pure; the `milestone` tool
// (src/tool/dev.rs) owns I/O and rendering, the AgentLoop owns dispatch.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const WG_SCHEMA_VERSION: u32 = 1;

/// A milestone's lifecycle state. `Blocked` is DERIVED from unmet dependencies
/// (recomputed, never the authoritative record of intent); the others are set
/// explicitly by an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    InProgress,
    Blocked,
    NeedsFix,
    Done,
    /// A candidate cause in a diagnostic (rc) tree — not yet verified (P2).
    Hypothesis,
    /// A cause that has passed the three-step verification (P2).
    Locked,
}

impl NodeStatus {
    fn tag(self) -> &'static str {
        match self {
            NodeStatus::Pending => " ",
            NodeStatus::InProgress => "~",
            NodeStatus::Blocked => "#",
            NodeStatus::NeedsFix => "!",
            NodeStatus::Done => "x",
            NodeStatus::Hypothesis => "?",
            NodeStatus::Locked => "·",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Pending => "pending",
            NodeStatus::InProgress => "in_progress",
            NodeStatus::Blocked => "blocked",
            NodeStatus::NeedsFix => "needs_fix",
            NodeStatus::Done => "done",
            NodeStatus::Hypothesis => "hypothesis",
            NodeStatus::Locked => "locked",
        }
    }
}

/// One node of the Work Graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub deps: Vec<u64>,
    pub status: NodeStatus,
    /// The Review Verdict (#4) attached when this milestone was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Files / commits touched — a human-readable trail, not load-bearing.
    #[serde(default)]
    pub touched: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraph {
    pub schema_version: u32,
    pub nodes: Vec<Milestone>,
}

/// Canonical on-disk location.
pub fn path(root: &Path) -> PathBuf {
    root.join("workgraph.json")
}

impl Default for WorkGraph {
    fn default() -> Self {
        WorkGraph { schema_version: WG_SCHEMA_VERSION, nodes: Vec::new() }
    }
}

impl WorkGraph {
    /// Load the graph for `root`: prefer `workgraph.json`; else migrate a legacy
    /// flat `todos.json` (each todo → a dep-less milestone); else empty.
    pub fn read(root: &Path) -> WorkGraph {
        if let Some(wg) =
            std::fs::read_to_string(path(root)).ok().and_then(|raw| Self::load(&raw).ok())
        {
            return wg;
        }
        if let Some(wg) =
            std::fs::read_to_string(root.join("todos.json")).ok().and_then(|raw| migrate_todos(&raw))
        {
            return wg;
        }
        WorkGraph::default()
    }

    /// Parse raw JSON and migrate forward to `WG_SCHEMA_VERSION` (mirrors
    /// `Session::load`, including refusing a newer-than-supported file).
    pub fn load(raw: &str) -> anyhow::Result<WorkGraph> {
        let mut json: serde_json::Value = serde_json::from_str(raw)?;
        let mut version = json.get("schema_version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if version > WG_SCHEMA_VERSION {
            anyhow::bail!(
                "workgraph schema_version {version} is newer than supported {WG_SCHEMA_VERSION}; refusing to mis-read"
            );
        }
        while version < WG_SCHEMA_VERSION {
            json = migrate(version, json)?;
            version += 1;
        }
        Ok(serde_json::from_value(json)?)
    }

    /// Atomically persist (write temp + rename), like `Session::save` (ADR 0004).
    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        let p = path(root);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, p)?;
        Ok(())
    }

    /// 在咨询文件锁内执行 read→mutate→save(ADR 0035),防并发写者 lost-update。
    /// 锁独立文件 `workgraph.json.lock`(`save` 的 atomic-rename 会换数据文件 inode,
    /// 故不能直接锁数据文件)。锁只包毫秒级闭包,**不覆盖调用方的 LLM turn**。
    /// fs2 锁由 OS 在进程退出/崩溃时自动释放 → 无 stale-lock。
    pub fn with_lock<T, F>(root: &Path, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&mut WorkGraph) -> anyhow::Result<T>,
    {
        use fs2::FileExt;
        let lock_path = root.join("workgraph.json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        let result = (|| {
            let mut g = WorkGraph::read(root);
            let out = f(&mut g)?;
            g.save(root)?;
            Ok(out)
        })();
        let _ = file.unlock();
        result
    }

    fn next_id(&self) -> u64 {
        self.nodes.iter().map(|n| n.id).max().map(|m| m + 1).unwrap_or(1)
    }

    pub fn get(&self, id: u64) -> Option<&Milestone> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Add a milestone. Errors (without mutating) if a dep id is unknown or the
    /// edge set would become cyclic. A brand-new node cannot itself create a
    /// cycle (nothing depends on it yet), but `validate` guards migrations and
    /// any future edge-editing path.
    pub fn add(&mut self, title: &str, acceptance: &str, deps: Vec<u64>) -> anyhow::Result<u64> {
        if title.trim().is_empty() {
            anyhow::bail!("milestone needs a `title`");
        }
        for d in &deps {
            if self.get(*d).is_none() {
                anyhow::bail!("unknown dependency id: {d}");
            }
        }
        let id = self.next_id();
        self.nodes.push(Milestone {
            id,
            title: title.trim().to_string(),
            acceptance: acceptance.trim().to_string(),
            deps,
            status: NodeStatus::Pending,
            verdict: None,
            touched: Vec::new(),
        });
        if let Err(e) = self.validate() {
            self.nodes.pop();
            return Err(e);
        }
        self.recompute_blocked();
        Ok(id)
    }

    /// Set a node's status explicitly, then recompute derived `Blocked` states.
    /// Returns false if the id is unknown.
    pub fn set_status(&mut self, id: u64, status: NodeStatus) -> bool {
        let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        n.status = status;
        self.recompute_blocked();
        true
    }

    /// Remove a node. Errors if any other node still depends on it.
    pub fn remove(&mut self, id: u64) -> anyhow::Result<()> {
        if let Some(dependent) = self.nodes.iter().find(|n| n.deps.contains(&id)) {
            anyhow::bail!("cannot remove #{id}: #{} depends on it", dependent.id);
        }
        let before = self.nodes.len();
        self.nodes.retain(|n| n.id != id);
        if self.nodes.len() == before {
            anyhow::bail!("unknown milestone id: {id}");
        }
        self.recompute_blocked();
        Ok(())
    }

    /// The next milestone to work on: the lowest-id `Pending` node whose deps are
    /// all `Done`. `None` when nothing is ready.
    pub fn next_ready(&self) -> Option<&Milestone> {
        self.nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Pending && self.deps_done(n))
            .min_by_key(|n| n.id)
    }

    fn deps_done(&self, n: &Milestone) -> bool {
        n.deps.iter().all(|d| self.get(*d).map(|m| m.status == NodeStatus::Done).unwrap_or(false))
    }

    /// Recompute the DERIVED `Blocked` state: a not-yet-active node with an unmet
    /// dependency shows `Blocked`; when its deps clear it returns to `Pending`.
    /// Never touches `InProgress` / `NeedsFix` / `Done` / `Hypothesis` / `Locked`
    /// (explicit intent).
    fn recompute_blocked(&mut self) {
        let unmet: Vec<bool> = self.nodes.iter().map(|n| !self.deps_done(n)).collect();
        for (n, blocked) in self.nodes.iter_mut().zip(unmet) {
            match n.status {
                NodeStatus::Pending if blocked => n.status = NodeStatus::Blocked,
                NodeStatus::Blocked if !blocked => n.status = NodeStatus::Pending,
                _ => {}
            }
        }
    }

    /// Validate referential integrity + acyclicity (DFS). Pure check.
    pub fn validate(&self) -> anyhow::Result<()> {
        for n in &self.nodes {
            for d in &n.deps {
                if self.get(*d).is_none() {
                    anyhow::bail!("milestone #{} references unknown dep #{d}", n.id);
                }
            }
        }
        // Cycle detection over the dependency edges (id -> its deps).
        let mut state: std::collections::HashMap<u64, u8> = std::collections::HashMap::new(); // 0=unseen,1=on-stack,2=done
        for n in &self.nodes {
            if self.has_cycle_from(n.id, &mut state) {
                anyhow::bail!("dependency cycle detected at #{}", n.id);
            }
        }
        Ok(())
    }

    fn has_cycle_from(&self, id: u64, state: &mut std::collections::HashMap<u64, u8>) -> bool {
        match state.get(&id) {
            Some(2) => return false,
            Some(1) => return true,
            _ => {}
        }
        state.insert(id, 1);
        if let Some(n) = self.get(id) {
            for d in &n.deps {
                if self.has_cycle_from(*d, state) {
                    return true;
                }
            }
        }
        state.insert(id, 2);
        false
    }

    /// Render the graph for the tool's output. Deterministic (id order).
    pub fn render(&self) -> String {
        if self.nodes.is_empty() {
            return "(empty work graph — add milestones with `milestone add`)".into();
        }
        let ready = self.next_ready().map(|n| n.id);
        let mut lines = Vec::new();
        for n in &self.nodes {
            let mut line = format!("[{}] #{} {}", n.status.tag(), n.id, n.title);
            if !n.deps.is_empty() {
                let ds: Vec<String> = n.deps.iter().map(|d| format!("#{d}")).collect();
                line.push_str(&format!("  (deps: {})", ds.join(",")));
            }
            if Some(n.id) == ready {
                line.push_str("  ▶ready");
            }
            if let Some(v) = &n.verdict {
                line.push_str(&format!("  ✓{v}"));
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    /// Render a concise text summary suitable for system-prompt injection.
    /// Lists ready, in-progress, needs-fix, and blocked nodes in a compact form
    /// so the agent is aware of its outstanding work.
    pub fn render_for_prompt(&self) -> String {
        if self.nodes.is_empty() {
            return String::new();
        }
        let ready = self.next_ready().map(|n| n.id);
        let mut lines = vec![format!(
            "<!-- workgraph: {} nodes, {} done -->",
            self.nodes.len(),
            self.nodes.iter().filter(|n| n.status == NodeStatus::Done).count(),
        )];
        // Bound the prompt: a large graph would bloat every system message. Urgent
        // rows (ready / in_progress / needs_fix) are always shown; the long tail of
        // pending/blocked is capped with an elision note.
        const MAX_PROMPT_NODES: usize = 40;
        let mut shown = 0usize;
        let mut elided = 0usize;
        for n in &self.nodes {
            if n.status == NodeStatus::Done {
                continue;
            }
            let urgent = matches!(n.status, NodeStatus::InProgress | NodeStatus::NeedsFix)
                || Some(n.id) == ready;
            if !urgent && shown >= MAX_PROMPT_NODES {
                elided += 1;
                continue;
            }
            let tag = match n.status {
                NodeStatus::Done => continue,
                NodeStatus::Pending if Some(n.id) == ready => "▶ready",
                NodeStatus::Pending => "  pending",
                NodeStatus::InProgress => "~active",
                NodeStatus::Blocked => "#blocked",
                NodeStatus::NeedsFix => "!needs_fix",
                NodeStatus::Hypothesis => "?hypothesis",
                NodeStatus::Locked => "·locked",
            };
            let deps = if n.deps.is_empty() {
                String::new()
            } else {
                let ds: Vec<String> = n.deps.iter().map(|d| format!("#{d}")).collect();
                format!(" ({})", ds.join(","))
            };
            lines.push(format!("- [{}] #{}{}{}", tag, n.id, n.title, deps));
            shown += 1;
        }
        if elided > 0 {
            lines.push(format!(
                "… ({elided} more pending/blocked hidden — use `milestone list` for the full graph)"
            ));
        }
        lines.join("\n")
    }
}

/// Forward-migration chain (mirrors `session::migrate`).
fn migrate(from: u32, json: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    match from {
        0 => Ok(json), // 0 -> 1: initial schema, nothing to transform.
        other => anyhow::bail!("no workgraph migration registered from schema_version {other}"),
    }
}

/// One-time conversion of a legacy flat `todos.json` into a Work Graph: each todo
/// becomes a dependency-less milestone.
fn migrate_todos(raw: &str) -> Option<WorkGraph> {
    #[derive(Deserialize)]
    struct OldTodo {
        id: u64,
        text: String,
        done: bool,
    }
    let todos: Vec<OldTodo> = serde_json::from_str(raw).ok()?;
    let nodes = todos
        .into_iter()
        .map(|t| Milestone {
            id: t.id,
            title: t.text,
            acceptance: String::new(),
            deps: Vec::new(),
            status: if t.done { NodeStatus::Done } else { NodeStatus::Pending },
            verdict: None,
            touched: Vec::new(),
        })
        .collect();
    Some(WorkGraph { schema_version: WG_SCHEMA_VERSION, nodes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wg() -> WorkGraph {
        WorkGraph::default()
    }

    #[test]
    fn add_assigns_ids_and_next_ready_respects_deps() {
        let mut g = wg();
        let a = g.add("data model", "", vec![]).unwrap();
        let b = g.add("logic", "", vec![a]).unwrap();
        assert_eq!((a, b), (1, 2));
        // Only the dep-free node is ready; #2 is blocked behind #1.
        assert_eq!(g.next_ready().map(|n| n.id), Some(1));
        assert_eq!(g.get(2).unwrap().status, NodeStatus::Blocked);

        g.set_status(1, NodeStatus::Done);
        // #1 done → #2 unblocks and becomes the next ready node.
        assert_eq!(g.next_ready().map(|n| n.id), Some(2));
        assert_eq!(g.get(2).unwrap().status, NodeStatus::Pending);
    }

    #[test]
    fn add_rejects_unknown_dep() {
        let mut g = wg();
        assert!(g.add("x", "", vec![99]).is_err());
        assert!(g.nodes.is_empty());
    }

    #[test]
    fn explicit_status_not_clobbered_by_recompute() {
        let mut g = wg();
        let a = g.add("a", "", vec![]).unwrap();
        let b = g.add("b", "", vec![a]).unwrap();
        g.set_status(b, NodeStatus::InProgress); // human insists, deps unmet
        // recompute must NOT downgrade an explicit InProgress to Blocked.
        assert_eq!(g.get(b).unwrap().status, NodeStatus::InProgress);
    }

    #[test]
    fn validate_detects_cycle() {
        // Hand-build a 1->2->1 cycle (the tool's add-only path can't create one).
        let g = WorkGraph {
            schema_version: WG_SCHEMA_VERSION,
            nodes: vec![
                Milestone { id: 1, title: "a".into(), acceptance: String::new(), deps: vec![2], status: NodeStatus::Pending, verdict: None, touched: vec![] },
                Milestone { id: 2, title: "b".into(), acceptance: String::new(), deps: vec![1], status: NodeStatus::Pending, verdict: None, touched: vec![] },
            ],
        };
        assert!(g.validate().is_err());
    }

    #[test]
    fn remove_rejected_when_depended_on() {
        let mut g = wg();
        let a = g.add("a", "", vec![]).unwrap();
        g.add("b", "", vec![a]).unwrap();
        assert!(g.remove(a).is_err()); // #2 depends on #1
        assert!(g.get(a).is_some());
    }

    #[test]
    fn save_load_roundtrip_and_version_guard() {
        let dir = std::env::temp_dir().join(format!("wg_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut g = wg();
        g.add("only", "accept it", vec![]).unwrap();
        g.save(&dir).unwrap();
        let raw = std::fs::read_to_string(path(&dir)).unwrap();
        let back = WorkGraph::load(&raw).unwrap();
        assert_eq!(back, g);
        // Future version is refused.
        let future = r#"{"schema_version": 999, "nodes": []}"#;
        assert!(WorkGraph::load(future).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_legacy_flat_todos() {
        let raw = r#"[{"id":1,"text":"first","done":true},{"id":2,"text":"second","done":false}]"#;
        let g = migrate_todos(raw).unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.get(1).unwrap().status, NodeStatus::Done);
        assert_eq!(g.get(2).unwrap().status, NodeStatus::Pending);
        assert!(g.get(2).unwrap().deps.is_empty());
    }

    #[test]
    fn node_status_hypothesis_and_locked_round_trip() {
        let mut g = wg();
        g.add("root cause candidate", "", vec![]).unwrap();
        g.set_status(1, NodeStatus::Hypothesis);
        assert_eq!(g.get(1).unwrap().status, NodeStatus::Hypothesis);
        assert_eq!(g.get(1).unwrap().status.tag(), "?");
        assert_eq!(g.get(1).unwrap().status.as_str(), "hypothesis");

        g.set_status(1, NodeStatus::Locked);
        assert_eq!(g.get(1).unwrap().status, NodeStatus::Locked);
        assert_eq!(g.get(1).unwrap().status.tag(), "·");
        assert_eq!(g.get(1).unwrap().status.as_str(), "locked");

        // Serialize and deserialize to verify JSON round-trip.
        let dir = std::env::temp_dir().join(format!("wg_stat_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        g.save(&dir).unwrap();
        let raw = std::fs::read_to_string(path(&dir)).unwrap();
        let back = WorkGraph::load(&raw).unwrap();
        assert_eq!(back.get(1).unwrap().status, NodeStatus::Locked);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_for_prompt_omits_done_and_shows_ready() {
        let mut g = wg();
        g.add("first", "", vec![]).unwrap();
        g.add("second", "", vec![1]).unwrap();
        g.set_status(1, NodeStatus::Done);
        let prompt = g.render_for_prompt();
        // #1 is done → omitted in prompt.
        assert!(!prompt.contains("first"), "done node should be omitted: {prompt}");
        // #2 is ready after #1 is done.
        assert!(prompt.contains("▶ready"), "ready marker should be present: {prompt}");
        assert!(prompt.contains("second"), "pending node should be listed: {prompt}");
    }

    #[test]
    fn render_for_prompt_empty_returns_empty() {
        let g = wg();
        assert!(g.render_for_prompt().is_empty(), "empty graph returns empty");
    }

    #[test]
    fn render_for_prompt_shows_all_statuses() {
        let mut g = wg();
        g.add("ready", "do it", vec![]).unwrap();
        g.add("active", "", vec![]).unwrap();
        g.set_status(2, NodeStatus::InProgress);
        // Blocked manually: add a dep that won't be done.
        g.add("blocked", "", vec![1]).unwrap(); // #1 is pending → blocked
        g.add("fixme", "", vec![]).unwrap();
        g.set_status(4, NodeStatus::NeedsFix);
        g.add("done", "", vec![]).unwrap();
        g.set_status(5, NodeStatus::Done);
        g.add("hyp", "", vec![]).unwrap();
        g.set_status(6, NodeStatus::Hypothesis);
        g.add("locked", "", vec![]).unwrap();
        g.set_status(7, NodeStatus::Locked);

        let prompt = g.render_for_prompt();
        // Done node omitted from list but header may mention count.
        assert!(!prompt.contains("- [x] #5"), "done node should be omitted: {prompt}");
        // Others present with correct tags.
        assert!(prompt.contains("▶ready"), "ready marker: {prompt}");
        assert!(prompt.contains("~active"), "active marker: {prompt}");
        assert!(prompt.contains("#blocked"), "blocked marker: {prompt}");
        assert!(prompt.contains("!needs_fix"), "needs_fix marker: {prompt}");
        assert!(prompt.contains("?hypothesis"), "hypothesis marker: {prompt}");
        assert!(prompt.contains("·locked"), "locked marker: {prompt}");
    }

    #[test]
    fn render_for_prompt_caps_large_graph() {
        let mut g = wg();
        // 50 pending nodes exceeds MAX_PROMPT_NODES (40); the non-urgent tail
        // is elided with a note, while the urgent ready node still shows.
        for i in 0..50 {
            g.add(&format!("task-{i}"), "", vec![]).unwrap();
        }
        let prompt = g.render_for_prompt();
        assert!(
            prompt.contains("more pending/blocked hidden"),
            "large graph should elide the tail: {prompt}"
        );
        assert!(prompt.contains("▶ready"), "ready node must survive the cap: {prompt}");
    }

    #[test]
    fn with_lock_prevents_lost_update_under_concurrency() {
        use std::sync::Arc;
        use std::thread;
        let dir = std::env::temp_dir().join(format!("cc_wglock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        WorkGraph::default().save(&dir).unwrap();
        let dir = Arc::new(dir);
        let n = 8;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let d = Arc::clone(&dir);
                thread::spawn(move || {
                    WorkGraph::with_lock(&d, |g| {
                        g.add(&format!("t{i}"), "", vec![])?;
                        Ok(())
                    })
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let g = WorkGraph::read(&dir);
        assert_eq!(g.nodes.len(), n, "no milestone lost under concurrent with_lock writers");
        let _ = std::fs::remove_dir_all(&*dir);
    }

    #[test]
    fn with_lock_releases_so_sequential_calls_do_not_deadlock() {
        let dir = std::env::temp_dir().join(format!("cc_wgseq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        WorkGraph::default().save(&dir).unwrap();
        for _ in 0..5 {
            WorkGraph::with_lock(&dir, |g| {
                g.add("t", "", vec![])?;
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(WorkGraph::read(&dir).nodes.len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
