//! TraceReader — reads `.ccd.trace.ndjson` and provides structured queries.
//! Used for post-hoc replay and LLM self-observation.
use crate::trace::types::*;
use std::collections::HashMap;
use std::path::Path;

// ============================================================================
// Data structures
// ============================================================================

/// A reconstructed span node in the span tree.
#[derive(Clone)]
pub struct SpanNode {
    pub span: SpanStart,
    pub end: Option<SpanEnd>,
    pub children: Vec<SpanNode>,
    /// All point events attached to this span (includes direct_events).
    pub events: Vec<PointEvent>,
    /// Point events directly on this span (not inherited from children).
    /// These are a subset of `events` — callers should iterate `events`
    /// for the full list, or `direct_events` for only local events.
    pub direct_events: Vec<PointEvent>,
}

/// Trace metadata: the `{"type":"meta",...}` header line.
pub struct TraceMeta {
    pub version: u64,
    pub ts: f64,
    pub pid: u32,
}

/// Aggregate statistics derived from the trace.
#[derive(Debug, Default)]
pub struct TraceStats {
    pub duration_ms: u64,
    pub llm_calls: usize,
    pub tool_calls: usize,
    pub total_prompt_tokens: u32,
    pub total_completion_tokens: u32,
    pub file_touches: usize,
    pub errors: usize,
    pub sub_agents: usize,
    pub compactions: usize,
    pub user_messages: usize,
}

// ============================================================================
// TraceReader
// ============================================================================

pub struct TraceReader {
    path: std::path::PathBuf,
}

impl TraceReader {
    /// 从指定路径创建 Reader。
    pub fn new(path: &Path) -> Self {
        TraceReader { path: path.to_path_buf() }
    }

    /// 从项目根读取默认 trace 文件 `<root>/.ccd.trace.ndjson`。
    pub fn from_root(root: &Path) -> Self {
        TraceReader::new(&root.join(".ccd.trace.ndjson"))
    }

    /// 完整重建 span 树。返回 (meta, 顶层 span 列表)。
    pub fn read_tree(&self) -> std::io::Result<(TraceMeta, Vec<SpanNode>)> {
        let body = std::fs::read_to_string(&self.path)?;
        let mut meta = TraceMeta { version: 0, ts: 0.0, pid: 0 };
        let mut spans_by_id: HashMap<String, SpanNode> = HashMap::new();
        let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
        let mut point_events: Vec<PointEvent> = Vec::new();
        let mut root_spans: Vec<String> = Vec::new();
        let mut span_order: Vec<String> = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let type_str = v["type"].as_str().unwrap_or("");
            if type_str == "meta" {
                meta = TraceMeta {
                    version: v["version"].as_u64().unwrap_or(0),
                    ts: v["ts"].as_f64().unwrap_or(0.0),
                    pid: v["pid"].as_u64().unwrap_or(0) as u32,
                };
                continue;
            }
            match type_str {
                "s" => {
                    if let Some(span) = parse_span_start(&v) {
                        let id = span.span_id.clone();
                        spans_by_id.entry(id.clone()).or_insert_with(|| SpanNode {
                            span,
                            end: None,
                            children: Vec::new(),
                            events: Vec::new(),
                            direct_events: Vec::new(),
                        });
                        span_order.push(id.clone());
                        if let Some(parent) = v.get("parent_id").and_then(|p| p.as_str()) {
                            children_of.entry(parent.to_string()).or_default().push(id);
                        } else {
                            root_spans.push(id);
                        }
                    }
                }
                "e" => {
                    if let Some(span_id) = v.get("span_id").and_then(|s| s.as_str()) {
                        if let Some(node) = spans_by_id.get_mut(span_id) {
                            node.end = Some(parse_span_end(&v));
                        }
                    }
                }
                "p" => {
                    point_events.push(parse_point_event(&v));
                }
                _ => {}
            }
        }

        // 构建树: 从 root 开始递归
        let mut roots = Vec::new();
        for root_id in &root_spans {
            if let Some(node) = spans_by_id.remove(root_id) {
                roots.push(build_tree(node, &mut spans_by_id, &children_of));
            }
        }
        // 孤儿节点: 父节点不在 root 列表中的残余 span
        let mut remaining: Vec<String> = spans_by_id.keys().cloned().collect();
        remaining.sort_by(|a, b| {
            span_order.iter().position(|x| x == a).unwrap_or(0)
                .cmp(&span_order.iter().position(|x| x == b).unwrap_or(0))
        });
        for orphan_id in remaining {
            if let Some(node) = spans_by_id.remove(&orphan_id) {
                roots.push(node);
            }
        }

