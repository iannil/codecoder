//! Live observability for headless Background runs (spec 2026-07-25, ADR 0039).
//! Tees each event to stderr (human) and `<root>/.ccd.bg.ndjson` (machine/tail).
//! Auto-rotates NDJSON at 10 MB to prevent unbounded disk usage (P2-5).
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::trace::observer_set::Observer;
use crate::trace::types::*;

#[cfg(test)]
use serde_json::json;

/// Max NDJSON file size before rotation (10 MB).
const ROTATE_SIZE: u64 = 10 * 1024 * 1024;
/// Max rotated files to keep.
const MAX_ROTATED: usize = 3;

pub struct BgObserver {
    ndjson: Option<std::fs::File>,
    /// Path to the NDJSON file, saved for rotation checks.
    ndjson_path: Option<PathBuf>,
}

impl BgObserver {
    /// Truncate-create `<root>/.ccd.bg.ndjson`, starting a fresh event stream for
    /// one BG run. Call EXACTLY ONCE at run start; subsequent observers over the
    /// same run use `new` (append). NDJSON is best-effort: if the file can't be
    /// opened, stderr output still happens.
    pub fn start_run(root: &Path) -> Self {
        let path = root.join(".ccd.bg.ndjson");
        let ndjson = std::fs::File::create(&path).ok();
        Self { ndjson, ndjson_path: Some(path) }
    }

    /// Open `<root>/.ccd.bg.ndjson` in APPEND mode (create if missing, never
    /// truncate) so the full event stream accumulates across all milestones of a
    /// run. Use `start_run` once at run start to reset the file. NDJSON is
    /// best-effort: if the file can't be opened, stderr output still happens.
    pub fn new(root: &Path) -> Self {
        let path = root.join(".ccd.bg.ndjson");
        let ndjson = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        Self { ndjson, ndjson_path: Some(path) }
    }

    /// Emit one event: stderr line + one JSON line to the NDJSON file.
    pub fn emit(&mut self, kind: &str, msg: &str) {
        self.emit_with_data(kind, msg, None);
    }

