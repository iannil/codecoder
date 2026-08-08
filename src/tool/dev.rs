// Development tools: commit (Ask), diff / plan / milestone (None). Local dev scaffolding.
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git").args(args).current_dir(root).output()
}

fn combined(out: &std::process::Output) -> (String, bool) {
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.stderr.is_empty() {
        s.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    (s, !out.status.success())
}

pub struct Commit;

impl Tool for Commit {
    fn name(&self) -> &str {
        "commit"
    }
    fn description(&self) -> &str {
        "Stage changes and create a git commit. Optionally limit to specific files."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" },
                "files": { "type": "array", "items": { "type": "string" }, "description": "Files to stage; default: all changes." }
            },
            "required": ["message"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "commit".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let message = args.get("message").and_then(Value::as_str).unwrap_or_default();
        if message.is_empty() {
            return Ok(ToolOutput::err("missing required arg: message"));
        }
        // Stage: explicit files, else everything.
        let files: Vec<String> = args
            .get("files")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let add_args: Vec<&str> = if files.is_empty() {
            vec!["add", "-A"]
        } else {
            let mut v = vec!["add", "--"];
            v.extend(files.iter().map(String::as_str));
            v
        };
        let (add_out, add_err) = combined(&git(ctx.root, &add_args)?);
        if add_err {
            return Ok(ToolOutput::err(format!("git add failed: {add_out}")));
        }
        let (out, err) = combined(&git(ctx.root, &["commit", "-m", message])?);
        Ok(ToolOutput { content: out, is_error: err, session_meta_mark: None })
    }
}

pub struct Diff;

impl Tool for Diff {
    fn name(&self) -> &str {
        "diff"
    }
    fn description(&self) -> &str {
        "Show git working-tree changes (or staged with `staged`), optionally for one path."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "staged": { "type": "boolean" }
            }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        // Check if git repo exists before running git diff
        let git_check = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&ctx.root)
            .output();
        match git_check {
            Ok(out) if !out.status.success() => {
                return Ok(ToolOutput::err(
                    "diff unavailable: no git repository. Run `git init` first."
                ));
            }
            Err(e) => {
                return Ok(ToolOutput::err(
                    format!("diff unavailable: git check failed: {e}")
                ));
            }
            _ => {} // git repo exists, proceed
        }

        let mut a = vec!["diff"];
        if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
            a.push("--cached");
        }
        let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
        if !path.is_empty() {
            a.push("--");
            a.push(path);
        }
        let (out, err) = combined(&git(ctx.root, &a)?);
        if err {
            return Ok(ToolOutput::err(out));
        }
        Ok(ToolOutput::ok(if out.is_empty() { "(no changes)".into() } else { out }))
    }
}

pub struct Plan;

impl Tool for Plan {
    fn name(&self) -> &str {
        "plan"
    }
    fn description(&self) -> &str {
        "Propose a plan (steps) for the user to approve or reject before you proceed."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "steps": { "type": "array", "items": { "type": "string" } },
                "plan": { "type": "string", "description": "Freeform plan text (alternative to steps)." }
            }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    // Intercepted by the AgentLoop (needs the event channel for the approval dialog).
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::err("plan must be handled by the AgentLoop"))
    }
}

/// The `milestone` tool manages the durable **Work Graph** (first-class citizen
/// #2; src/workgraph.rs) — dependency-ordered work that survives context resets.
/// Each node is a simple title + deps; the agent self-reports completion.
/// Supersedes the old flat `todo` (legacy `todos.json` is migrated forward on
/// first read).
pub struct Milestone;

impl Tool for Milestone {
    fn name(&self) -> &str {
        "milestone"
    }
    fn description(&self) -> &str {
        "Manage the durable Work Graph of milestones (dependency-ordered work that \
         survives context resets): action = list | add | start | done | next | remove | plan. \
         `add` takes title (+ optional deps); `done` marks a milestone complete. \
         `next` returns the next ready milestone to work on. \
         `plan` shows the plan for a milestone (if one exists)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "add", "start", "done", "next", "remove", "plan"] },
                "id": { "type": "integer" },
                "title": { "type": "string" },
                "deps": { "type": "array", "items": { "type": "integer" }, "description": "Milestone ids that must be done first." }
            },
            "required": ["action"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        use crate::workgraph::WorkGraph;
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("list")
            .to_string();
        let id = args.get("id").and_then(Value::as_u64);

        // plan action reads milestone plan files from disk, not the workgraph mutex.
        if action == "plan" {
            let Some(i) = id else {
                return Ok(ToolOutput::err("plan needs `id`"));
            };
            let g = WorkGraph::read(ctx.root);
            let Some(n) = g.get(i) else {
                return Ok(ToolOutput::err(format!("unknown milestone #{i}")));
            };
            if crate::milestone_plan::plan_exists(ctx.root, i) {
                match crate::milestone_plan::read_plan(ctx.root, i) {
                    Ok(plan) => Ok(ToolOutput::ok(format!(
                        "Milestone #{} Plan:\nskill: {}\nacceptance criteria:\n{}\n\
                         test requirements: {}\nrisks: {}",
                        plan.milestone_id, plan.skill_used,
                        plan.acceptance_criteria.iter().map(|c| format!("  - {c}")).collect::<Vec<_>>().join("\n"),
                        plan.test_requirements,
                        if plan.risks.is_empty() { "none".into() } else { plan.risks.join("; ") },
                    ))),
                    Err(e) => Ok(ToolOutput::err(format!("plan corrupt: {e}"))),
                }
            } else {
                Ok(ToolOutput::ok(format!(
                    "Milestone #{} \"{}\" has no plan yet. Use `use_skill` to load an engineer skill and generate one.",
                    i, n.title,
                )))
            }
        } else {
            // 读+改+存统一在咨询锁内(ADR 0035),防与并发写者 lost-update。
            // 读动作(list/next)走同一锁取一致快照;with_lock 末尾 save 对未改图幂等。
            WorkGraph::with_lock(ctx.root, |g| Ok(Self::apply(g, &action, id, &args)))
        }
    }
}

