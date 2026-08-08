use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    pub id: u64,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub created_at: SystemTime,
    pub owner: Option<String>,
    pub blocked_by: Vec<u64>,
    pub blocks: Vec<u64>,
    pub active_form: Option<String>,
}

/// In-memory task store (Mutex<Vec<TaskEntry>>). Not persisted across restarts.
pub struct TaskStore {
    tasks: Vec<TaskEntry>,
    next_id: u64,
}

impl TaskStore {
    pub fn new() -> Self {
        Self { tasks: Vec::new(), next_id: 1 }
    }
    pub fn create(&mut self, subject: String, description: String, status: TaskStatus,
                  active_form: Option<String>, owner: Option<String>, blocked_by: Vec<u64>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        // Add reverse dependency: each task in blocked_by now has this task in its blocks.
        for &b in &blocked_by {
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == b) {
                t.blocks.push(id);
            }
        }
        self.tasks.push(TaskEntry {
            id, subject, description, status, created_at: SystemTime::now(),
            owner, blocked_by, blocks: Vec::new(), active_form,
        });
        id
    }
    pub fn get(&self, id: u64) -> Option<&TaskEntry> { self.tasks.iter().find(|t| t.id == id) }
    pub fn list(&self, status: Option<TaskStatus>) -> Vec<&TaskEntry> {
        self.tasks.iter().filter(|t| status.map_or(true, |s| t.status == s)).collect()
    }
    pub fn update(&mut self, id: u64, status: Option<TaskStatus>, subject: Option<String>,
                  description: Option<String>, owner: Option<String>, active_form: Option<String>,
                  add_blocks: Vec<u64>, add_blocked_by: Vec<u64>) -> bool {
        let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) else { return false };
        if let Some(s) = status { t.status = s; }
        if let Some(s) = subject { t.subject = s; }
        if let Some(d) = description { t.description = d; }
        if let Some(o) = owner { t.owner = Some(o); }
        if let Some(a) = active_form { t.active_form = Some(a); }
        for &b in &add_blocks { if !t.blocks.contains(&b) { t.blocks.push(b); } }
        for &b in &add_blocked_by { if !t.blocked_by.contains(&b) { t.blocked_by.push(b); } }
        true
    }
    pub fn stop(&mut self, id: u64) -> bool {
        let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) else { return false };
        t.status = TaskStatus::Deleted;
        true
    }
}

pub static TASK_STORE: LazyLock<Mutex<TaskStore>> = LazyLock::new(|| Mutex::new(TaskStore::new()));

pub struct TaskCreate;

impl Tool for TaskCreate {
    fn name(&self) -> &str { "task_create" }
    fn description(&self) -> &str {
        "Create a new task in the in-memory task list. Returns the new task id."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subject": { "type": "string", "description": "Brief title of the task (required)." },
                "description": { "type": "string", "description": "Detailed description." },
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"], "description": "Initial status; default pending." },
                "active_form": { "type": "string", "description": "Present-continuous form shown while in progress." },
                "owner": { "type": "string", "description": "Who the task is assigned to." },
                "blocked_by": { "type": "array", "items": { "type": "integer" }, "description": "Task ids that must complete first." }
            },
            "required": ["subject"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let subject = args.get("subject").and_then(Value::as_str).unwrap_or_default().to_string();
        if subject.is_empty() {
            return Ok(ToolOutput::err("task_create requires `subject`"));
        }
        let description = args.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
        let status = match args.get("status").and_then(Value::as_str) {
            Some("in_progress") => TaskStatus::InProgress,
            Some("completed") => TaskStatus::Completed,
            Some("deleted") => TaskStatus::Deleted,
            _ => TaskStatus::Pending,
        };
        let active_form = args.get("active_form").and_then(Value::as_str).map(String::from);
        let owner = args.get("owner").and_then(Value::as_str).map(String::from);
        let blocked_by: Vec<u64> = args.get("blocked_by")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        let id = TASK_STORE.lock().unwrap().create(subject, description, status, active_form, owner, blocked_by);
        Ok(ToolOutput::ok(json!({ "id": id }).to_string()))
    }
}

pub struct TaskGet;

impl Tool for TaskGet {
    fn name(&self) -> &str { "task_get" }
    fn description(&self) -> &str { "Get a single task by its id." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "task_id": { "type": "integer", "description": "Task id (required)." } },
            "required": ["task_id"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let Some(id) = args.get("task_id").and_then(Value::as_u64) else {
            return Ok(ToolOutput::err("task_get requires `task_id`"));
        };
        let store = TASK_STORE.lock().unwrap();
        match store.get(id) {
            Some(t) => Ok(ToolOutput::ok(serde_json::to_string_pretty(t).unwrap_or_default())),
            None => Ok(ToolOutput::err(format!("no task with id {id}"))),
        }
    }
}

pub struct TaskList;

impl Tool for TaskList {
    fn name(&self) -> &str { "task_list" }
    fn description(&self) -> &str { "List tasks, optionally filtered by status." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"] } }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let status = match args.get("status").and_then(Value::as_str) {
            Some("pending") => Some(TaskStatus::Pending),
            Some("in_progress") => Some(TaskStatus::InProgress),
            Some("completed") => Some(TaskStatus::Completed),
            Some("deleted") => Some(TaskStatus::Deleted),
            _ => None,
        };
        let store = TASK_STORE.lock().unwrap();
        let tasks: Vec<&TaskEntry> = store.list(status);
        // Compact summary: id, status, subject, owner, blocked_by.
        let summary: Vec<Value> = tasks.iter().map(|t| json!({
            "id": t.id, "status": t.status, "subject": t.subject,
            "owner": t.owner, "blocked_by": t.blocked_by,
        })).collect();
        Ok(ToolOutput::ok(serde_json::to_string_pretty(&summary).unwrap_or_default()))
    }
}

