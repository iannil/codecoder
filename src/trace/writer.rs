//! Dedicated thread that drains TraceEvent channel and writes `.ccd.trace.ndjson`.
//! 10 MB rotation, keep 3 rotated files. Never blocks the AgentLoop.
use crate::trace::types::TraceEvent;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

const ROTATE_SIZE: u64 = 10 * 1024 * 1024;
const MAX_ROTATED: usize = 3;

pub struct TraceWriter {

    ndjson: Option<std::fs::File>,
    ndjson_path: PathBuf,
    bytes_written: u64,
    initial_metadata_written: bool,
}

impl TraceWriter {
    /// Spawn a dedicated thread for writing trace events. Returns the channel Sender.
    /// File: `<root>/.ccd.trace.ndjson`
    pub fn spawn(root: &Path) -> Sender<TraceEvent> {
        let (tx, rx) = std::sync::mpsc::channel::<TraceEvent>();
        let path = root.join(".ccd.trace.ndjson");
        std::thread::spawn(move || {
            let mut writer = TraceWriter {
                ndjson: None,
                ndjson_path: path,
                bytes_written: 0,
                initial_metadata_written: false,
            };
            writer.run(rx);
        });
        tx
    }

    fn run(&mut self, rx: Receiver<TraceEvent>) {
        for event in rx {
            if !self.initial_metadata_written {
                self.initial_metadata_written = true;
                let header = serde_json::json!({
                    "type": "meta",
                    "version": 1,
                    "ts": crate::trace::types::now_ts(),
                    "pid": std::process::id(),
                });
                self.write_line(&header.to_string());
            }
            self.maybe_rotate();
            let json = match serde_json::to_string(&event) {
                Ok(j) => j,
                Err(e) => format!("{{\"type\":\"error\",\"msg\":\"serialize failed: {e}\"}}"),
            };
            self.write_line(&json);
        }
    }

    fn write_line(&mut self, line: &str) {
        let file = self.ndjson.get_or_insert_with(|| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.ndjson_path)
                .expect("failed to open trace file")
        });
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
        self.bytes_written += line.len() as u64 + 1;
    }

    fn maybe_rotate(&mut self) {
        if self.bytes_written < ROTATE_SIZE {
            return;
        }
        rotate_ndjson(&self.ndjson_path);
        self.ndjson = Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.ndjson_path)
                .expect("failed to reopen trace file after rotation"),
        );
        self.bytes_written = 0;
    }
}

fn rotate_ndjson(path: &Path) {
    let dir = path.parent().unwrap_or(Path::new("."));
    for i in (MAX_ROTATED..100).rev() {
        let old = dir.join(format!(".ccd.trace.{i}.ndjson"));
        let _ = std::fs::remove_file(&old);
    }
    for i in (1..MAX_ROTATED).rev() {
        let src = dir.join(format!(".ccd.trace.{i}.ndjson"));
        let dst = dir.join(format!(".ccd.trace.{}.ndjson", i + 1));
        let _ = std::fs::rename(&src, &dst);
    }
    let rotated = dir.join(".ccd.trace.1.ndjson");
    let _ = std::fs::rename(path, &rotated);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writer_creates_file_and_writes_meta() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let _ = tx.send(TraceEvent::span_start("sp_001".into(), None, crate::trace::types::SpanKind::Turn, serde_json::json!({})));
        drop(tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2, "meta + at least 1 event: {}", body);
        let meta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(meta["type"], "meta");
        assert_eq!(meta["version"], 1);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "s");
        assert_eq!(ev["span_id"], "sp_001");
    }

    #[test]
    fn writer_handles_span_end() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let _ = tx.send(TraceEvent::span_end("sp_001".into(), serde_json::json!({"duration_ms": 100})));
        drop(tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines.len() >= 2);
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "e");
        assert_eq!(ev["meta"]["duration_ms"], 100);
    }

    #[test]
    fn writer_handles_point_event() {
        let dir = tempdir().unwrap();
        let tx = TraceWriter::spawn(dir.path());
        let _ = tx.send(TraceEvent::point(
            crate::trace::types::EventKind::Notice { text: "hello".into() },
            serde_json::json!({}),
        ));
        drop(tx);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let body = std::fs::read_to_string(dir.path().join(".ccd.trace.ndjson")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        let ev: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev["type"], "p");
        // EventKind is serialized as adjacently-tagged JSON object
        assert!(ev["kind"].is_object(), "kind should be an object, got: {:?}", ev["kind"]);
    }
}