impl Milestone {
    /// 纯内存态:按 action 改 `g` 并返回输出(无 IO,由 with_lock 负责存盘)。
    fn apply(g: &mut crate::workgraph::WorkGraph, action: &str, id: Option<u64>, _args: &Value) -> ToolOutput {
        use crate::workgraph::NodeStatus;
        match action {
            "list" => ToolOutput::ok(g.render()),
            "next" => ToolOutput::ok(match g.next_ready() {
                Some(n) => format!("▶ #{} {}", n.id, n.title),
                None => "(nothing ready — all milestones done, blocked, or in progress)".into(),
            }),
            "add" => {
                let title = _args.get("title").and_then(Value::as_str).unwrap_or_default();
                let deps: Vec<u64> = _args
                    .get("deps")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default();
                match g.add(title, deps) {
                    Ok(new) => {
                        ToolOutput::ok(format!("added #{new}\n{}", g.render()))
                    }
                    Err(e) => ToolOutput::err(e.to_string()),
                }
            }
            "start" => {
                if id.map(|i| g.set_status(i, NodeStatus::InProgress)).unwrap_or(false) {
                    ToolOutput::ok(g.render())
                } else {
                    ToolOutput::err("start needs a valid `id`")
                }
            }
            "done" => {
                let Some(i) = id else {
                    return ToolOutput::err("done needs `id`");
                };
                if !g.set_status(i, NodeStatus::Done) {
                    return ToolOutput::err("done needs a valid `id`");
                }
                ToolOutput::ok(g.render())
            }
            "remove" => {
                let Some(i) = id else {
                    return ToolOutput::err("remove needs `id`");
                };
                if let Err(e) = g.remove(i) {
                    return ToolOutput::err(e.to_string());
                }
                ToolOutput::ok(g.render())
            }
            other => ToolOutput::err(format!("unknown action: {other}")),
        }
    }
}

pub struct Memory;

