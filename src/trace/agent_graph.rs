//! AgentGraph — reconstructs the sub-agent call tree from trace events.
//! Reads AgentGraphEdge point events and SubAgent spans to build a tree.

use crate::trace::reader::SpanNode;
use crate::trace::reader::TraceReader;
use crate::trace::types::*;
use std::collections::HashMap;

/// Status of an agent node.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentNode {
    pub span_id: String,
    pub agent_id: String,
    pub label: String,
    pub parent_span_id: Option<String>,
    pub launch_seq: u32,
    pub status: AgentStatus,
    pub summary: Option<String>,
    pub turn_count: usize,
}

#[derive(Debug, Clone)]
pub struct AgentEdge {
    pub parent_span_id: String,
    pub child_span_id: String,
    pub label: String,
    pub launch_seq: u32,
}

#[derive(Debug, Clone)]
pub struct AgentGraph {
    pub nodes: Vec<AgentNode>,
    pub edges: Vec<AgentEdge>,
}

impl AgentGraph {
    pub fn new() -> Self {
        AgentGraph { nodes: Vec::new(), edges: Vec::new() }
    }

    /// Build an AgentGraph from a TraceReader.
    pub fn from_reader(reader: &TraceReader) -> std::io::Result<Self> {
        let (_meta, tree) = reader.read_tree()?;
        let mut graph = AgentGraph::new();

        // Collect edges from point events and sub-agent spans from tree
        let mut edge_map: HashMap<String, Vec<AgentEdge>> = HashMap::new();
        Self::collect_from_tree(&tree, &mut graph, &mut edge_map);

        // Build edges from edge_map
        for (_parent, children) in &edge_map {
            for child in children {
                graph.edges.push(child.clone());
            }
        }

        Ok(graph)
    }

