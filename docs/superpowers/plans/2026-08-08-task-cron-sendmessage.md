# Task Management + Cron + SendMessage 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 CodeCoder 补全 P1 工作流工具：Task 管理（5 个）、Cron 调度（3 个）、SendMessage（1 个），共 9 个工具。

**Architecture:** 三个子系统各自独立。Task 管理和 Cron 用 `LazyLock<Mutex<...>>` 全局单例存内存数据；Cron 复用 daemon 的 `cmd_tx` 通道注入 `AgentCommand::ProcessMessage`；SendMessage 需改造 `agent.rs` 子代理生命周期，给子代理挂一个消息接收通道。

**Tech Stack:** Rust, serde_json, std::sync (Mutex, LazyLock, channel), std::process

---

## 全局约束

- 遵循 ADR 0018 的 `Tool` trait 接口：`name()`、`description()`、`schema()`、`permission()`、`run()`
- 权限模型：Task 管理全部 `Permission::None`；Cron 写操作 `Permission::Ask { key: "cron" }`；SendMessage `Permission::None`
- 错误处理：统一通过 `ToolOutput::err()` 返回，不 panic
- 测试：每个工具都要有单元测试；纯逻辑（cron 解析、task 依赖）可离线测试
- 新工具注册到 `src/tool/mod.rs` 的 `Toolbox::builtin()`
- 现有测试基线：595 个通过（含已实现的 MCP/LSP 工具）

---

### Task 1: Task 管理工具族 — 数据模型 + 5 个工具

**Files:**
- Create: `src/tool/task_manage.rs`
- Modify: `src/tool/mod.rs` (添加 `pub mod task_manage` + 注册 5 个工具)
- Test: 内联在 `src/tool/task_manage.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Tool`, `ToolCtx`, `ToolOutput`, `Permission`
- Produces: `TaskEntry` (struct), `TaskStatus` (enum), `TaskStore` (全局单例), 5 个 Tool 类型

- [ ] **Step 1: 定义数据结构**

在 `src/tool/task_manage.rs` 顶部定义：

```rust
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
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
```

- [ ] **Step 2: 实现 TaskCreate 工具**

```rust
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
```

- [ ] **Step 3: 实现 TaskGet / TaskList 工具**

```rust
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
```

- [ ] **Step 4: 实现 TaskUpdate / TaskStop 工具**

```rust
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
```

- [ ] **Step 5: 单元测试**

```rust
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
```

- [ ] **Step 6: 注册到 Toolbox 并提交**

在 `src/tool/mod.rs` 添加 `pub mod task_manage;` 并在 `Toolbox::builtin()` 中追加：
```rust
Box::new(task_manage::TaskCreate),
Box::new(task_manage::TaskGet),
Box::new(task_manage::TaskList),
Box::new(task_manage::TaskUpdate),
Box::new(task_manage::TaskStop),
```

验证并提交：
```bash
cargo build 2>&1 | tail -3
cargo test task_manage 2>&1 | tail -5
git add src/tool/task_manage.rs src/tool/mod.rs
git commit -m "feat(task): add in-memory task management tools

Implement task_create/get/list/update/stop with a Mutex<Vec<TaskEntry>>
global store, dependency tracking (blocks/blocked_by), and status filter.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Cron 调度工具族 — cron 解析 + 3 个工具

**Files:**
- Create: `src/tool/cron.rs`
- Modify: `src/tool/mod.rs` (添加 `pub mod cron` + 注册 3 个工具)
- Test: 内联在 `src/tool/cron.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Tool`, `ToolCtx`, `ToolOutput`, `Permission`
- Produces: `CronEntry` (struct), `CronStore` (全局单例), `parse_cron` (纯函数), `cron_matches_now` (纯函数), 3 个 Tool 类型

- [ ] **Step 1: 定义数据结构 + cron 解析纯函数**

```rust
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct CronEntry {
    pub id: u64,
    pub cron: String,          // 5-field cron expression
    pub prompt: String,        // prompt injected on fire
    pub description: String,
    pub created_at: SystemTime,
    pub last_fired: Option<SystemTime>,
    pub is_one_shot: bool,
}

