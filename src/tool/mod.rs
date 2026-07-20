// Tool trait + registry (ADR 0018). Tools are built-in primitives compiled into
// the binary; a runtime-authored executable is a Capability, not a Tool.
pub mod builtin;
pub mod dev;
pub mod net;
pub mod reason;
pub mod search;
pub mod wasm;

use crate::permission::Permission;
use serde_json::{Value, json};
use std::path::Path;

pub struct ToolCtx<'a> {
    pub root: &'a Path,
    /// Cooperative-cancellation flag (ADR 0016). Present on real turns so a
    /// long-running tool (e.g. `run_command`) can poll it and kill its child;
    /// `None` in unit tests that never cancel.
    pub cancel: Option<&'a crate::agent::CancelToken>,
}

impl<'a> ToolCtx<'a> {
    /// A context with no cancellation signal (unit tests, one-shot helpers).
    pub fn new(root: &'a Path) -> Self {
        ToolCtx { root, cancel: None }
    }

    /// A context wired to the turn's cancel token (the live agent loop).
    pub fn with_cancel(root: &'a Path, cancel: &'a crate::agent::CancelToken) -> Self {
        ToolCtx { root, cancel: Some(cancel) }
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
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
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

/// The fixed set of built-in Tools (ADR 0018). Dispatch by name; also produces the
/// OpenAI-facing `tools` schema list for the request (ADR 0017).
pub struct Toolbox {
    tools: Vec<Box<dyn Tool>>,
}

impl Toolbox {
    pub fn builtin() -> Self {
        Self {
            tools: vec![
                Box::new(builtin::ReadFile),
                Box::new(builtin::ListDirectory),
                Box::new(builtin::WriteFile),
                Box::new(builtin::EditFile),
                Box::new(builtin::RunCommand),
                Box::new(builtin::UseSkill),
                Box::new(builtin::RunCapability),
                Box::new(builtin::GenerateSkill),
                Box::new(builtin::GeneratePrompt),
                Box::new(builtin::PromotePrompt),
                Box::new(builtin::GenerateCapability),
                Box::new(builtin::Agent),
                Box::new(builtin::AskUser),
                Box::new(builtin::Review),
                Box::new(builtin::Confirm),
                Box::new(builtin::SelfHeal),
                Box::new(net::SearchWeb),
                Box::new(net::SearchGithub),
                Box::new(net::ReverseApi),
                Box::new(dev::Commit),
                Box::new(dev::Diff),
                Box::new(dev::Plan),
                Box::new(dev::Milestone),
                Box::new(dev::Memory),
                Box::new(reason::Reason),
                Box::new(search::Glob),
                Box::new(search::Grep),
            ],
        }
    }

    /// The tool set a depth-1 sub-agent may use (ADR 0019): a curated subset of the
    /// `Permission::None` (never-prompts) tools — read/list, skills, read-only
    /// networking, and `diff` — and NOT `agent` (depth is locked to 1).
    pub fn read_only_child() -> Self {
        Self {
            tools: vec![
                Box::new(builtin::ReadFile),
                Box::new(builtin::ListDirectory),
                Box::new(builtin::UseSkill),
                Box::new(search::Glob),
                Box::new(search::Grep),
                Box::new(net::SearchWeb),
                Box::new(net::SearchGithub),
                Box::new(net::ReverseApi),
                Box::new(dev::Diff),
            ],
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    /// The `tools` array for a chat-completions request.
    pub fn wire_schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.schema(),
                    }
                })
            })
            .collect()
    }
}
