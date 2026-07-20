// Inference-tree tool (first-class citizen #3): manages causal-reasoning nodes
// persisted to `causal_tree.json`. Each node is a candidate cause with a
// verification status (hypothesis / locked) and optional margin/leverage metadata.
// Permission::None — local scratch, no dangerous side effects.
// Sub-agents are excluded (same as `milestone`/`plan`/`memory`).
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde::{Deserialize, Serialize};
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
         `add <question>` creates a causal node. \
         `status <id> <hypothesis|locked>` sets verification state. \
         `margin <id> [margin] [leverage] [terminal]` sets metadata. \
         `list` renders the causal tree. \
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
                "terminal": { "type": "string", "description": "Terminal reason: excluded | natural_law | boundary" }
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
        let parent = args.get("parent").and_then(Value::as_u64);
        let mut tree = CausalTree::load(ctx.root);
        let id = tree.add(question, parent);
        tree.save(ctx.root)?;
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
        if margin.is_none() && leverage.is_none() && terminal.is_none() {
            return Ok(ToolOutput::err("provide at least one of: margin, leverage, terminal"));
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<u64>,
    question: String,
    #[serde(default)]
    status: String, // "hypothesis" | "locked"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    margin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leverage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CausalTree {
    nodes: Vec<CausalNode>,
    next_id: u64,
}

impl CausalTree {
    fn path(root: &Path) -> std::path::PathBuf {
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
        // Atomic write: temp + rename (mirrors session.rs / workgraph.rs).
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

    fn set_margin(
        &mut self,
        id: u64,
        margin: Option<String>,
        leverage: Option<String>,
        terminal: Option<String>,
    ) -> bool {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            if let Some(m) = margin {
                n.margin = Some(m);
            }
            if let Some(l) = leverage {
                n.leverage = Some(l);
            }
            if let Some(t) = terminal {
                n.terminal = Some(t);
            }
            true
        } else {
            false
        }
    }

    fn render(&self) -> String {
        if self.nodes.is_empty() {
            return "(empty causal tree — add nodes with `reason add`)".into();
        }
        // Build parent→children map.
        let mut children: std::collections::HashMap<Option<u64>, Vec<&CausalNode>> =
            std::collections::HashMap::new();
        for n in &self.nodes {
            children.entry(n.parent).or_default().push(n);
        }
        let mut lines = Vec::new();
        // Roots: nodes with no parent, sorted by id.
        let mut roots: Vec<&CausalNode> = children.get(&None).into_iter().flatten().copied().collect();
        roots.sort_by_key(|n| n.id);
        for root in roots {
            write_node(root, 0, &children, &mut lines);
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
            let target = if i == path.len() - 1 { "◀" } else { "↑" };
            lines.push(format!("{tag} #{} {}  {}", n.id, n.question, target));
        }
        lines.join("\n")
    }
}

/// Recursively render a node and its children.
fn write_node(
    n: &CausalNode,
    depth: usize,
    children: &std::collections::HashMap<Option<u64>, Vec<&CausalNode>>,
    out: &mut Vec<String>,
) {
    let indent = "  ".repeat(depth);
    let tag = match n.status.as_str() {
        "locked" => "✓",
        _ => "?",
    };
    let mut line = format!("{indent}{tag} #{} {}", n.id, n.question);
    let mut meta_parts = Vec::new();
    if let Some(m) = &n.margin {
        meta_parts.push(format!("margin:{}", m));
    }
    if let Some(l) = &n.leverage {
        meta_parts.push(format!("leverage:{}", l));
    }
    if let Some(t) = &n.terminal {
        meta_parts.push(format!("terminal:{}", t));
    }
    if !meta_parts.is_empty() {
        line.push_str(&format!("  [{}]", meta_parts.join(", ")));
    }
    out.push(line);
    // Sort children by id for deterministic rendering.
    let mut sorted = children.get(&Some(n.id)).into_iter().flatten().copied().collect::<Vec<_>>();
    sorted.sort_by_key(|c| c.id);
    for child in sorted {
        write_node(child, depth + 1, children, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_creates_node_with_hypothesis_status() {
        let dir = std::env::temp_dir().join(format!("cc_reason_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = Reason.run(json!({ "action": "add", "question": "why is this failing?" }), &mut ctx).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("#0"), "expected node #0: {}", out.content);
        // Load the tree from disk and verify.
        let tree = CausalTree::load(&dir);
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].status, "hypothesis");
        assert_eq!(tree.nodes[0].question, "why is this failing?");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_requires_question() {
        let dir = std::env::temp_dir().join(format!("cc_reason_q_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = Reason.run(json!({ "action": "add" }), &mut ctx).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("missing required"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_and_margin_update() {
        let dir = std::env::temp_dir().join(format!("cc_reason_sm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        // Add a node.
        Reason.run(json!({ "action": "add", "question": "root cause" }), &mut ctx).unwrap();
        // Lock it.
        let out = Reason.run(json!({ "action": "status", "id": 0, "status": "locked" }), &mut ctx).unwrap();
        assert!(!out.is_error, "status update failed: {}", out.content);
        // Set margin & leverage.
        let out = Reason.run(
            json!({ "action": "margin", "id": 0, "margin": "can change config", "leverage": "high" }),
            &mut ctx,
        ).unwrap();
        assert!(!out.is_error, "margin update failed: {}", out.content);
        // Verify on disk.
        let tree = CausalTree::load(&dir);
        assert_eq!(tree.nodes[0].status, "locked");
        assert_eq!(tree.nodes[0].margin.as_deref(), Some("can change config"));
        assert_eq!(tree.nodes[0].leverage.as_deref(), Some("high"));
        assert_eq!(tree.nodes[0].terminal, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_renders_tree() {
        let dir = std::env::temp_dir().join(format!("cc_reason_l_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        // Add root and child.
        Reason.run(json!({ "action": "add", "question": "why?" }), &mut ctx).unwrap();
        Reason.run(json!({ "action": "add", "question": "because X", "parent": 0 }), &mut ctx).unwrap();
        Reason.run(json!({ "action": "add", "question": "because Y", "parent": 0 }), &mut ctx).unwrap();
        let out = Reason.run(json!({ "action": "list" }), &mut ctx).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("why?"), "list should include root: {}", out.content);
        assert!(out.content.contains("because X"), "list should include child: {}", out.content);
        assert!(out.content.contains("because Y"), "list should include child: {}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn trace_walks_to_root() {
        let dir = std::env::temp_dir().join(format!("cc_reason_t_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        Reason.run(json!({ "action": "add", "question": "a" }), &mut ctx).unwrap();
        Reason.run(json!({ "action": "add", "question": "b", "parent": 0 }), &mut ctx).unwrap();
        let out = Reason.run(json!({ "action": "trace", "id": 1 }), &mut ctx).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("#0 a"), "trace should include root: {}", out.content);
        assert!(out.content.contains("#1 b"), "trace should include leaf: {}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_id_errors() {
        let dir = std::env::temp_dir().join(format!("cc_reason_e_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = Reason.run(json!({ "action": "status", "id": 99, "status": "locked" }), &mut ctx).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("unknown node"));
        std::fs::remove_dir_all(&dir).ok();
    }
}