pub struct CronStore {
    entries: Vec<CronEntry>,
    next_id: u64,
}

impl CronStore {
    pub fn new() -> Self { Self { entries: Vec::new(), next_id: 1 } }
    pub fn create(&mut self, cron: String, prompt: String, description: String, is_one_shot: bool) -> Result<u64, String> {
        if !is_valid_cron(&cron) {
            return Err(format!("invalid cron expression: {cron}"));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(CronEntry {
            id, cron, prompt, description, created_at: SystemTime::now(),
            last_fired: None, is_one_shot,
        });
        Ok(id)
    }
    pub fn delete(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }
    pub fn list(&self) -> &Vec<CronEntry> { &self.entries }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut CronEntry> { self.entries.iter_mut().find(|e| e.id == id) }
}

pub static CRON_STORE: LazyLock<Mutex<CronStore>> = LazyLock::new(|| Mutex::new(CronStore::new()));

/// Validate a 5-field cron expression: `min hour dom mon dow`.
/// Each field is `*`, a number, a range `a-b`, a step `*/n`, or a comma list.
/// Returns true if the expression is syntactically valid.
pub fn is_valid_cron(cron: &str) -> bool {
    let fields: Vec<&str> = cron.trim().split_whitespace().collect();
    if fields.len() != 5 { return false; }
    let ranges: [(i32, i32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)];
    for (i, field) in fields.iter().enumerate() {
        let (lo, hi) = ranges[i];
        if !valid_field(field, lo, hi) { return false; }
    }
    true
}

fn valid_field(field: &str, lo: i32, hi: i32) -> bool {
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() { return false; }
        // Handle step notation: base/step where base can be */n or a-b/n or n/step
        let (base, step) = match part.split_once('/') {
            Some((b, s)) => (b, Some(s)),
            None => (part, None),
        };
        if let Some(s) = step {
            if s.parse::<i32>().map(|v| v <= 0).unwrap_or(true) { return false; }
        }
        if base == "*" { continue; }
        // Range a-b or single number
        let (a, b) = match base.split_once('-') {
            Some((x, y)) => (x.parse::<i32>().ok(), y.parse::<i32>().ok()),
            None => (base.parse::<i32>().ok(), base.parse::<i32>().ok()),
        };
        let (Some(a), Some(b)) = (a, b) else { return false; };
        if a < lo || b > hi || a > b { return false; }
    }
    true
}

/// Check whether the current time matches a 5-field cron expression.
/// Uses the local timezone. Returns true if every field matches "now".
pub fn cron_matches_now(cron: &str, now: &std::time::SystemTime) -> bool {
    let fields: Vec<&str> = cron.trim().split_whitespace().collect();
    if fields.len() != 5 { return false; }
    let dt: chrono::DateTime<chrono::Local> = now.clone().into();
    let values = [dt.minute(), dt.hour(), dt.day(), dt.month() as i32, dt.weekday().num_days_from_sunday() as i32];
    for (i, field) in fields.iter().enumerate() {
        if !field_matches(field, values[i]) { return false; }
    }
    true
}

fn field_matches(field: &str, value: i32) -> bool {
    for part in field.split(',') {
        let part = part.trim();
        let (base, step) = match part.split_once('/') {
            Some((b, s)) => (b, s.parse::<i32>().ok()),
            None => (part, None),
        };
        let matches = match base {
            "*" => true,
            r => {
                let (a, b) = match r.split_once('-') {
                    Some((x, y)) => (x.parse::<i32>().unwrap_or(-1), y.parse::<i32>().unwrap_or(-1)),
                    None => { let v = r.parse::<i32>().unwrap_or(-1); (v, v) }
                };
                value >= a && value <= b
            }
        };
        if matches {
            if let Some(st) = step {
                // For `*/n` on a range base, value must be congruent to base start.
                let start = if base == "*" { 0 } else { base.split_once('-').map(|(x, _)| x.parse::<i32>().unwrap_or(0)).unwrap_or(0) };
                if value >= start && (value - start) % st == 0 { return true; }
            } else {
                return true;
            }
        }
    }
    false
}
```

**注意：** 上述 `cron_matches_now` 使用了 `chrono` crate。检查 `Cargo.toml` 是否已有 `chrono`；如果没有，添加 `chrono = "0.4"`。若不想引入依赖，可以改用 `std::time` 的 `UNIX_EPOCH` 和手工计算（复杂），所以推荐加 `chrono`。

- [ ] **Step 2: 实现三个 Cron 工具**

```rust
pub struct CronCreate;

