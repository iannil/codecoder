// LLM tool to decompose a high-level goal into workgraph milestones.
// Uses the main provider to generate the decomposition, then seeds each
// milestone into workgraph.json. Permission::None — local scratch, same
// as plan/milestone. Sub-agents are excluded (same as milestone/plan/memory).

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
         to generate structured milestones with acceptance criteria, \
         then seeds them into the workgraph. \
         Returns the list of created milestone IDs."
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
             a sequence of milestones with acceptance criteria.\n\n\
             Goal: {goal}\n"
        );
        if !context.is_empty() {
            prompt.push_str(&format!("Context:\n{context}\n\n"));
        }
        prompt.push_str(
            "Output milestones in the following format, one per milestone. \
             Each milestone must have a title (short, imperative) and acceptance criteria \
             (specific, testable conditions). Decompose into 3-8 milestones. \
             Order them such that earlier milestones are prerequisites for later ones. \
             Do NOT add numbering or bullet markers — use the exact format:\n\n\
             MILESTONE: <title>\n\
             ACCEPTANCE: <criteria>\n\n\
             Separate milestones with a blank line."
        );

        // Build a simple provider request (no tools, just a text completion).
        let req = CompletionRequest {
            model: crate::config::Config::from_env().model,
            messages: vec![
                Message::text(0, Role::User, prompt),
            ],
            max_tokens: 4096,
            temperature: 0.3,
            tools: vec![],
        };

        let provider = crate::select_provider(&crate::config::Config::from_env());
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
        let milestones = parse_milestones(&response_text);
        if milestones.is_empty() {
            return Ok(ToolOutput::err(
                "LLM did not produce any parseable milestones. Response:\n{response_text}",
            ));
        }

        // Write each milestone to the workgraph.
        let root = ctx.root.to_path_buf();
        let mut created_ids = Vec::new();
        for (title, acceptance) in &milestones {
            match WorkGraph::with_lock(&root, |g| g.add(title, acceptance, vec![])) {
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

/// Parse the LLM response into a list of (title, acceptance_criteria) pairs.
/// Expects the format:
///   MILESTONE: <title>
///   ACCEPTANCE: <criteria>
fn parse_milestones(response: &str) -> Vec<(String, String)> {
    let mut milestones = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_acceptance: Option<String> = None;

    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // Blank line: flush the current milestone.
            if let (Some(title), Some(acceptance)) = (current_title.take(), current_acceptance.take()) {
                milestones.push((title, acceptance));
            }
            current_title = None;
            current_acceptance = None;
            continue;
        }

        if let Some(title) = trimmed.strip_prefix("MILESTONE:") {
            // Flush any previous un-flushed milestone.
            if let (Some(t), Some(a)) = (current_title.take(), current_acceptance.take()) {
                milestones.push((t, a));
            }
            current_title = Some(title.trim().to_string());
            current_acceptance = None;
        } else if let Some(acc) = trimmed.strip_prefix("ACCEPTANCE:") {
            current_acceptance = Some(acc.trim().to_string());
        } else if trimmed.starts_with("MILESTONE") && trimmed.contains(':') {
            // Handle variations like "MILESTONE 1:" — strip the number.
            let after_colon = trimmed.splitn(2, ':').nth(1).unwrap_or("").trim();
            if let (Some(t), Some(a)) = (current_title.take(), current_acceptance.take()) {
                milestones.push((t, a));
            }
            current_title = Some(after_colon.to_string());
            current_acceptance = None;
        }
        // Lines that don't match either pattern are ignored (e.g. blank lines
        // between blocks, or commentary).
    }

    // Flush the last milestone if accumulated.
    if let (Some(title), Some(acceptance)) = (current_title.take(), current_acceptance.take()) {
        milestones.push((title, acceptance));
    }

    milestones
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workgraph::WorkGraph;

    #[test]
    fn parse_milestones_basic() {
        let input = "\
MILESTONE: Set up data model
ACCEPTANCE: Schema is defined, migrations run, basic CRUD works

MILESTONE: Implement API endpoints
ACCEPTANCE: All endpoints return correct status codes, integration tests pass";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].0, "Set up data model");
        assert!(ms[0].1.contains("Schema is defined"));
        assert_eq!(ms[1].0, "Implement API endpoints");
        assert!(ms[1].1.contains("integration tests pass"));
    }

    #[test]
    fn parse_milestones_with_numbered_variant() {
        let input = "\
MILESTONE 1: Scaffold project
ACCEPTANCE: Project compiles, basic structure in place

MILESTONE 2: Core logic
ACCEPTANCE: Core algorithm handles edge cases, tests pass";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].0, "Scaffold project");
        assert_eq!(ms[1].0, "Core logic");
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
ACCEPTANCE: User can log in with email and password

Some notes here.

MILESTONE: Dashboard
ACCEPTANCE: Dashboard shows user data";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].0, "Login feature");
        assert_eq!(ms[1].0, "Dashboard");
    }

    #[test]
    fn parse_milestones_acceptance_after_last_milestone_without_blank_line_flushes() {
        // The last milestone should be flushed even without a trailing blank line.
        let input = "\
MILESTONE: Only one
ACCEPTANCE: Works";
        let ms = parse_milestones(input);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].0, "Only one");
        assert_eq!(ms[0].1, "Works");
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