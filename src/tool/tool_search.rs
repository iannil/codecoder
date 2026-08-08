use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::path::Path;

pub struct ToolSearch;

impl Tool for ToolSearch {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search for available tools that are not shown by default. \
         Provide a search query and matching tools will be loaded for this session. \
         Use this when you need a tool that isn't in the current tool list."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search term to match against tool names and descriptions."
                }
            },
            "required": ["query"]
        })
    }

    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }

    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if query.is_empty() {
            return Ok(ToolOutput::err("tool_search requires `query`"));
        }
        // The actual search happens in AgentLoop::dispatch_tool which has access to
        // the Toolbox. We pass the query via session_meta_mark so dispatch_tool can
        // search and load matching tools.
        Ok(ToolOutput::ok(format!("searching for tools matching: {query}"))
            .with_session_meta_mark(json!({ "tool_search_query": query })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn tool_search_requires_query() {
        let out = ToolSearch.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("requires `query`"));
    }

    #[test]
    fn tool_search_permission_none() {
        assert!(matches!(
            ToolSearch.permission(&json!({}), Path::new(".")),
            Permission::None
        ));
    }

    #[test]
    fn tool_search_propagates_query_via_mark() {
        let out = ToolSearch
            .run(json!({ "query": "task" }), &mut ToolCtx::new(Path::new(".")))
            .unwrap();
        assert!(!out.is_error);
        let mark = out.session_meta_mark.unwrap();
        assert_eq!(
            mark.get("tool_search_query").and_then(Value::as_str),
            Some("task")
        );
    }
}