impl Tool for CronCreate {
    fn name(&self) -> &str { "cron_create" }
    fn description(&self) -> &str {
        "Register a new cron job. On each matching schedule, the prompt is injected into the agent as a new message. Cron expression is 5 fields: minute hour day-of-month month day-of-week."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cron": { "type": "string", "description": "5-field cron expression, e.g. '*/5 * * * *'." },
                "prompt": { "type": "string", "description": "Prompt to inject on each fire." },
                "description": { "type": "string", "description": "Human-readable description." },
                "is_one_shot": { "type": "boolean", "description": "Fire once then auto-delete; default false." }
            },
            "required": ["cron", "prompt"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "cron".into() }
    }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let cron = args.get("cron").and_then(Value::as_str).unwrap_or_default().to_string();
        let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or_default().to_string();
        if cron.is_empty() || prompt.is_empty() {
            return Ok(ToolOutput::err("cron_create requires `cron` and `prompt`"));
        }
        let description = args.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
        let is_one_shot = args.get("is_one_shot").and_then(Value::as_bool).unwrap_or(false);
        match CRON_STORE.lock().unwrap().create(cron, prompt, description, is_one_shot) {
            Ok(id) => Ok(ToolOutput::ok(json!({ "id": id }).to_string())),
            Err(e) => Ok(ToolOutput::err(e)),
        }
    }
}

pub struct CronDelete;

impl Tool for CronDelete {
    fn name(&self) -> &str { "cron_delete" }
    fn description(&self) -> &str { "Delete a registered cron job by id." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "integer", "description": "Cron job id (required)." } },
            "required": ["id"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "cron".into() }
    }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let Some(id) = args.get("id").and_then(Value::as_u64) else {
            return Ok(ToolOutput::err("cron_delete requires `id`"));
        };
        if CRON_STORE.lock().unwrap().delete(id) {
            Ok(ToolOutput::ok(format!("deleted cron job {id}")))
        } else {
            Ok(ToolOutput::err(format!("no cron job with id {id}")))
        }
    }
}

pub struct CronList;

