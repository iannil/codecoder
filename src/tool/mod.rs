// Tool trait + registry (ADR 0018). Tools are built-in primitives compiled into
// the binary; a runtime-authored executable is a Capability, not a Tool.
pub mod builtin;
pub mod cron;
pub mod dev;
pub mod generate_milestones;
pub mod mcp;
pub mod lsp;
pub mod net;
pub mod reason;
pub mod search;
pub mod send_message;
pub mod task_manage;
pub mod wasm;
pub mod tool_search;
pub mod worktree;

use crate::permission::Permission;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

pub struct ToolCtx<'a> {
    pub root: &'a Path,
    /// Cooperative-cancellation flag (ADR 0016). Present on real turns so a
    /// long-running tool (e.g. `run_command`) can poll it and kill its child;
    /// `None` in unit tests that never cancel.
    pub cancel: Option<&'a crate::agent::CancelToken>,
    /// 命令超时(0 = 无超时)。从 config 传入,run_command 工具可被参数覆盖。
    pub command_timeout: std::time::Duration,
}

impl<'a> ToolCtx<'a> {
    /// A context with no cancellation signal (unit tests, one-shot helpers).
    pub fn new(root: &'a Path) -> Self {
        ToolCtx { root, cancel: None, command_timeout: std::time::Duration::from_secs(0) }
    }

    /// A context wired to the turn's cancel token (the live agent loop).
    pub fn with_cancel(root: &'a Path, cancel: &'a crate::agent::CancelToken) -> Self {
        let cfg = crate::config::Config::from_env();
        ToolCtx {
            root,
            cancel: Some(cancel),
            command_timeout: std::time::Duration::from_secs(cfg.command_timeout_secs as u64),
        }
    }

    /// True once the turn has been cancelled; long-running tools should stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.is_cancelled())
    }
}

#[derive(Debug)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Optional meta to write onto the current session leaf's `SessionEntry.meta`.
    /// Applied by `AgentLoop::dispatch_tool` after the tool runs — tools can't touch
    /// the in-memory session directly (`ToolCtx` has no session access), so this is
    /// the side-channel (Phase E: `reason link` uses it).
    pub session_meta_mark: Option<serde_json::Value>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, session_meta_mark: None }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, session_meta_mark: None }
    }
    /// Attach a session-leaf meta mark (Phase E side-channel). Builder-style.
    pub fn with_session_meta_mark(mut self, mark: serde_json::Value) -> Self {
        self.session_meta_mark = Some(mark);
        self
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON schema of the params, fed to the LLM.
    fn schema(&self) -> Value;
    /// The tool self-reports its side effect + permission key from its args.
    /// `root` is provided so a tool can consult on-disk metadata (e.g.
    /// run_capability reads a manifest to key by environment).
    fn permission(&self, args: &Value, root: &Path) -> Permission;
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput>;
}

/// The fixed set of built-in Tools (ADR 0018). Split into core_tools (always-present)
/// and extra_tools (searchable). Dispatch by name; also produces the
/// OpenAI-facing `tools` schema list for the request (ADR 0017).
pub struct Toolbox {
    core_tools: Vec<Box<dyn Tool>>,
    extra_tools: Vec<Box<dyn Tool>>,
}