    fn collect_from_tree(
        tree: &[SpanNode],
        graph: &mut AgentGraph,
        edge_map: &mut HashMap<String, Vec<AgentEdge>>,
    ) {
        for node in tree {
            // Check direct_events for AgentGraphEdge
            for ev in &node.direct_events {
                if let EventKind::AgentGraphEdge(edge) = &ev.kind {
                    let e = AgentEdge {
                        parent_span_id: edge.parent_span_id.clone(),
                        child_span_id: edge.child_span_id.clone(),
                        label: edge.label.clone(),
                        launch_seq: edge.launch_seq,
                    };
                    edge_map.entry(e.parent_span_id.clone()).or_default().push(e);
                }
            }

            // If this node is a sub-agent, add it to nodes
            if node.span.kind == SpanKind::SubAgent {
                let status = match &node.end {
                    Some(end) => {
                        if end.meta.get("is_error").and_then(|v: &serde_json::Value| v.as_bool()).unwrap_or(false) {
                            AgentStatus::Failed
                        } else {
                            AgentStatus::Completed
                        }
                    }
                    None => AgentStatus::Running,
                };
                let agent_id = node.span.meta.get("agent_id").and_then(|v: &serde_json::Value| v.as_str()).unwrap_or("").to_string();
                let label = node.span.meta.get("label").and_then(|v: &serde_json::Value| v.as_str()).unwrap_or("").to_string();
                let launch_seq = node.span.meta.get("launch_seq").and_then(|v: &serde_json::Value| v.as_u64()).unwrap_or(0) as u32;

                // Count turns inside this sub-agent
                let turn_count = count_turns(&node.children);

                graph.nodes.push(AgentNode {
                    span_id: node.span.span_id.clone(),
                    agent_id,
                    label,
                    parent_span_id: node.span.parent_id.clone(),
                    launch_seq,
                    status,
                    summary: None,
                    turn_count,
                });
            }

            Self::collect_from_tree(&node.children, graph, edge_map);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Render as a simple ASCII tree.
    pub fn render_tree(&self) -> String {
        if self.is_empty() {
            return "  (no sub-agents)\n".to_string();
        }

        let mut out = String::new();
        let mut children_of: HashMap<Option<String>, Vec<&AgentNode>> = HashMap::new();
        for node in &self.nodes {
            children_of.entry(node.parent_span_id.clone()).or_default().push(node);
        }

        // Find roots (no parent, or parent not in nodes)
        let roots = children_of.remove(&None).unwrap_or_default();

        // Sort by launch_seq
        let mut sorted_roots: Vec<&AgentNode> = roots.iter().copied().collect();
        sorted_roots.sort_by(|a, b| a.launch_seq.cmp(&b.launch_seq));

        for root in &sorted_roots {
            render_agent_node(root, 0, &children_of, &mut out);
        }

        // Remaining nodes not connected to any root
        let mut remaining: Vec<&AgentNode> = Vec::new();
        let mut visited = std::collections::HashSet::new();
        // Mark all nodes already rendered as part of the root tree
        for root in &sorted_roots {
            visited.insert(root.span_id.as_str());
            collect_visited_ids(root, &children_of, &mut visited);
        }
        for (_key, nodes) in &children_of {
            for node in nodes {
                if !visited.contains(node.span_id.as_str()) {
                    remaining.push(node);
                }
            }
        }
        remaining.sort_by(|a, b| a.launch_seq.cmp(&b.launch_seq));
        for node in &remaining {
            render_agent_node(node, 0, &children_of, &mut out);
        }

        out
    }

    /// Render for LLM consumption.
    pub fn render_for_llm(&self) -> String {
        let mut out = String::new();
        out.push_str("## Agent Call Tree\n");
        out.push_str(&format!("  Total sub-agents: {}\n", self.nodes.len()));
        out.push_str(&self.render_tree());
        out
    }
}

fn count_turns(children: &[SpanNode]) -> usize {
    let mut count = 0;
    for child in children {
        if child.span.kind == SpanKind::Turn {
            count += 1;
        }
        count += count_turns(&child.children);
    }
    count
}

fn collect_visited_ids<'a>(
    node: &AgentNode,
    children_of: &HashMap<Option<String>, Vec<&'a AgentNode>>,
    visited: &mut std::collections::HashSet<&'a str>,
) {
    if let Some(children) = children_of.get(&Some(node.span_id.clone())) {
        for child in children {
            visited.insert(child.span_id.as_str());
            collect_visited_ids(child, children_of, visited);
        }
    }
}

fn render_agent_node(
    node: &AgentNode,
    depth: usize,
    children_of: &HashMap<Option<String>, Vec<&AgentNode>>,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    let status_str = match node.status {
        AgentStatus::Completed => "✓",
        AgentStatus::Failed => "✗",
        AgentStatus::Running => "⋯",
        AgentStatus::Cancelled => "⊘",
    };
    let turn_info = if node.turn_count > 0 {
        format!(" ({} turns)", node.turn_count)
    } else {
        String::new()
    };
    out.push_str(&format!("{indent}├─ {status_str} sub-agent: \"{label}\" {id}{turn_info}\n", indent = indent, status_str = status_str, label = node.label, id = node.span_id, turn_info = turn_info));
    // Render children
    if let Some(children) = children_of.get(&Some(node.span_id.clone())) {
        let mut sorted = children.clone();
        sorted.sort_by(|a, b| a.launch_seq.cmp(&b.launch_seq));
        for child in sorted {
            render_agent_node(child, depth + 1, children_of, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_trace(path: &std::path::Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn agent_graph_from_empty_trace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1001.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let graph = AgentGraph::from_reader(&reader).unwrap();
        assert!(graph.is_empty());
    }

    #[test]
    fn agent_graph_from_single_subagent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"agent_graph_edge","ts":1000.1,"meta":{"parent_span_id":"sp_001","child_span_id":"sp_002","label":"refactor main.rs","launch_seq":1}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"sub_agent","ts":1000.1,"meta":{"agent_id":"agt_xyz","label":"refactor main.rs","launch_seq":1}}
{"type":"s","span_id":"sp_003","parent_id":"sp_002","kind":"turn","ts":1000.2,"meta":{}}
{"type":"e","span_id":"sp_003","ts":1001.0,"meta":{}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1002.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let graph = AgentGraph::from_reader(&reader).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.nodes[0].label, "refactor main.rs");
        assert_eq!(graph.nodes[0].turn_count, 1);
    }

    #[test]
    fn render_tree_prints_tree() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"agent_graph_edge","ts":1000.1,"meta":{"parent_span_id":"sp_001","child_span_id":"sp_002","label":"fix bug","launch_seq":1}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"sub_agent","ts":1000.1,"meta":{"agent_id":"agt_abc","label":"fix bug","launch_seq":1}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1002.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let graph = AgentGraph::from_reader(&reader).unwrap();
        let rendered = graph.render_tree();
        assert!(rendered.contains("sub-agent"));
        assert!(rendered.contains("fix bug"));
    }
}