impl Tool for CronList {
    fn name(&self) -> &str { "cron_list" }
    fn description(&self) -> &str { "List all registered cron jobs." }
    fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let entries = CRON_STORE.lock().unwrap().list().clone();
        let summary: Vec<Value> = entries.iter().map(|e| json!({
            "id": e.id, "cron": e.cron, "description": e.description,
            "prompt": e.prompt, "is_one_shot": e.is_one_shot,
        })).collect();
        Ok(ToolOutput::ok(serde_json::to_string_pretty(&summary).unwrap_or_default()))
    }
}
```

- [ ] **Step 3: 单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cron_accepts_standard_expressions() {
        assert!(is_valid_cron("*/5 * * * *"));
        assert!(is_valid_cron("0 9 * * 1-5"));
        assert!(is_valid_cron("30 14 28 2 *"));
        assert!(is_valid_cron("0,15,30,45 * * * *"));
        assert!(is_valid_cron("*/10 0-5 * * *"));
    }

    #[test]
    fn invalid_cron_rejected() {
        assert!(!is_valid_cron(""));
        assert!(!is_valid_cron("* * *"));       // 3 fields
        assert!(!is_valid_cron("* * * * * *")); // 6 fields
        assert!(!is_valid_cron("60 * * * *"));  // minute 60 out of range
        assert!(!is_valid_cron("* 24 * * *"));  // hour 24 out of range
        assert!(!is_valid_cron("*/0 * * * *")); // step 0 invalid
        assert!(!is_valid_cron("* * 32 * *"));  // day 32 out of range
    }

    #[test]
    fn cron_store_create_validates_and_assigns_ids() {
        let mut store = CronStore::new();
        let id = store.create("*/5 * * * *".into(), "check status".into(), "desc".into(), false).unwrap();
        assert_eq!(id, 1);
        assert!(store.create("bad".into(), "x".into(), "".into(), false).is_err());
    }

    #[test]
    fn cron_store_delete_removes() {
        let mut store = CronStore::new();
        let id = store.create("*/5 * * * *".into(), "x".into(), "".into(), false).unwrap();
        assert!(store.delete(id));
        assert!(!store.delete(id));
        assert!(store.list().is_empty());
    }

    #[test]
    fn cron_tools_permission_model() {
        assert!(matches!(CronCreate.permission(&json!({}), Path::new(".")), Permission::Ask { key } if key == "cron"));
        assert!(matches!(CronDelete.permission(&json!({}), Path::new(".")), Permission::Ask { key } if key == "cron"));
        assert!(matches!(CronList.permission(&json!({}), Path::new(".")), Permission::None));
    }
}
```

- [ ] **Step 4: 注册到 Toolbox 并提交**

在 `src/tool/mod.rs` 添加 `pub mod cron;` 并注册：
```rust
Box::new(cron::CronCreate),
Box::new(cron::CronDelete),
Box::new(cron::CronList),
```

验证并提交：
```bash
cargo build 2>&1 | tail -3
cargo test cron 2>&1 | tail -5
git add src/tool/cron.rs src/tool/mod.rs Cargo.toml Cargo.lock
git commit -m "feat(cron): add cron scheduling tools

Implement cron_create/delete/list with a 5-field cron expression parser
and validation, plus an in-memory CronStore singleton.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Cron 后台调度线程 — 注入 agent 消息

**Files:**
- Modify: `src/daemon/mod.rs` (添加 cron 后台线程)
- Modify: `src/tool/cron.rs` (暴露 `cron_due_entries()` 给 daemon 消费)

**Interfaces:**
- Consumes: `CronStore` (Task 2), `AgentCommand` (来自 `src/agent.rs`)
- Produces: `spawn_cron_thread(cmd_tx: Sender<AgentCommand>)` 函数

- [ ] **Step 1: 在 cron.rs 添加 daemon 可消费的查询函数**

在 `src/tool/cron.rs` 添加（非 Tool，供 daemon 调用）：
```rust
use std::sync::mpsc::Sender;

