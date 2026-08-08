// Cron scheduling tools (Task 2): cron expression parsing + cron_create/delete/list.
// In-memory CronStore singleton (Mutex<Vec<CronEntry>>), not persisted across restarts.
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use chrono::{Datelike, Timelike};
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

/// Check whether a given time matches a 5-field cron expression.
/// Uses the local timezone. Returns true if every field matches the given instant.
pub fn cron_matches_now(cron: &str, now: &std::time::SystemTime) -> bool {
    let fields: Vec<&str> = cron.trim().split_whitespace().collect();
    if fields.len() != 5 { return false; }
    let dt: chrono::DateTime<chrono::Local> = (*now).into();
    let values = [dt.minute() as i32, dt.hour() as i32, dt.day() as i32, dt.month() as i32, dt.weekday().num_days_from_sunday() as i32];
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

    #[test]
    fn poll_cron_due_fires_matching_and_removes_one_shot() {
        // CRON_STORE is a process-wide singleton; clean it between tests.
        CRON_STORE.lock().unwrap().entries.clear();
        CRON_STORE.lock().unwrap().next_id = 1;

        // Match current minute so it fires now.
        let dt = chrono::Local::now();
        let cron_now = format!("{} * * * *", dt.minute());
        let recurring = CRON_STORE.lock().unwrap()
            .create(cron_now.clone(), "recurring-fire".into(), "".into(), false).unwrap();
        let one_shot = CRON_STORE.lock().unwrap()
            .create(cron_now.clone(), "one-shot-fire".into(), "".into(), true).unwrap();

        let due = poll_cron_due();
        assert_eq!(due, vec!["recurring-fire".to_string(), "one-shot-fire".to_string()]);

        // One-shot removed; recurring stays.
        let remaining: Vec<String> = CRON_STORE.lock().unwrap().list().iter()
            .map(|e| e.prompt.clone()).collect();
        assert_eq!(remaining, vec!["recurring-fire".to_string()]);

        // Immediate re-poll within the same minute must not re-fire.
        let due2 = poll_cron_due();
        assert!(due2.is_empty(), "no re-fire within the same minute");

        let _ = (recurring, one_shot);
        CRON_STORE.lock().unwrap().entries.clear();
    }
}