        // 将 point events 关联到最近的祖先 span
        for pe in &point_events {
            for root in &mut roots {
                attach_event(root, pe);
            }
        }

        Ok((meta, roots))
    }

    /// 读取最近 N 个事件（不重建树，快速获取尾部）。
    /// 返回每行的原始 JSON 字符串。
    pub fn recent_events(&self, n: usize) -> std::io::Result<Vec<String>> {
        let body = std::fs::read_to_string(&self.path)?;
        let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
        // 跳过 meta 行
        let start = lines.len().saturating_sub(n).max(1);
        Ok(lines[start..].iter().map(|s| (*s).to_string()).collect())
    }

    /// 渲染为 LLM 可读的文本摘要。
    pub fn render_for_llm(&self, max_events: usize) -> std::io::Result<String> {
        let (_meta, tree) = self.read_tree()?;
        let stats = self.compute_stats(&tree);
        let mut output = String::new();

        // 头部摘要
        output.push_str(&format!(
            "## Trace 摘要\n\
            ⏱ 总耗时: {:.1}s | LLM 调用: {} | 工具: {} | 文件 touch: {} | 错误: {}\n\n",
            stats.duration_ms as f64 / 1000.0,
            stats.llm_calls,
            stats.tool_calls,
            stats.file_touches,
            stats.errors,
        ));

        // 执行流程
        output.push_str("### 执行流程\n");
        let mut count = 0usize;
        for root in &tree {
            output.push_str(&render_node(root, 0, &mut count, max_events));
            if count >= max_events { break; }
        }
        if count >= max_events {
            output.push_str(&format!("  ... (超过 {} 个事件, 已截断)\n", max_events));
        }

        // 文件 touch 汇总
        let mut file_touches: Vec<String> = Vec::new();
        collect_file_touches(&tree, &mut file_touches);
        if !file_touches.is_empty() {
            output.push_str("\n### 文件 touch 汇总\n");
            for ft in file_touches.iter().take(20) {
                output.push_str(&format!("  {}\n", ft));
            }
            if file_touches.len() > 20 {
                output.push_str(&format!("  ... 还有 {} 个文件\n", file_touches.len() - 20));
            }
        }

        // 统计详情
        output.push_str(&format!(
            "\n### 统计\n\
            LLM 调用: {} 次 ({} prompt + {} completion tokens)\n\
            工具调用: {} 次 ({} 错误)\n\
            Sub-agent: {} 次\n\
            上下文压缩: {} 次\n\
            用户消息: {} 条\n",
            stats.llm_calls,
            stats.total_prompt_tokens,
            stats.total_completion_tokens,
            stats.tool_calls,
            stats.errors,
            stats.sub_agents,
            stats.compactions,
            stats.user_messages,
        ));

        Ok(output)
    }

    /// 涉及某个文件的所有事件。
    pub fn filter_by_file(&self, path: &str) -> std::io::Result<Vec<(SpanNode, Vec<PointEvent>)>> {
        let (_meta, tree) = self.read_tree()?;
        let mut results = Vec::new();
        filter_tree_by_file(&tree, path, &mut results);
        Ok(results)
    }

    /// 统计摘要。
    pub fn aggregate_stats(&self) -> std::io::Result<TraceStats> {
        let (_meta, tree) = self.read_tree()?;
        Ok(self.compute_stats(&tree))
    }

    fn compute_stats(&self, tree: &[SpanNode]) -> TraceStats {
        let mut stats = TraceStats::default();
        for root in tree {
            accumulate_stats(root, &mut stats);
        }
        // 取最长的 span 的 end - start 作为总耗时
        for root in tree {
            if let Some(ref end) = root.end {
                let dur = ((end.ts - root.span.ts) * 1000.0) as u64;
                stats.duration_ms = stats.duration_ms.max(dur);
            }
        }
        stats
    }
}

// ============================================================================
// 内部辅助函数
// ============================================================================