impl Tool for Memory {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        "Persistent key-value memory across sessions: action = get | set | list | delete. \
         Also the index of locally-stored data (key `data:<name>`)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["get", "set", "list", "delete"] },
                "key": { "type": "string" },
                "value": { "type": "string" }
            },
            "required": ["action"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("list");
        let key = args.get("key").and_then(Value::as_str).unwrap_or_default();
        let root = ctx.root;
        match action {
            "list" => {
                let keys = crate::memory::list(root);
                Ok(ToolOutput::ok(if keys.is_empty() { "(no memories)".into() } else { keys.join("\n") }))
            }
            "get" => match crate::memory::get(root, key) {
                Some(v) => Ok(ToolOutput::ok(v)),
                None => Ok(ToolOutput::err(format!("no memory: {key}"))),
            },
            "set" => {
                if !crate::memory::key_ok(key) {
                    return Ok(ToolOutput::err("invalid key (no '/' or '..')"));
                }
                let value = args.get("value").and_then(Value::as_str).unwrap_or_default();
                crate::memory::set(root, key, value)?;
                Ok(ToolOutput::ok(format!("remembered '{key}'")))
            }
            "delete" => match crate::memory::remove(root, key) {
                Ok(()) => Ok(ToolOutput::ok(format!("forgot '{key}'"))),
                Err(_) => Ok(ToolOutput::err(format!("no memory: {key}"))),
            },
            other => Ok(ToolOutput::err(format!("unknown action: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cc_dev_{}_{}", std::process::id(), unique_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    // Monotonic per-process counter: guarantees each test gets its own dir even when
    // the suite runs in parallel (subsec_nanos collided → shared dir → cross-test wipes).
    fn unique_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn milestone_add_deps_and_done_gates_next() {
        let dir = ctx_dir();
        let mut ctx = ToolCtx::new(&dir);
        Milestone.run(json!({ "action": "add", "title": "data model" }), &mut ctx).unwrap();
        Milestone
            .run(json!({ "action": "add", "title": "logic", "deps": [1] }), &mut ctx)
            .unwrap();
        // #2 is blocked behind #1: `next` yields #1.
        let n = Milestone.run(json!({ "action": "next" }), &mut ctx).unwrap();
        assert!(n.content.contains("#1 data model"), "got: {}", n.content);
        // Complete #1 → #2 unblocks.
        Milestone.run(json!({ "action": "done", "id": 1 }), &mut ctx).unwrap();
        let n2 = Milestone.run(json!({ "action": "next" }), &mut ctx).unwrap();
        assert!(n2.content.contains("#2 logic"), "got: {}", n2.content);
        // The milestone renders as done.
        let list = Milestone.run(json!({ "action": "list" }), &mut ctx).unwrap();
        assert!(list.content.contains("[x] #1 data model"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn milestone_done_marks_complete() {
        let dir = ctx_dir();
        let mut ctx = ToolCtx::new(&dir);
        Milestone.run(json!({ "action": "add", "title": "risky" }), &mut ctx).unwrap();
        Milestone.run(json!({ "action": "done", "id": 1 }), &mut ctx).unwrap();
        let list = Milestone.run(json!({ "action": "list" }), &mut ctx).unwrap();
        assert!(list.content.contains("[x] #1 risky"), "got: {}", list.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn milestone_add_creates_without_acceptance() {
        let dir = ctx_dir();
        let mut ctx = ToolCtx::new(&dir);
        let out = Milestone
            .run(json!({"action":"add","title":"core"}), &mut ctx)
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("#1"), "render should show milestone: {}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn memory_persists_across_instances() {
        let dir = ctx_dir();
        {
            let mut ctx = ToolCtx::new(&dir);
            Memory.run(json!({ "action": "set", "key": "goal", "value": "ship v1" }), &mut ctx).unwrap();
            Memory
                .run(json!({ "action": "set", "key": "data:acme", "value": "{\"path\":\"data/acme.json\"}" }), &mut ctx)
                .unwrap();
        }
        // A fresh tool call (new "session") reads it back from disk.
        let mut ctx = ToolCtx::new(&dir);
        let got = Memory.run(json!({ "action": "get", "key": "goal" }), &mut ctx).unwrap();
        assert_eq!(got.content, "ship v1");
        let list = Memory.run(json!({ "action": "list" }), &mut ctx).unwrap();
        assert!(list.content.contains("goal") && list.content.contains("data:acme"));

        Memory.run(json!({ "action": "delete", "key": "goal" }), &mut ctx).unwrap();
        let gone = Memory.run(json!({ "action": "get", "key": "goal" }), &mut ctx).unwrap();
        assert!(gone.is_error);

        // Rejects unsafe keys.
        let bad = Memory.run(json!({ "action": "set", "key": "../escape", "value": "x" }), &mut ctx).unwrap();
        assert!(bad.is_error);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn milestone_plan_shows_existing_or_hint() {
        let dir = ctx_dir();
        let mut ctx = ToolCtx::new(&dir);
        Milestone.run(json!({ "action": "add", "title": "core" }), &mut ctx).unwrap();
        // No plan yet → helpful hint.
        let out = Milestone.run(json!({ "action": "plan", "id": 1 }), &mut ctx).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("has no plan yet"), "got: {}", out.content);
        // Write a plan, then it should be shown.
        crate::milestone_plan::write_plan(
            &dir,
            &crate::milestone_plan::MilestonePlan {
                milestone_id: 1,
                title: "core".into(),
                skill_used: "engineer-coach".into(),
                created_at: "now".into(),
                acceptance_criteria: vec!["works".into()],
                scope: crate::milestone_plan::MilestoneScope {
                    files_to_create: vec!["src/x.rs".into()],
                    files_to_modify: vec![],
                    estimated_lines: 10,
                },
                risks: vec!["risk1".into()],
                test_requirements: "unit tests".into(),
            },
        )
        .unwrap();
        let out2 = Milestone.run(json!({ "action": "plan", "id": 1 }), &mut ctx).unwrap();
        assert!(!out2.is_error);
        assert!(out2.content.contains("engineer-coach"), "got: {}", out2.content);
        assert!(out2.content.contains("risk1"), "got: {}", out2.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_requires_permission_diff_is_free() {
        assert!(matches!(Diff.permission(&json!({}), Path::new(".")), Permission::None));
        match Commit.permission(&json!({ "message": "x" }), Path::new(".")) {
            Permission::Ask { key } => assert_eq!(key, "commit"),
            _ => panic!("expected Ask"),
        }
    }
}