/// Reap due cron entries: return the prompts that should fire now, and update
/// last_fired. One-shot entries that fire are removed. Called by the daemon's
/// cron thread once per tick.
pub fn poll_cron_due() -> Vec<String> {
    let mut store = CRON_STORE.lock().unwrap();
    let now = std::time::SystemTime::now();
    let mut due = Vec::new();
    let mut to_remove = Vec::new();
    for e in store.entries.iter_mut() {
        if cron_matches_now(&e.cron, &now) {
            // Skip if we already fired within this minute (avoid re-firing).
            let already_fired = e.last_fired.map_or(false, |lf| {
                let secs = now.duration_since(lf).unwrap_or_default().as_secs();
                secs < 60
            });
            if !already_fired {
                due.push(e.prompt.clone());
                e.last_fired = Some(now);
                if e.is_one_shot {
                    to_remove.push(e.id);
                }
            }
        }
    }
    for id in to_remove {
        store.entries.retain(|e| e.id != id);
    }
    due
}
```

- [ ] **Step 2: 在 daemon/mod.rs 添加 cron 后台线程**

在 daemon 启动已有后台线程（如 autotask、workgraph tick）附近，添加一个 cron 线程。它每秒检查一次 `poll_cron_due()`，把到期的 prompt 通过 `cmd_tx` 发送 `AgentCommand::ProcessMessage`。

找到 daemon 中持有 `cmd_tx` 的位置（`AgentCommand` 的发送端），在 `run()` 里追加：

```rust
// Cron scheduler thread: every second, inject due cron prompts as messages.
// (mirrors the autotask thread pattern above)
{
    let cmd_tx_cron = cmd_tx.clone();
    let _cron_handle = std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        for prompt in crate::tool::cron::poll_cron_due() {
            let _ = cmd_tx_cron.send(AgentCommand::ProcessMessage(prompt));
        }
    });
}
```

**注意:** 需要确认 daemon 中 `cmd_tx` 的名称和类型。若 daemon 不直接持 `cmd_tx`，而是通过 `DaemonSessionManager` 管理，则需在 session 创建时把 cron 线程与对应 session 的 `cmd_tx` 绑定。**实现时先读 `src/daemon/mod.rs` 和 `src/daemon/session_manager.rs` 确认 `cmd_tx` 的流向**，再决定线程挂在哪。

- [ ] **Step 3: 提交**

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "test result:" | grep -v "0 passed" | tail -3
git add src/daemon/mod.rs src/tool/cron.rs
git commit -m "feat(cron): add daemon background thread that injects due cron prompts

Poll the cron store every second and inject matching prompts into the
agent as ProcessMessage commands, mirroring the autotask thread pattern.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: SendMessage 工具 — 子代理通信通道

**Files:**
- Create: `src/tool/send_message.rs`
- Modify: `src/agent.rs` (子代理生命周期改造)
- Modify: `src/tool/mod.rs` (注册 `send_message`)
- Test: 内联在 `src/tool/send_message.rs` 的 `#[cfg(test)]`

**Interfaces:**
- Consumes: `Tool`, `ToolCtx`, `ToolOutput`, `Permission`; `AgentRegistry` (新全局单例)
- Produces: `AgentRegistry` (struct), `SendMessage` (Tool)

- [ ] **Step 1: 定义全局 Agent 注册表**

在 `src/tool/send_message.rs` 定义：

```rust
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

/// Global registry of live sub-agents, keyed by agent_id, holding a channel
/// through which the parent can send them messages.
pub struct AgentRegistry {
    agents: HashMap<String, std::sync::mpsc::Sender<String>>,
}

impl AgentRegistry {
    pub fn new() -> Self { Self { agents: HashMap::new() } }
    pub fn register(&mut self, id: String, tx: std::sync::mpsc::Sender<String>) {
        self.agents.insert(id, tx);
    }
    pub fn unregister(&mut self, id: &str) { self.agents.remove(id); }
    pub fn send(&mut self, id: &str, message: &str) -> Result<(), String> {
        match self.agents.get(id) {
            Some(tx) => tx.send(message.to_string()).map_err(|_| format!("agent {id} is no longer reachable")),
            None => Err(format!("no live agent with id {id}")),
        }
    }
    pub fn contains(&self, id: &str) -> bool { self.agents.contains_key(id) }
}

pub static AGENT_REGISTRY: LazyLock<Mutex<AgentRegistry>> = LazyLock::new(|| Mutex::new(AgentRegistry::new()));
```

- [ ] **Step 2: 实现 SendMessage 工具**

```rust
pub struct SendMessage;

impl Tool for SendMessage {
    fn name(&self) -> &str { "send_message" }
    fn description(&self) -> &str {
        "Send a message to a live sub-agent (identified by its agent_id) or to the parent agent. For sub-agent communication and coordination."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "Target: a sub-agent's id, or the special value 'main' for the parent." },
                "message": { "type": "string", "description": "Message content (required)." },
                "summary": { "type": "string", "description": "Short summary for display (optional)." }
            },
            "required": ["to", "message"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let to = args.get("to").and_then(Value::as_str).unwrap_or_default().to_string();
        let message = args.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
        if to.is_empty() || message.is_empty() {
            return Ok(ToolOutput::err("send_message requires `to` and `message`"));
        }
        // 'main' = parent agent. This is handled by the parent's own message loop,
        // but for now we route sub-agent -> parent via the registry's parent channel.
        if to == "main" {
            // Parent agent's channel is registered under "main" by the spawning logic.
            match AGENT_REGISTRY.lock().unwrap().send("main", &message) {
                Ok(()) => Ok(ToolOutput::ok("message sent to parent")),
                Err(e) => Ok(ToolOutput::err(e)),
            }
        } else {
            match AGENT_REGISTRY.lock().unwrap().send(&to, &message) {
                Ok(()) => Ok(ToolOutput::ok(format!("message sent to {to}")),
                Err(e) => Ok(ToolOutput::err(e)),
            }
        }
    }
}
```