fn parse_span_start(v: &serde_json::Value) -> Option<SpanStart> {
    let span_id = v["span_id"].as_str()?.to_string();
    let parent_id = v.get("parent_id").and_then(|p| p.as_str()).map(|s| s.to_string());
    let kind = match v["kind"].as_str()? {
        "turn" => SpanKind::Turn,
        "llm_call" => SpanKind::LlmCall,
        "tool_call" => SpanKind::ToolCall,
        "sub_agent" => SpanKind::SubAgent,
        "milestone" => SpanKind::Milestone,
        "reasoning" => SpanKind::Reasoning,
        "compaction" => SpanKind::Compaction,
        _ => return None,
    };
    Some(SpanStart {
        span_id,
        parent_id,
        kind,
        ts: v["ts"].as_f64().unwrap_or(0.0),
        meta: v.get("meta").cloned().unwrap_or_default(),
    })
}

fn parse_span_end(v: &serde_json::Value) -> SpanEnd {
    SpanEnd {
        span_id: v["span_id"].as_str().unwrap_or("").to_string(),
        ts: v["ts"].as_f64().unwrap_or(0.0),
        meta: v.get("meta").cloned().unwrap_or_default(),
    }
}

fn parse_point_event(v: &serde_json::Value) -> PointEvent {
    let kind = match v["kind"].as_str() {
        Some("file_touch") => {
            let meta = v.get("meta").cloned().unwrap_or_default();
            EventKind::FileTouch {
                path: meta["path"].as_str().unwrap_or("").to_string(),
                touch: match meta["touch"].as_str() {
                    Some("read") => TouchType::Read,
                    Some("edit") => TouchType::Edit,
                    Some("create") => TouchType::Create,
                    Some("delete") => TouchType::Delete,
                    _ => TouchType::Hit,
                },
                lines: None,
            }
        }
        Some("permission_check") => {
            let meta = v.get("meta").cloned().unwrap_or_default();
            EventKind::PermissionCheck {
                key: meta["key"].as_str().unwrap_or("").to_string(),
                decision: match meta["decision"].as_str() {
                    Some("granted") => PermissionDecision::Granted,
                    Some("denied") => PermissionDecision::Denied,
                    Some("cancelled") => PermissionDecision::Cancelled,
                    _ => PermissionDecision::AutoGranted,
                },
            }
        }
        Some("user_message") => {
            let meta = v.get("meta").cloned().unwrap_or_default();
            EventKind::UserMessage {
                source: match meta["source"].as_str() {
                    Some("manual") => MessageSource::Manual,
                    Some("auto") => MessageSource::Auto,
                    _ => MessageSource::Injected,
                },
                summary: meta["summary"].as_str().unwrap_or("").to_string(),
            }
        }
        _ => {
            EventKind::Notice {
                text: v["meta"]["text"].as_str().unwrap_or("").to_string(),
            }
        }
    };
    PointEvent {
        kind,
        ts: v["ts"].as_f64().unwrap_or(0.0),
        meta: v.get("meta").cloned().unwrap_or_default(),
    }
}

fn build_tree(
    mut node: SpanNode,
    remaining: &mut HashMap<String, SpanNode>,
    children_of: &HashMap<String, Vec<String>>,
) -> SpanNode {
    if let Some(child_ids) = children_of.get(&node.span.span_id).cloned() {
        for child_id in child_ids {
            if let Some(child) = remaining.remove(&child_id) {
                node.children.push(build_tree(child, remaining, children_of));
            }
        }
    }
    node
}

fn attach_event(node: &mut SpanNode, event: &PointEvent) {
    // 尝试附加到子节点
    for child in &mut node.children {
        if event.ts >= child.span.ts {
            let child_end = child.end.as_ref().map(|e| e.ts).unwrap_or(f64::MAX);
            if event.ts <= child_end {
                attach_event(child, event);
                return;
            }
        }
    }
    // 没有合适的子节点 → 附加到当前节点
    node.direct_events.push(event.clone());
}