pub struct TaskUpdate;

impl Tool for TaskUpdate {
    fn name(&self) -> &str { "task_update" }
    fn description(&self) -> &str { "Update a task's fields or dependencies." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "integer", "description": "Task id (required)." },
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"] },
                "subject": { "type": "string" },
                "description": { "type": "string" },
                "owner": { "type": "string" },
                "active_form": { "type": "string" },
                "add_blocks": { "type": "array", "items": { "type": "integer" }, "description": "Task ids this task now blocks." },
                "add_blocked_by": { "type": "array", "items": { "type": "integer" }, "description": "Task ids this task now depends on." }
            },
            "required": ["task_id"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let Some(id) = args.get("task_id").and_then(Value::as_u64) else {
            return Ok(ToolOutput::err("task_update requires `task_id`"));
        };
        let status = match args.get("status").and_then(Value::as_str) {
            Some("pending") => Some(TaskStatus::Pending),
            Some("in_progress") => Some(TaskStatus::InProgress),
            Some("completed") => Some(TaskStatus::Completed),
            Some("deleted") => Some(TaskStatus::Deleted),
            _ => None,
        };
        let subject = args.get("subject").and_then(Value::as_str).map(String::from);
        let description = args.get("description").and_then(Value::as_str).map(String::from);
        let owner = args.get("owner").and_then(Value::as_str).map(String::from);
        let active_form = args.get("active_form").and_then(Value::as_str).map(String::from);
        let add_blocks: Vec<u64> = args.get("add_blocks").and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        let add_blocked_by: Vec<u64> = args.get("add_blocked_by").and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        let ok = TASK_STORE.lock().unwrap().update(id, status, subject, description, owner, active_form, add_blocks, add_blocked_by);
        if ok { Ok(ToolOutput::ok(format!("updated task {id}"))) }
        else { Ok(ToolOutput::err(format!("no task with id {id}"))) }
    }
}

pub struct TaskStop;

impl Tool for TaskStop {
    fn name(&self) -> &str { "task_stop" }
    fn description(&self) -> &str { "Stop a task by marking it as deleted." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "task_id": { "type": "integer", "description": "Task id (required)." } },
            "required": ["task_id"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let Some(id) = args.get("task_id").and_then(Value::as_u64) else {
            return Ok(ToolOutput::err("task_stop requires `task_id`"));
        };
        let ok = TASK_STORE.lock().unwrap().stop(id);
        if ok { Ok(ToolOutput::ok(format!("stopped task {id}"))) }
        else { Ok(ToolOutput::err(format!("no task with id {id}"))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assigns_incrementing_ids() {
        let mut store = TaskStore::new();
        let a = store.create("first".into(), String::new(), TaskStatus::Pending, None, None, vec![]);
        let b = store.create("second".into(), String::new(), TaskStatus::Pending, None, None, vec![]);
        assert!(b > a);
        assert_eq!(store.get(a).unwrap().subject, "first");
    }

    #[test]
    fn create_records_reverse_blocks() {
        let mut store = TaskStore::new();
        let dep = store.create("dep".into(), String::new(), TaskStatus::Pending, None, None, vec![]);
        let main = store.create("main".into(), String::new(), TaskStatus::Pending, None, None, vec![dep]);
        assert_eq!(store.get(dep).unwrap().blocks, vec![main]);
        assert_eq!(store.get(main).unwrap().blocked_by, vec![dep]);
    }

    #[test]
    fn list_filters_by_status() {
        let mut store = TaskStore::new();
        store.create("a".into(), String::new(), TaskStatus::Pending, None, None, vec![]);
        store.create("b".into(), String::new(), TaskStatus::InProgress, None, None, vec![]);
        assert_eq!(store.list(Some(TaskStatus::Pending)).len(), 1);
        assert_eq!(store.list(None).len(), 2);
    }

    #[test]
    fn update_changes_fields_and_appends_deps() {
        let mut store = TaskStore::new();
        let id = store.create("x".into(), String::new(), TaskStatus::Pending, None, None, vec![]);
        let ok = store.update(id, Some(TaskStatus::InProgress), Some("y".into()), None, None, None, vec![], vec![5]);
        assert!(ok);
        let t = store.get(id).unwrap();
        assert_eq!(t.status, TaskStatus::InProgress);
        assert_eq!(t.subject, "y");
        assert_eq!(t.blocked_by, vec![5]);
    }

    #[test]
    fn stop_marks_deleted() {
        let mut store = TaskStore::new();
        let id = store.create("x".into(), String::new(), TaskStatus::Pending, None, None, vec![]);
        assert!(store.stop(id));
        assert_eq!(store.get(id).unwrap().status, TaskStatus::Deleted);
        assert!(!store.stop(999));
    }

    #[test]
    fn task_create_tool_requires_subject() {
        let out = TaskCreate.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn task_permissions_all_none() {
        assert!(matches!(TaskCreate.permission(&json!({}), Path::new(".")), Permission::None));
        assert!(matches!(TaskGet.permission(&json!({}), Path::new(".")), Permission::None));
        assert!(matches!(TaskList.permission(&json!({}), Path::new(".")), Permission::None));
        assert!(matches!(TaskUpdate.permission(&json!({}), Path::new(".")), Permission::None));
        assert!(matches!(TaskStop.permission(&json!({}), Path::new(".")), Permission::None));
    }
}