**注意:** 这里 `send_message` 的 `to: "main"` 需要在 Task 5 的子代理生命周期中处理——子代理循环里，若收到来自父类的消息来自 `main`，则可选地转发给父级。**Task 4 先实现注册表 + 工具本身，Task 5 接入子代理生命周期。**

- [ ] **Step 3: 单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn registry_register_send_unregister() {
        let mut reg = AgentRegistry::new();
        let (tx, rx) = mpsc::channel();
        reg.register("sub_1".into(), tx);
        assert!(reg.contains("sub_1"));
        assert!(reg.send("sub_1", "hello").is_ok());
        assert_eq!(rx.try_recv().unwrap(), "hello");
        reg.unregister("sub_1");
        assert!(!reg.contains("sub_1"));
        assert!(reg.send("sub_1", "x").is_err());
    }

    #[test]
    fn send_message_tool_requires_to_and_message() {
        let out = SendMessage.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
        let out = SendMessage.run(json!({"to": "x"}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn send_message_to_unknown_agent_errors() {
        let out = SendMessage.run(json!({"to": "nonexistent", "message": "hi"}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn send_message_permission_none() {
        assert!(matches!(SendMessage.permission(&json!({}), Path::new(".")), Permission::None));
    }
}
```

- [ ] **Step 4: 注册并提交**

在 `src/tool/mod.rs` 添加 `pub mod send_message;` 并注册 `Box::new(send_message::SendMessage)`。

```bash
cargo build 2>&1 | tail -3
cargo test send_message 2>&1 | tail -5
git add src/tool/send_message.rs src/tool/mod.rs
git commit -m "feat(send_message): add sub-agent messaging tool and registry

Implement send_message tool with a global AgentRegistry of live sub-agent
channels. Wire-up to the sub-agent lifecycle lands in the next task.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 子代理生命周期接入 SendMessage

**Files:**
- Modify: `src/agent.rs` (子代理 spawn 时注册通道 + 子代理循环监听消息)
- Modify: `src/tool/send_message.rs` (若需要补充父级通道注册)

**Interfaces:**
- Consumes: `AgentRegistry` (Task 4), `spawn_sub_agent_fresh`/`spawn_sub_agent_fork` (来自 `src/agent.rs`)
- Produces: 子代理可接收来自父类的消息

- [ ] **Step 1: 子代理 spawn 时注册消息通道**

在 `src/agent.rs` 的 `spawn_sub_agent_fresh`（以及 `spawn_sub_agent_fork`、`spawn_sub_agent_text`）中，创建子代理时同时创建 `(msg_tx, msg_rx)` 通道，并把 `msg_tx` 注册到 `AGENT_REGISTRY`：

```rust
// 在 spawn_sub_agent_fresh 中，spawn 子进程线程前：
let (msg_tx, msg_rx) = std::sync::mpsc::channel::<String>();
let agent_id = format!("fresh_{}", std::process::id());
// 注册父->子通道
crate::tool::send_message::AGENT_REGISTRY.lock().unwrap().register(agent_id.clone(), msg_tx);
```

- [ ] **Step 2: 把消息通道传给子代理循环**

子代理的 `process_turn` 目前是同步阻塞的。需要让子代理在 turn 之间能接收消息。**关键设计决策：**

方案 A（推荐）：子代理的 `process_turn` 改为轮询 `msg_rx.try_recv()`——在每个 LLM 调用之间/turn 开始时，检查是否有父类消息，若有则作为额外的 user message 追加到对话。

方案 B：子代理在后台线程中运行，父类消息通过共享队列注入。

**实现时选择方案 A**，在 `AgentLoop` 的 turn 循环里，turn 开始前 `while let Ok(m)=msg_rx.try_recv(){ messages.push(...) }`。

- [ ] **Step 3: 子代理结束时注销**

在子代理线程结束处（`handle.join()` 后或线程内结束时），调用：
```rust
crate::tool::send_message::AGENT_REGISTRY.lock().unwrap().unregister(&agent_id);
```

- [ ] **Step 4: 测试 + 提交**

由于这个任务涉及真实子代理生命周期，单元测试较难覆盖。可以通过一个 L1 集成测试（模拟 LLM 的 StubClient）验证：父 agent 派发子代理，子代理调用 `send_message` 回传消息，父 agent 收到。

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "test result:" | grep -v "0 passed" | tail -3
git add src/agent.rs src/tool/send_message.rs
git commit -m "feat(send_message): wire sub-agent lifecycle to messaging registry

Register a message channel per sub-agent on spawn, drain parent messages
into the sub-agent turn loop, and unregister on completion.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 文档更新 + 集成验证

**Files:**
- Modify: `README.md` (工具表 31→40, 添加 9 个工具)
- Create: `docs/adr/0041-task-cron-sendmessage.md` (架构决策记录)

- [ ] **Step 1: 更新 README 工具表**

在工具表末尾添加：
```
| `task_create` | 创建内存任务（返回 id） |
| `task_get` | 按 id 获取任务详情 |
| `task_list` | 列出任务（可按 status 过滤） |
| `task_update` | 更新任务字段/依赖 |
| `task_stop` | 停止任务（标记为 deleted） |
| `cron_create` | 注册 cron 定时任务（到点注入 prompt） |
| `cron_delete` | 删除 cron 任务 |
| `cron_list` | 列出 cron 任务 |
| `send_message` | 向子代理/父代理发送消息 |
```
工具计数 31→40。

- [ ] **Step 2: 编写 ADR 0041**

创建 `docs/adr/0041-task-cron-sendmessage.md`，记录：
- **决策：** Task 管理用内存 `Mutex<Vec<TaskEntry>>`（非持久化，与持久化 `workgraph.json` 互补）；Cron 用内存 `CronStore` + daemon 后台线程轮询注入 `AgentCommand::ProcessMessage`；SendMessage 用全局 `AgentRegistry` 管理子代理通道
- **理由：** Task 是轻量级会话内跟踪，无需跨重启持久化；Cron 复用 autotask 的 `cmd_tx` 注入模式，避免引入外部调度器；SendMessage 双向通信需要改造子代理生命周期
- **后果：** Task 在 daemon 重启后消失；Cron 调度依赖 daemon 常驻；SendMessage 修改了子代理 spawn 路径
- **替代方案：** 外部 cron（未采用，避免依赖）；文件持久化 task（未采用，与 milestone 重复）

- [ ] **Step 3: 运行完整测试套件并提交**

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "test result:" | grep -v "0 passed"
git add README.md docs/adr/0041-task-cron-sendmessage.md
git commit -m "docs: add ADR 0041, update README for task/cron/sendmessage tools

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 执行顺序

1. **Task 1** → Task 管理工具族（5 个工具，独立）
2. **Task 2** → Cron 调度工具族（3 个工具 + cron 解析，独立）
3. **Task 3** → Cron 后台线程（依赖 Task 2 + daemon cmd_tx）
4. **Task 4** → SendMessage 工具 + 注册表（基础实现）
5. **Task 5** → 子代理生命周期接入（依赖 Task 4，最复杂）
6. **Task 6** → 文档更新 + 集成验证

Task 1 和 Task 2 可并行；Task 4 与 Task 1/2 独立。Task 3 依赖 Task 2，Task 5 依赖 Task 4。