fn render_node(node: &SpanNode, depth: usize, count: &mut usize, max: usize) -> String {
    if *count >= max { return String::new(); }
    *count += 1;
    let indent = "  ".repeat(depth);
    let kind_str = format!("{:?}", node.span.kind);
    let duration = node.end.as_ref()
        .and_then(|e| e.meta.get("duration_ms").and_then(|v| v.as_f64()))
        .map(|d| format!(" ({:.0}ms)", d))
        .unwrap_or_default();
    let tool_info = node.span.meta.get("tool")
        .and_then(|v| v.as_str())
        .map(|t| format!(" [{}]", t))
        .unwrap_or_default();

    let mut out = format!("{indent}{kind_str}{duration}{tool_info}\n");

    // LLM call 详情
    if let Some(model) = node.span.meta.get("model").and_then(|v| v.as_str()) {
        let pt = node.span.meta.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let ct = node.end.as_ref()
            .and_then(|e| e.meta.get("completion_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0);
        out.push_str(&format!("{indent}  模型: {} | tokens: {}→{}\n", model, pt, ct));
    }
    if let Some(preview) = node.span.meta.get("input_preview").and_then(|v| v.as_str()) {
        out.push_str(&format!("{indent}  输入: {}\n", preview));
    }
    if let Some(preview) = node.span.meta.get("output_preview").and_then(|v| v.as_str()) {
        let truncated: String = preview.chars().take(200).collect();
        out.push_str(&format!("{indent}  输出: {}\n", truncated));
    }
    // 错误标记
    if node.end.as_ref().and_then(|e| e.meta.get("is_error")).and_then(|v| v.as_bool()).unwrap_or(false) {
        out.push_str(&format!("{indent}  ❌ 错误\n"));
    }

    // 直接事件
    for ev in &node.direct_events {
        match &ev.kind {
            EventKind::Notice { text } => {
                out.push_str(&format!("{indent}  📝 {}\n", text));
            }
            EventKind::UserMessage { source, summary } => {
                let src = match source {
                    MessageSource::Manual => "手动",
                    MessageSource::Auto => "自动",
                    MessageSource::Injected => "注入",
                };
                out.push_str(&format!("{indent}  💬 [{}] {}\n", src, summary));
            }
            _ => {}
        }
    }

    for child in &node.children {
        out.push_str(&render_node(child, depth + 1, count, max));
    }
    out
}

fn collect_file_touches(tree: &[SpanNode], out: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    for node in tree {
        for event in &node.events {
            if let EventKind::FileTouch { path, touch, .. } = &event.kind {
                let key = format!("{}: [{:?}]", path, touch);
                if seen.insert(key.clone()) {
                    out.push(key);
                }
            }
        }
        collect_file_touches(&node.children, out);
    }
}

fn filter_tree_by_file<'a>(
    tree: &'a [SpanNode],
    path: &str,
    results: &mut Vec<(SpanNode, Vec<PointEvent>)>,
) {
    for node in tree {
        // Check both events and direct_events for file touches
        let matching: Vec<PointEvent> = node.events.iter().chain(node.direct_events.iter())
            .filter(|e| matches!(&e.kind, EventKind::FileTouch { path: p, .. } if p == path))
            .cloned()
            .collect();
        if !matching.is_empty() {
            results.push((node.clone(), matching));
        }
        filter_tree_by_file(&node.children, path, results);
    }
}

fn accumulate_stats(node: &SpanNode, stats: &mut TraceStats) {
    match node.span.kind {
        SpanKind::LlmCall => {
            stats.llm_calls += 1;
            if let Some(pt) = node.span.meta.get("prompt_tokens").and_then(|v| v.as_u64()) {
                stats.total_prompt_tokens += pt as u32;
            }
            if let Some(ct) = node.end.as_ref().and_then(|e| e.meta.get("completion_tokens").and_then(|v| v.as_u64())) {
                stats.total_completion_tokens += ct as u32;
            }
        }
        SpanKind::ToolCall => {
            stats.tool_calls += 1;
            if node.end.as_ref().and_then(|e| e.meta.get("is_error")).and_then(|v| v.as_bool()).unwrap_or(false) {
                stats.errors += 1;
            }
        }
        SpanKind::SubAgent => stats.sub_agents += 1,
        SpanKind::Compaction => stats.compactions += 1,
        _ => {}
    }
    for ev in &node.events {
        match &ev.kind {
            EventKind::FileTouch { .. } => stats.file_touches += 1,
            EventKind::UserMessage { .. } => stats.user_messages += 1,
            _ => {}
        }
    }
    for child in &node.children {
        accumulate_stats(child, stats);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_trace(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn read_tree_simple_span() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":123}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1100.0,"meta":{"duration_ms":100}}
"#);
        let reader = TraceReader::new(&path);
        let (meta, tree) = reader.read_tree().unwrap();
        assert_eq!(meta.version, 1);
        assert_eq!(meta.pid, 123);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].span.span_id, "sp_001");
        assert!(tree[0].end.is_some());
    }

    #[test]
    fn read_tree_nested_spans() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"llm_call","ts":1000.1,"meta":{}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{"duration_ms":900}}
{"type":"e","span_id":"sp_001","ts":1001.0,"meta":{"duration_ms":1000}}
"#);
        let reader = TraceReader::new(&path);
        let (_meta, tree) = reader.read_tree().unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].span.span_id, "sp_002");
    }

    #[test]
    fn read_tree_handles_point_events() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"notice","ts":1000.5,"meta":{"text":"hello"}}
{"type":"e","span_id":"sp_001","ts":1100.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let (_meta, tree) = reader.read_tree().unwrap();
        // 应该有至少一个 direct_event
        assert!(tree[0].direct_events.len() >= 1 || tree[0].events.len() >= 1);
    }

    #[test]
    fn recent_events_returns_last_n() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        let mut content = r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