impl Toolbox {
    pub fn builtin() -> Self {
        let core_tools: Vec<Box<dyn Tool>> = vec![
            Box::new(builtin::ReadFile),
            Box::new(builtin::ListDirectory),
            Box::new(builtin::WriteFile),
            Box::new(builtin::EditFile),
            Box::new(builtin::RunCommand),
            Box::new(builtin::UseSkill),
            Box::new(search::Glob),
            Box::new(search::Grep),
            Box::new(net::SearchWeb),
            Box::new(net::SearchGithub),
            Box::new(dev::Commit),
            Box::new(dev::Diff),
            Box::new(dev::Plan),
            Box::new(dev::Milestone),
            Box::new(dev::Memory),
            Box::new(builtin::AskUser),
            Box::new(builtin::Confirm),
            Box::new(builtin::Agent),
            Box::new(reason::Reason),
            Box::new(builtin::Review),
        ];
        let extra_tools: Vec<Box<dyn Tool>> = vec![
            Box::new(builtin::RunCapability),
            Box::new(builtin::GenerateSkill),
            Box::new(builtin::GeneratePrompt),
            Box::new(builtin::PromotePrompt),
            Box::new(builtin::GenerateCapability),
            Box::new(net::ReverseApi),
            Box::new(generate_milestones::GenerateMilestones),
            Box::new(mcp::McpToolCall),
            Box::new(mcp::McpListResources),
            Box::new(mcp::McpReadResource),
            Box::new(lsp::LspTool),
            Box::new(task_manage::TaskCreate),
            Box::new(task_manage::TaskGet),
            Box::new(task_manage::TaskList),
            Box::new(task_manage::TaskUpdate),
            Box::new(task_manage::TaskStop),
            Box::new(cron::CronCreate),
            Box::new(cron::CronDelete),
            Box::new(cron::CronList),
            Box::new(send_message::SendMessage),
            Box::new(worktree::EnterWorktree),
            Box::new(worktree::ExitWorktree),
            Box::new(tool_search::ToolSearch),
        ];
        Self { core_tools, extra_tools }
    }

    /// The tool set a depth-1 sub-agent may use (ADR 0019): a curated subset of the
    /// `Permission::None` (never-prompts) tools — read/list, skills, read-only
    /// networking, and `diff` — and NOT `agent` (depth is locked to 1).
    pub fn read_only_child() -> Self {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(builtin::ReadFile),
            Box::new(builtin::ListDirectory),
            Box::new(builtin::UseSkill),
            Box::new(search::Glob),
            Box::new(search::Grep),
            Box::new(net::SearchWeb),
            Box::new(net::SearchGithub),
            Box::new(net::ReverseApi),
            Box::new(dev::Diff),
            Box::new(generate_milestones::GenerateMilestones),
        ];
        Self { core_tools: tools, extra_tools: Vec::new() }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.core_tools
            .iter()
            .chain(self.extra_tools.iter())
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Helper to render a single tool into its OpenAI-facing schema entry.
    fn tool_schema(t: &dyn Tool) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": t.name(),
                "description": t.description(),
                "parameters": t.schema(),
            }
        })
    }

    /// The schemas for the always-present core tools.
    pub fn wire_schemas_core(&self) -> Vec<Value> {
        self.core_tools.iter().map(|t| Self::tool_schema(t.as_ref())).collect()
    }

    /// The schemas for the named extra tools (typically `loaded_extra_tools`).
    pub fn wire_schemas_subset(&self, names: &HashSet<String>) -> Vec<Value> {
        self.extra_tools
            .iter()
            .filter(|t| names.contains(t.name()))
            .map(|t| Self::tool_schema(t.as_ref()))
            .collect()
    }

    /// Search the extra tools by name or description (case-insensitive substring).
    pub fn search(&self, query: &str) -> Vec<&dyn Tool> {
        let q = query.to_lowercase();
        self.extra_tools
            .iter()
            .filter(|t| {
                t.name().to_lowercase().contains(&q) || t.description().to_lowercase().contains(&q)
            })
            .map(|t| t.as_ref())
            .collect()
    }

    /// The `tools` array for a chat-completions request. Full list (core + extra),
    /// for backward compatibility.
    pub fn wire_schemas(&self) -> Vec<Value> {
        let mut schemas = self.wire_schemas_core();
        schemas.extend(self.extra_tools.iter().map(|t| Self::tool_schema(t.as_ref())));
        schemas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_meta_mark_defaults_none_and_builder_sets_it() {
        assert!(ToolOutput::ok("x").session_meta_mark.is_none());
        assert!(ToolOutput::err("x").session_meta_mark.is_none());
        let m = serde_json::json!({"causal_node": 5, "status": "hypothesis"});
        let o = ToolOutput::ok("linked").with_session_meta_mark(m.clone());
        assert_eq!(o.session_meta_mark, Some(m));
        assert!(!o.is_error);
    }
}
