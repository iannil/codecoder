//! Live observability for headless Background runs (spec 2026-07-25, ADR 0037).
//! Tees each event to stderr (human) and `<root>/.ccd.bg.ndjson` (machine/tail).
use std::io::Write;
use std::path::Path;

pub struct BgObserver {
    ndjson: Option<std::fs::File>,
}

impl BgObserver {
    /// Truncate-create `<root>/.ccd.bg.ndjson`, starting a fresh event stream for
    /// one BG run. Call EXACTLY ONCE at run start; subsequent observers over the
    /// same run use `new` (append). NDJSON is best-effort: if the file can't be
    /// opened, stderr output still happens.
    pub fn start_run(root: &Path) -> Self {
        let path = root.join(".ccd.bg.ndjson");
        let ndjson = std::fs::File::create(&path).ok();
        Self { ndjson }
    }

    /// Open `<root>/.ccd.bg.ndjson` in APPEND mode (create if missing, never
    /// truncate) so the full event stream accumulates across all milestones of a
    /// run. Use `start_run` once at run start to reset the file. NDJSON is
    /// best-effort: if the file can't be opened, stderr output still happens.
    pub fn new(root: &Path) -> Self {
        let path = root.join(".ccd.bg.ndjson");
        let ndjson = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        Self { ndjson }
    }

    /// Emit one event: stderr line + one JSON line to the NDJSON file.
    pub fn emit(&mut self, kind: &str, msg: &str) {
        eprintln!("[bg] {kind}: {msg}");
        if let Some(f) = self.ndjson.as_mut() {
            let line = serde_json::json!({ "kind": kind, "msg": msg }).to_string();
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
