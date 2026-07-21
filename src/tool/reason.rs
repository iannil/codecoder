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
         action = add | status | margin | list | trace | to_milestone | link. \
         `add <question>` creates a causal node. \
         `status <id> <hypothesis|locked>` sets verification state. \
         `margin <id> [margin] [leverage] [terminal]` sets metadata. \
         `list` renders the causal tree. \
         `trace <id>` walks from a node up to the root. \
         `to_milestone <id>` converts a locked node into a workgraph milestone. \
         `link <id>` links the current session leaf to a causal node (for hypothesis tracking)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "status", "margin", "list", "trace", "to_milestone", "link"]
                },
                "id": { "type": "integer" },
                "question": { "type": "string", "description": "The causal question for `add`" },
                "status": { "type": "string", "enum": ["hypothesis", "locked"], "description": "Verification state for `status`" },
                "margin": { "type": "string", "description": "Available margin description" },
                "leverage": { "type": "string", "description": "Leverage level (high/medium/low)" },
                "terminal": { "type": "string", "description": "Terminal reason: excluded | natural_law | boundary" },
                "milestone_title": { "type": "string", "description": "Optional title for the milestone (default: the node's question)" }
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
            "to_milestone" => self.to_milestone(args, ctx),
            "link" => self.link(args, ctx),
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
            // After locking, suggest milestone conversion if margin+leverage are set.
            let extra = if status == "locked" {
                if let Some(n) = tree.nodes.iter().find(|n| n.id == id) {
                    if n.margin.is_some() && n.leverage.is_some() && n.terminal.is_none() {
                        format!(
                            "\n\n💡 node #{id} is locked with margin+leverage — \
                             convert to a workgraph milestone: `reason action=to_milestone id={id}`"
                        )
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            Ok(ToolOutput::ok(format!("node #{id} status → {status}{extra}")))
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

    fn to_milestone(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let id = args.get("id").and_then(Value::as_u64);
        let Some(id) = id else {
            return Ok(ToolOutput::err("to_milestone requires `id`"));
        };
        let tree = CausalTree::load(ctx.root);
        let node = tree.nodes.iter().find(|n| n.id == id);
        let Some(node) = node else {
            return Ok(ToolOutput::err(format!("unknown node id: {id}")));
        };
        if node.status != "locked" {
            return Ok(ToolOutput::err(format!(
                "node #{id} is '{status}', must be 'locked' to convert to milestone",
                status = node.status
            )));
        }
        let title = args.get("milestone_title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or(&node.question)
            .to_string();
        let acceptance = format!(
            "Resolve the causal finding: {}. margin: {} leverage: {}",
            node.question,
            node.margin.as_deref().unwrap_or("(unspecified)"),
            node.leverage.as_deref().unwrap_or("(unspecified)"),
        );

        let mut wg = crate::workgraph::WorkGraph::read(ctx.root);
        match wg.add(&title, &acceptance, vec![]) {
            Ok(new_id) => {
                wg.save(ctx.root)?;
                Ok(ToolOutput::ok(format!(
                    "converted inference node #{id} → workgraph milestone #{new_id}: {title}"
                )))
            }
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
    }

    fn link(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let id = args.get("id").and_then(Value::as_u64).unwrap_or(0);
        let tree = CausalTree::load(ctx.root);
        if !tree.nodes.iter().any(|n| n.id == id) {
            return Ok(ToolOutput::err(format!("unknown causal node #{id}")));
        }
        Ok(ToolOutput::ok(format!("session leaf linked to causal node #{id} (status: hypothesis)"))
            .with_session_meta_mark(json!({"causal_node": id, "status": "hypothesis"})))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ruling: Option<String>,
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
            ruling: None,
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
            let mut line = format!("{tag} #{} {}  {}", n.id, n.question, target);
            if let Some(r) = &n.ruling {
                line.push_str(&format!(" [ruled out: {r}]"));
            }
            lines.push(line);
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
    if let Some(r) = &n.ruling {
        line.push_str(&format!(" [ruled out: {r}]"));
    }
    out.push(line);
    // Sort children by id for deterministic rendering.
    let mut sorted = children.get(&Some(n.id)).into_iter().flatten().copied().collect::<Vec<_>>();
    sorted.sort_by_key(|c| c.id);
    for child in sorted {
        write_node(child, depth + 1, children, out);
    }
}

/// Phase E: record a ruling on a causal node when a session branch exploring it
/// is abandoned. Loads `causal_tree.json`, sets the node's `ruling`, saves.
pub fn record_ruling(root: &Path, causal_node_id: u64, ruling: &str) -> anyhow::Result<()> {
    let mut tree = CausalTree::load(root);
    let node = tree.nodes.iter_mut().find(|n| n.id == causal_node_id)
        .ok_or_else(|| anyhow::anyhow!("unknown causal node #{causal_node_id}"))?;
    node.ruling = Some(ruling.to_string());
    tree.save(root)
}

// ── Cross-Session Inference Tree Collection ─────────────────────────────

/// A cross-session inference node collected from session files' `meta` fields.
#[derive(Debug, Clone)]
pub struct CrossSessionNode {
    /// The session file stem (e.g., "session-1718...")
    pub session_id: String,
    /// The message id within the session
    pub message_id: crate::message::MessageId,
    /// A brief preview of the message content (first ~60 chars)
    pub preview: String,
}

/// Collects all inference-tree hypothesis nodes across all session files.
/// Scans `sessions/*.json` under `root`, filters entries where `meta.status == "hypothesis"`,
/// and returns them as `CrossSessionNode` entries with a preview of the message content.
///
/// This enables cross-session causal reasoning: an agent can see what hypotheses
/// were raised in other sessions, enabling follow-up investigation or convergence
/// of related debugging efforts.
pub fn collect_cross_session_hypotheses(root: &Path) -> Vec<CrossSessionNode> {
    use crate::session::{SessionManager, Session, sessions_dir};

    let mut out = Vec::new();
    let mgr = SessionManager::new(root);

    for meta in mgr.list() {
        let session_path = sessions_dir(root).join(format!("{}.json", meta.id));
        // Skip files we can't read (permissions, corruption, etc.)
        let raw = match std::fs::read_to_string(&session_path) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let session = match Session::load(&raw) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Collect entries with meta.status == "hypothesis"
        for entry in &session.entries {
            let is_hypothesis = entry.meta
                .as_ref()
                .and_then(|m| m.get("status"))
                .and_then(|s| s.as_str())
                .map(|s| s == "hypothesis")
                .unwrap_or(false);

            if is_hypothesis {
                let preview = extract_preview(&entry.message);
                out.push(CrossSessionNode {
                    session_id: meta.id.clone(),
                    message_id: entry.message.id,
                    preview,
                });
            }
        }
    }

    out
}

/// Extract a brief preview from a message (first ~60 characters of text content).
fn extract_preview(msg: &crate::message::Message) -> String {
    use crate::message::MessageItem;
    for item in &msg.items {
        if let MessageItem::Text { text } = item {
            let truncated = if text.len() > 60 {
                format!("{}…", &text[..60])
            } else {
                text.clone()
            };
            return truncated;
        }
    }
    "(no text preview)".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, SessionEntry};
    use crate::message::{Message, Role, MessageItem};

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
    fn collect_cross_session_hypotheses_gathers_meta_nodes() {
        let dir = std::env::temp_dir().join(format!("cc_cross_session_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Create two session files with hypothesis-meta entries
        let sessions_dir = dir.join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Session 1: Two entries, one with hypothesis status
        let mut session1 = Session::new("gpt-4o");
        let msg1 = Message {
            id: 0,
            role: Role::User,
            items: vec![MessageItem::Text { text: "Why is the server slow?".to_string() }],
        };
        session1.entries.push(SessionEntry {
            message: msg1,
            parent: None,
            meta: Some(serde_json::json!({"status": "hypothesis"})),
        });

        let msg2 = Message {
            id: 1,
            role: Role::Assistant,
            items: vec![MessageItem::Text { text: "Let me investigate...".to_string() }],
        };
        session1.entries.push(SessionEntry {
            message: msg2,
            parent: Some(0),
            meta: None,
        });
        session1.leaf = Some(1);

        let session1_path = sessions_dir.join("session-aaa.json");
        session1.save(&session1_path).unwrap();

        // Session 2: One entry with hypothesis status
        let mut session2 = Session::new("gpt-4o");
        let msg3 = Message {
            id: 10,
            role: Role::User,
            items: vec![MessageItem::Text { text: "Database connection timeout".to_string() }],
        };
        session2.entries.push(SessionEntry {
            message: msg3,
            parent: None,
            meta: Some(serde_json::json!({"status": "hypothesis"})),
        });
        session2.leaf = Some(10);

        let session2_path = sessions_dir.join("session-bbb.json");
        session2.save(&session2_path).unwrap();

        // Test the collection function
        let hypotheses = collect_cross_session_hypotheses(&dir);

        // Should find 2 hypothesis nodes from both sessions
        assert_eq!(hypotheses.len(), 2, "expected 2 hypotheses, got {}: {:?}", hypotheses.len(), hypotheses);

        // Verify session_id and message_id mapping
        let h1 = hypotheses.iter().find(|h| h.session_id == "session-aaa" && h.message_id == 0);
        assert!(h1.is_some(), "should find hypothesis from session-aaa message 0");
        assert!(h1.unwrap().preview.contains("server slow"), "preview should contain 'server slow': {}", h1.unwrap().preview);

        let h2 = hypotheses.iter().find(|h| h.session_id == "session-bbb" && h.message_id == 10);
        assert!(h2.is_some(), "should find hypothesis from session-bbb message 10");
        assert!(h2.unwrap().preview.contains("timeout"), "preview should contain 'timeout': {}", h2.unwrap().preview);

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

    #[test]
    fn to_milestone_requires_locked() {
        let dir = std::env::temp_dir().join(format!("cc_reason_tm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        // Add a node (starts as hypothesis).
        Reason.run(json!({ "action": "add", "question": "why?" }), &mut ctx).unwrap();
        // Try to_milestone on hypothesis → should error.
        let out = Reason.run(json!({ "action": "to_milestone", "id": 0 }), &mut ctx).unwrap();
        assert!(out.is_error, "hypothesis node should not convert: {}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn to_milestone_creates_milestone_from_locked_node() {
        let dir = std::env::temp_dir().join(format!("cc_reason_tm2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        Reason.run(json!({ "action": "add", "question": "root cause X" }), &mut ctx).unwrap();
        Reason.run(json!({ "action": "status", "id": 0, "status": "locked" }), &mut ctx).unwrap();
        Reason.run(json!({ "action": "margin", "id": 0, "margin": "can fix config", "leverage": "high" }), &mut ctx).unwrap();
        let out = Reason.run(json!({ "action": "to_milestone", "id": 0 }), &mut ctx).unwrap();
        assert!(!out.is_error, "to_milestone failed: {}", out.content);
        assert!(out.content.contains("milestone #1"), "should create milestone #1: {}", out.content);
        // Verify the workgraph has the new milestone.
        let wg = crate::workgraph::WorkGraph::read(&dir);
        assert_eq!(wg.nodes.len(), 1);
        assert_eq!(wg.nodes[0].title, "root cause X");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn link_returns_session_meta_mark_for_known_node() {
        let dir = std::env::temp_dir().join(format!("cc_reasonlink_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        // create a causal node first
        let _ = Reason.run(json!({"action":"add","question":"why?"}), &mut ctx).unwrap();
        // link node 0
        let out = Reason.run(json!({"action":"link","id":0}), &mut ctx).unwrap();
        assert!(!out.is_error, "link on known node should succeed");
        assert_eq!(
            out.session_meta_mark,
            Some(json!({"causal_node": 0, "status": "hypothesis"})),
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn link_unknown_node_errors_without_mark() {
        let dir = std::env::temp_dir().join(format!("cc_reasonlink2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = Reason.run(json!({"action":"link","id":99}), &mut ctx).unwrap();
        assert!(out.is_error, "link on unknown node should error");
        assert!(out.session_meta_mark.is_none(), "no mark on error");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_ruling_writes_to_causal_node() {
        let dir = std::env::temp_dir().join(format!("cc_recordruling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let _ = Reason.run(json!({"action":"add","question":"why?"}), &mut ctx).unwrap();
        record_ruling(&dir, 0, "ruled out: too slow").unwrap();
        let tree = CausalTree::load(&dir);
        let node = tree.nodes.iter().find(|n| n.id == 0).unwrap();
        assert_eq!(node.ruling.as_deref(), Some("ruled out: too slow"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn causal_tree_loads_old_file_without_ruling() {
        // an old causal_tree.json with no `ruling` fields still loads
        let dir = std::env::temp_dir().join(format!("cc_oldruling_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("causal_tree.json"),
            r#"{"nodes":[{"id":1,"question":"q","status":"hypothesis"}],"next_id":2}"#,
        ).unwrap();
        let tree = CausalTree::load(&dir);
        assert_eq!(tree.nodes.len(), 1);
        assert!(tree.nodes[0].ruling.is_none(), "old file: ruling defaults to None");
        std::fs::remove_dir_all(&dir).ok();
    }
}