    /// Emit one event with optional structured data (extra JSON fields merged
    /// into the top-level NDJSON object alongside `kind` and `msg`).
    pub fn emit_with_data(&mut self, kind: &str, msg: &str, data: Option<serde_json::Value>) {
        eprintln!("[bg] {kind}: {msg}");
        // 每次 emit 前检查文件大小,超限则轮转(仅当有路径)。
        if let Some(ref p) = self.ndjson_path {
            if std::fs::metadata(p).map(|m| m.len() > ROTATE_SIZE).unwrap_or(false) {
                let _ = rotate_ndjson(p);
                // 轮转后需重建文件句柄。
                if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                    self.ndjson = Some(f);
                }
            }
        }
        if let Some(f) = self.ndjson.as_mut() {
            let line = if let Some(d) = data {
                let mut obj = serde_json::json!({ "kind": kind, "msg": msg });
                if let Some(obj_map) = obj.as_object_mut() {
                    if let Some(data_obj) = d.as_object() {
                        for (k, v) in data_obj {
                            obj_map.insert(k.clone(), v.clone());
                        }
                    }
                }
                obj.to_string()
            } else {
                serde_json::json!({ "kind": kind, "msg": msg }).to_string()
            };
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

// === Observer trait impl ===

impl Observer for BgObserver {
    fn on_point(&mut self, event: &PointEvent) {
        match &event.kind {
            EventKind::ToolCallBegin { name, .. } => {
                self.emit("tool_started", name);
            }
            EventKind::ToolCallEnd { name, is_error, .. } => {
                if *is_error {
                    self.emit("tool_error", name);
                } else {
                    self.emit("tool_finished", name);
                }
            }
            EventKind::MilestoneStatus { id, title, old_status, new_status } => {
                self.emit("milestone", &format!("#{id} ({title}): {old_status} → {new_status}"));
            }
            EventKind::PermissionFull { key, decision, tool, .. } => {
                if matches!(decision, PermissionDecision::Denied) {
                    self.emit("denied", &format!("{tool}:{key}"));
                }
            }
            EventKind::RetryEvent { kind, attempt, .. } => {
                self.emit("retry", &format!("{kind} attempt #{attempt}"));
            }
            EventKind::Notice { text } => {
                self.emit("notice", text);
            }
            _ => {}
        }
    }
}

// === External emit methods (for events emitted outside AgentLoop) ===

impl BgObserver {
    /// For events emitted outside AgentLoop (run start/end, budget, etc.)
    pub fn emit_external(&mut self, kind: &str, msg: &str) {
        self.emit(kind, msg);
    }

    /// For events emitted outside AgentLoop, with structured data.
    pub fn emit_external_with_data(&mut self, kind: &str, msg: &str, data: Option<serde_json::Value>) {
        self.emit_with_data(kind, msg, data);
    }
}

/// 轮转 NDJSON 文件:重命名 `.ccd.bg.ndjson` → `.ccd.bg.1.ndjson`,
/// 清理超过 MAX_ROTATED(3) 的旧轮转文件。
fn rotate_ndjson(path: &Path) -> std::io::Result<()> {
    // 先清理超过限额的旧轮转文件: .ccd.bg.3.ndjson → 删除。
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    for i in (MAX_ROTATED..100).rev() {
        let old = dir.join(format!(".ccd.bg.{i}.ndjson"));
        let _ = std::fs::remove_file(&old);
    }
    // 已有轮转文件依次后移: .ccd.bg.1.ndjson → .ccd.bg.2.ndjson
    for i in (1..MAX_ROTATED).rev() {
        let src = dir.join(format!(".ccd.bg.{i}.ndjson"));
        let dst = dir.join(format!(".ccd.bg.{}.ndjson", i + 1));
        let _ = std::fs::rename(&src, &dst);
    }
    // 当前文件 → .ccd.bg.1.ndjson
    let rotated = dir.join(".ccd.bg.1.ndjson");
    std::fs::rename(path, &rotated)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn emit_with_data_includes_extra_fields() {
        let dir = tempdir().unwrap();
        let mut obs = BgObserver::new(dir.path());
        obs.emit_with_data("llm_call", "done", Some(json!({"prompt_tokens": 100, "completion_tokens": 50})));
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(v["kind"], "llm_call");
        assert_eq!(v["prompt_tokens"], 100);
        assert_eq!(v["completion_tokens"], 50);
    }

    #[test]
    fn emit_with_data_no_data_is_identical_to_emit() {
        let dir = tempdir().unwrap();
        let mut obs = BgObserver::new(dir.path());
        obs.emit_with_data("tool_started", "run_command", None);
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(v["kind"], "tool_started");
        assert_eq!(v["msg"], "run_command");
        assert!(v.get("prompt_tokens").is_none(), "no extra fields when data=None");
    }

    #[test]
    fn ndjson_appends_one_valid_json_line_per_emit() {
        let dir = tempdir().unwrap();
        let mut obs = BgObserver::new(dir.path());
        obs.emit("tool_started", "run_command");
        obs.emit("gate", "pass");
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "one line per emit");
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["kind"], "tool_started");
        assert_eq!(v0["msg"], "run_command");
        let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v1["kind"], "gate");
        assert_eq!(v1["msg"], "pass");
    }

    #[test]
    fn start_run_truncates_prior_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".ccd.bg.ndjson"), "stale\n").unwrap();
        let mut obs = BgObserver::start_run(dir.path());
        obs.emit("k", "v");
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        assert!(!body.contains("stale"), "start_run truncates prior content");
        assert_eq!(body.lines().count(), 1);
    }

    #[test]
    fn new_appends_to_existing_stream() {
        // The full event stream must survive across observers within one run:
        // start_run resets, then each `new` observer appends.
        let dir = tempdir().unwrap();
        let mut first = BgObserver::start_run(dir.path());
        first.emit("milestone_start", "#1 a");
        // A second observer (e.g. a later milestone) must NOT truncate the file.
        let mut second = BgObserver::new(dir.path());
        second.emit("milestone_start", "#2 b");
        let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "both events survive: {body}");
        let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["msg"], "#1 a");
        let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v1["msg"], "#2 b");
    }
}
