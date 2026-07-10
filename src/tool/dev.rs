// Development tools: commit (Ask), diff / plan / todo (None). Local dev scaffolding.
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde::{Deserialize, Serialize};
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
        Ok(ToolOutput { content: out, is_error: err })
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoItem {
    id: u64,
    text: String,
    done: bool,
}

fn todos_path(root: &Path) -> std::path::PathBuf {
    root.join("todos.json")
}
fn load_todos(root: &Path) -> Vec<TodoItem> {
    std::fs::read_to_string(todos_path(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
fn save_todos(root: &Path, todos: &[TodoItem]) -> anyhow::Result<()> {
    std::fs::write(todos_path(root), serde_json::to_string_pretty(todos)?)?;
    Ok(())
}
fn render_todos(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "(no todos)".into();
    }
    todos
        .iter()
        .map(|t| format!("[{}] #{} {}", if t.done { "x" } else { " " }, t.id, t.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct Todo;

impl Tool for Todo {
    fn name(&self) -> &str {
        "todo"
    }
    fn description(&self) -> &str {
        "Manage the todo list: action = list | create | update | complete | delete."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "create", "update", "complete", "delete"] },
                "id": { "type": "integer" },
                "text": { "type": "string" }
            },
            "required": ["action"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("list");
        let id = args.get("id").and_then(Value::as_u64);
        let text = args.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut todos = load_todos(ctx.root);

        match action {
            "list" => {}
            "create" => {
                if text.is_empty() {
                    return Ok(ToolOutput::err("create needs `text`"));
                }
                let next = todos.iter().map(|t| t.id).max().unwrap_or(0) + 1;
                todos.push(TodoItem { id: next, text, done: false });
            }
            "update" => match id.and_then(|i| todos.iter_mut().find(|t| t.id == i)) {
                Some(t) => t.text = text,
                None => return Ok(ToolOutput::err("update needs a valid `id`")),
            },
            "complete" => match id.and_then(|i| todos.iter_mut().find(|t| t.id == i)) {
                Some(t) => t.done = true,
                None => return Ok(ToolOutput::err("complete needs a valid `id`")),
            },
            "delete" => match id {
                Some(i) => todos.retain(|t| t.id != i),
                None => return Ok(ToolOutput::err("delete needs `id`")),
            },
            other => return Ok(ToolOutput::err(format!("unknown action: {other}"))),
        }
        save_todos(ctx.root, &todos)?;
        Ok(ToolOutput::ok(render_todos(&todos)))
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
    fn todo_crud_roundtrip() {
        let dir = ctx_dir();
        let mut ctx = ToolCtx::new(&dir);
        Todo.run(json!({ "action": "create", "text": "write tests" }), &mut ctx).unwrap();
        Todo.run(json!({ "action": "create", "text": "ship it" }), &mut ctx).unwrap();
        Todo.run(json!({ "action": "complete", "id": 1 }), &mut ctx).unwrap();
        let out = Todo.run(json!({ "action": "delete", "id": 2 }), &mut ctx).unwrap();
        assert!(out.content.contains("[x] #1 write tests"));
        assert!(!out.content.contains("#2"));
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
    fn commit_requires_permission_diff_is_free() {
        assert!(matches!(Diff.permission(&json!({}), Path::new(".")), Permission::None));
        match Commit.permission(&json!({ "message": "x" }), Path::new(".")) {
            Permission::Ask { key } => assert_eq!(key, "commit"),
            _ => panic!("expected Ask"),
        }
    }
}
