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
}

impl NodeStatus {
    fn tag(self) -> &'static str {
        match self {
            NodeStatus::Pending => " ",
            NodeStatus::InProgress => "~",
            NodeStatus::Blocked => "#",
            NodeStatus::NeedsFix => "!",
            NodeStatus::Done => "x",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Pending => "pending",
            NodeStatus::InProgress => "in_progress",
            NodeStatus::Blocked => "blocked",
            NodeStatus::NeedsFix => "needs_fix",
            NodeStatus::Done => "done",
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
    /// Never touches `InProgress` / `NeedsFix` / `Done` (explicit intent).
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
}