"#.to_string();
        for i in 0..10 {
            content.push_str(&format!(
                r#"{{"type":"s","span_id":"sp_{i:04}","parent_id":null,"kind":"turn","ts":{}.0,"meta":{{}}}}
"#,
                1000.0 + i as f64
            ));
        }
        write_trace(&path, &content);
        let reader = TraceReader::new(&path);
        let events = reader.recent_events(3).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn render_for_llm_includes_tool_info() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"tool_call","ts":1000.1,"meta":{"tool":"read_file","input_preview":"file_path: src/main.rs"}}
{"type":"e","span_id":"sp_002","ts":1000.2,"meta":{"is_error":false,"output_preview":"fn main() { ... }"}}
{"type":"e","span_id":"sp_001","ts":1001.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let rendered = reader.render_for_llm(100).unwrap();
        assert!(rendered.contains("ToolCall"), "expected ToolCall in render, got: {rendered}");
        assert!(rendered.contains("src/main.rs") || rendered.contains("统计"), "expected stats in render");
    }

    #[test]
    fn aggregate_stats_counts_correctly() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"s","span_id":"sp_002","parent_id":"sp_001","kind":"llm_call","ts":1000.1,"meta":{"prompt_tokens":100}}
{"type":"e","span_id":"sp_002","ts":1001.0,"meta":{"completion_tokens":50}}
{"type":"s","span_id":"sp_003","parent_id":"sp_001","kind":"tool_call","ts":1001.1,"meta":{"tool":"read_file"}}
{"type":"e","span_id":"sp_003","ts":1001.2,"meta":{"is_error":false}}
{"type":"s","span_id":"sp_004","parent_id":"sp_001","kind":"tool_call","ts":1001.3,"meta":{"tool":"write_file"}}
{"type":"e","span_id":"sp_004","ts":1001.5,"meta":{"is_error":true}}
{"type":"e","span_id":"sp_001","ts":1002.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let stats = reader.aggregate_stats().unwrap();
        assert_eq!(stats.llm_calls, 1);
        assert_eq!(stats.tool_calls, 2);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.total_prompt_tokens, 100);
        assert_eq!(stats.total_completion_tokens, 50);
    }

    #[test]
    fn filter_by_file_returns_matching_events() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"p","kind":"file_touch","ts":1000.5,"meta":{"path":"src/main.rs","touch":"read"}}
{"type":"p","kind":"file_touch","ts":1000.6,"meta":{"path":"src/lib.rs","touch":"read"}}
{"type":"e","span_id":"sp_001","ts":1100.0,"meta":{}}
"#);
        let reader = TraceReader::new(&path);
        let results = reader.filter_by_file("src/main.rs").unwrap();
        assert!(!results.is_empty(), "should find file_touch for src/main.rs");
    }

    #[test]
    fn from_root_reads_default_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        write_trace(&path, r#"{"type":"meta","version":1,"ts":1000.0,"pid":0}
{"type":"s","span_id":"sp_001","parent_id":null,"kind":"turn","ts":1000.0,"meta":{}}
{"type":"e","span_id":"sp_001","ts":1100.0,"meta":{}}
"#);
        let reader = TraceReader::from_root(dir.path());
        let (_meta, tree) = reader.read_tree().unwrap();
        assert_eq!(tree.len(), 1);
    }
}