// LLM tool to decompose a high-level goal into workgraph milestones.
// Uses the main provider to generate the decomposition, then seeds each
// milestone into workgraph.json. Permission::None — local scratch, same
// as plan/milestone. Sub-agents are excluded (same as milestone/plan/memory).
//
// Task 4: only generates milestone titles, no acceptance criteria.

use super::{Tool, ToolCtx, ToolOutput};
use crate::message::{Message, MessageItem, Role};
use crate::permission::Permission;
use crate::provider::CompletionRequest;
use crate::workgraph::WorkGraph;
use serde_json::{Value, json};
use std::path::Path;

pub struct GenerateMilestones;

impl Tool for GenerateMilestones {
    fn name(&self) -> &str {
        "generate_milestones"
    }

    fn description(&self) -> &str {
        "Decompose a high-level goal into workgraph milestones. \
         Takes a goal description and optional context, calls the LLM \
         to generate structured milestone titles, then seeds them into \
         the workgraph. Returns the list of created milestone IDs."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The high-level goal to decompose into milestones"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context (existing code structure, constraints, etc.)",
                    "default": ""
                }
            },
            "required": ["goal"]
        })
    }

    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }

    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let goal = args.get("goal").and_then(Value::as_str).unwrap_or_default();
        if goal.is_empty() {
            return Ok(ToolOutput::err("missing required arg: goal"));
        }
        let context = args.get("context").and_then(Value::as_str).unwrap_or_default();

        // Build the provider prompt.
        let mut prompt = format!(
            "You are a project planner. Decompose the following high-level goal into \
             a sequence of milestone titles.\n\n\
             Goal: {goal}\n"
        );
        if !context.is_empty() {
            prompt.push_str(&format!("Context:\n{context}\n\n"));
        }
        prompt.push_str(
            "Output milestones in the following format, one per milestone. \
             Each milestone must have a title (short, imperative). Decompose into 3-8 milestones. \
             Order them such that earlier milestones are prerequisites for later ones. \
             Do NOT add numbering or bullet markers — use the exact format:\n\n\
             MILESTONE: <title>\n\n\
             Separate milestones with a blank line."
        );

        // Build a simple provider request (no tools, just a text completion).
        let req = CompletionRequest {
            model: crate::config::Config::load().model,
            messages: vec![
                Message::text(0, Role::User, prompt),
            ],
            max_tokens: 4096,
            temperature: 0.3,
            tools: vec![],
        };

        let provider = crate::select_provider(&crate::config::Config::load());
        let completion = provider.complete(&req)?;

        let response_text = completion
            .message
            .items
            .iter()
            .filter_map(|item| {
                if let MessageItem::Text { text } = item {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Parse the LLM response into milestones.
        let titles = parse_milestones(&response_text);
        if titles.is_empty() {
            return Ok(ToolOutput::err(
                format!("LLM did not produce any parseable milestones. Response:\n{response_text}"),
            ));
        }

        // Write each milestone to the workgraph.
        let root = ctx.root.to_path_buf();
        let mut created_ids = Vec::new();
        for title in &titles {
            match WorkGraph::with_lock(&root, |g| g.add(title, vec![])) {
                Ok(id) => created_ids.push(id),
                Err(e) => {
                    // If one milestone fails, report partial success.
                    return Ok(ToolOutput::err(format!(
                        "added {} milestones before error: {e}",
                        created_ids.len()
                    )));
                }
            }
        }

        let ids_str = created_ids
            .iter()
            .map(|id| format!("#{id}"))
            .collect::<Vec<_>>()
            .join(", ");

        // Reload and render the graph for the output.
        let wg = WorkGraph::read(&root);
        Ok(ToolOutput::ok(format!(
            "Created {} milestone(s): {}\n\n{}",
            created_ids.len(),
            ids_str,
            wg.render()
        )))
    }
}

/// Parse the LLM response into a list of milestone titles.
/// Expects the format:
///   MILESTONE: <title>
fn parse_milestones(response: &str) -> Vec<String> {
    let mut titles = Vec::new();
    let mut current_title: Option<String> = None;

    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // Blank line: flush the current milestone.
            if let Some(title) = current_title.take() {
                titles.push(title);
            }
            current_title = None;
            continue;
        }

        if let Some(title) = trimmed.strip_prefix("MILESTONE:") {
            // Flush any previous un-flushed milestone.
            if let Some(t) = current_title.take() {
                titles.push(t);
            }
            current_title = Some(title.trim().to_string());
        } else if trimmed.starts_with("MILESTONE") && trimmed.contains(':') {
            // Handle variations like "MILESTONE 1:" — strip the number.
            let after_colon = trimmed.splitn(2, ':').nth(1).unwrap_or("").trim();
            if let Some(t) = current_title.take() {
                titles.push(t);
            }
            current_title = Some(after_colon.to_string());
        }
        // Lines that don't match either pattern are ignored.
    }

    // Flush the last milestone if accumulated.
    if let Some(title) = current_title.take() {
        titles.push(title);
    }

    titles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workgraph::WorkGraph;

    #[test]
    fn parse_milestones_basic() {
        let input = "\
MILESTONE: Set up data model

MILESTONE: Implement API endpoints";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0], "Set up data model");
        assert_eq!(ms[1], "Implement API endpoints");
    }

    #[test]
    fn parse_milestones_with_numbered_variant() {
        let input = "\
MILESTONE 1: Scaffold project

MILESTONE 2: Core logic";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0], "Scaffold project");
        assert_eq!(ms[1], "Core logic");
    }

    #[test]
    fn parse_milestones_empty_returns_empty() {
        assert!(parse_milestones("").is_empty());
        assert!(parse_milestones("Some random text without milestones").is_empty());
    }

    #[test]
    fn parse_milestones_handles_extra_lines() {
        let input = "\
Here is my plan:

MILESTONE: Login feature

Some notes here.

MILESTONE: Dashboard";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0], "Login feature");
        assert_eq!(ms[1], "Dashboard");
    }

    #[test]
    fn parse_milestones_last_milestone_without_blank_line_flushes() {
        let input = "\
MILESTONE: Only one";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0], "Only one");
    }

    #[test]
    fn generate_milestones_tool_permission_is_none() {
        let tool = GenerateMilestones;
        assert!(matches!(
            tool.permission(&json!({}), Path::new(".")),
            Permission::None
        ));
    }

    #[test]
    fn generate_milestones_tool_schema() {
        let tool = GenerateMilestones;
        let schema = tool.schema();
        let props = schema.get("properties").unwrap();
        assert!(props.get("goal").is_some());
        assert!(props.get("context").is_some());
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.contains(&json!("goal")));
    }

    #[test]
    fn generate_milestones_missing_goal_errors() {
        let dir = std::env::temp_dir().join(format!("cc_genms_empty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let tool = GenerateMilestones;
        let out = tool.run(json!({}), &mut ctx).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("missing required arg: goal"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_milestones_with_stub_provider() {
        // Integration test: the stub provider returns a fixed response, so the
        // tool will produce an error ("no parseable milestones"). That's expected
        // — the test verifies the tool wires up provider + workgraph correctly
        // without crashing.
        let dir = std::env::temp_dir().join(format!("cc_genms_stub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Set env to use stub (no API key).
        let mut ctx = ToolCtx::new(&dir);
        let tool = GenerateMilestones;
        let out = tool.run(
            json!({"goal": "Build a blog engine", "context": "Rust, Actix-web"}),
            &mut ctx,
        ).unwrap();
        // Stub returns a canned response, so parse will fail. But the tool
        // should not panic — it returns an error gracefully.
        assert!(out.is_error, "stub provider should produce parse error, got: {}", out.content);
        // The workgraph should still be empty (no milestones written).
        let wg = WorkGraph::read(&dir);
        assert!(wg.nodes.is_empty(), "no milestones should be written when parse fails");
        std::fs::remove_dir_all(&dir).ok();
    }
}