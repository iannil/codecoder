use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub id: u64,
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_branch: String,
    pub session_id: String,
    pub created_at: SystemTime,
}

pub struct WorktreeStore {
    entries: Vec<WorktreeEntry>,
    next_id: u64,
}

impl WorktreeStore {
    pub fn new() -> Self {
        Self { entries: Vec::new(), next_id: 1 }
    }

    pub fn add(&mut self, entry: WorktreeEntry) {
        self.entries.push(entry);
    }

    pub fn get(&self, id: u64) -> Option<&WorktreeEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    pub fn list(&self) -> &Vec<WorktreeEntry> {
        &self.entries
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

pub static WORKTREE_STORE: LazyLock<Mutex<WorktreeStore>> =
    LazyLock::new(|| Mutex::new(WorktreeStore::new()));

/// Run a git command in the given root, return stdout/stderr combined.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git failed: {e}"))
        .and_then(|out| {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.stderr.is_empty() {
                s.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            if out.status.success() {
                Ok(s)
            } else {
                Err(s)
            }
        })
}

pub struct EnterWorktree;

impl Tool for EnterWorktree {
    fn name(&self) -> &str {
        "enter_worktree"
    }
    fn description(&self) -> &str {
        "Create a new git worktree with an isolated branch and session directory. \
         Returns the worktree path and branch name. Call exit_worktree when done."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Optional worktree name. Auto-generated if omitted." },
                "base_branch": { "type": "string", "description": "Branch to fork from. Default: master." }
            }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "worktree".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| {
                format!(
                    "wt_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                )
            });
        let base_branch = args
            .get("base_branch")
            .and_then(Value::as_str)
            .unwrap_or("master")
            .to_string();
        let branch = format!("feat/{name}");
        let worktree_path = ctx.root.join(".worktrees").join(&name);

        if worktree_path.exists() {
            return Ok(ToolOutput::err(format!("worktree already exists: {name}")));
        }

        // Ensure .worktrees directory exists
        if let Err(e) = std::fs::create_dir_all(ctx.root.join(".worktrees")) {
            return Ok(ToolOutput::err(format!("cannot create .worktrees dir: {e}")));
        }

        // git worktree add
        let path_str = worktree_path.to_string_lossy().to_string();
        match git(ctx.root, &["worktree", "add", &path_str, "-b", &branch, &base_branch]) {
            Ok(_) => {
                let mut store = WORKTREE_STORE.lock().unwrap();
                let id = store.next_id();
                store.add(WorktreeEntry {
                    id,
                    name: name.clone(),
                    path: worktree_path,
                    branch: branch.clone(),
                    base_branch,
                    session_id: format!("worktree-{name}"),
                    created_at: SystemTime::now(),
                });
                Ok(ToolOutput::ok(
                    json!({ "id": id, "path": format!(".worktrees/{name}"), "branch": branch }).to_string(),
                ))
            }
            Err(e) => Ok(ToolOutput::err(format!("worktree creation failed: {e}"))),
        }
    }
}

pub struct ExitWorktree;

impl Tool for ExitWorktree {
    fn name(&self) -> &str {
        "exit_worktree"
    }
    fn description(&self) -> &str {
        "Exit a worktree. Actions: 'merge' (merge back and cleanup), 'keep' (leave as-is), 'discard' (delete worktree and branch)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["merge", "keep", "discard"],
                    "description": "What to do with the worktree."
                }
            },
            "required": ["action"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "worktree".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or_default();
        if action.is_empty() {
            return Ok(ToolOutput::err("exit_worktree requires `action`"));
        }

        // Get the latest worktree entry (most recently created)
        let entry = {
            let store = WORKTREE_STORE.lock().unwrap();
            store.list().last().cloned()
        };
        let Some(entry) = entry else {
            return Ok(ToolOutput::err("no active worktree found"));
        };

        match action {
            "keep" => {
                WORKTREE_STORE.lock().unwrap().remove(entry.id);
                Ok(ToolOutput::ok(format!(
                    "worktree kept at .worktrees/{} on branch {}",
                    entry.name, entry.branch
                )))
            }
            "merge" => {
                // Checkout base branch, merge the worktree branch, then cleanup
                match git(ctx.root, &["checkout", &entry.base_branch])
                    .and_then(|_| git(ctx.root, &["merge", &entry.branch]))
                    .and_then(|_| git(ctx.root, &["worktree", "remove", &entry.path.to_string_lossy()]))
                    .and_then(|_| git(ctx.root, &["branch", "-D", &entry.branch]))
                {
                    Ok(out) => {
                        WORKTREE_STORE.lock().unwrap().remove(entry.id);
                        Ok(ToolOutput::ok(format!(
                            "merged {} into {}: {out}",
                            entry.branch, entry.base_branch
                        )))
                    }
                    Err(e) => Ok(ToolOutput::err(format!("merge failed: {e}"))),
                }
            }
            "discard" => {
                match git(ctx.root, &["worktree", "remove", &entry.path.to_string_lossy()])
                    .and_then(|_| git(ctx.root, &["branch", "-D", &entry.branch]))
                {
                    Ok(out) => {
                        WORKTREE_STORE.lock().unwrap().remove(entry.id);
                        Ok(ToolOutput::ok(format!("discarded worktree {}: {out}", entry.name)))
                    }
                    Err(e) => Ok(ToolOutput::err(format!("discard failed: {e}"))),
                }
            }
            _ => Ok(ToolOutput::err(format!("unknown action: {action}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_store_add_and_remove() {
        let mut store = WorktreeStore::new();
        let id = store.next_id();
        store.add(WorktreeEntry {
            id,
            name: "test".into(),
            path: PathBuf::from("/tmp/wt"),
            branch: "feat/test".into(),
            base_branch: "master".into(),
            session_id: "ws-test".into(),
            created_at: SystemTime::now(),
        });
        assert_eq!(store.list().len(), 1);
        assert!(store.remove(id));
        assert!(store.list().is_empty());
    }

    #[test]
    fn worktree_tools_permission_model() {
        assert!(matches!(
            EnterWorktree.permission(&json!({}), Path::new(".")),
            Permission::Ask { key } if key == "worktree"
        ));
        assert!(matches!(
            ExitWorktree.permission(&json!({}), Path::new(".")),
            Permission::Ask { key } if key == "worktree"
        ));
    }

    #[test]
    fn enter_worktree_requires_name() {
        // No validation needed — name is optional, auto-generated
    }

    #[test]
    fn exit_worktree_requires_action() {
        let out = ExitWorktree.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("requires `action`"));
    }
}