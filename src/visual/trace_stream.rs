//! TraceStream — follows .ccd.trace.ndjson and pushes new lines via SSE.
//! Uses `notify` to watch for file modifications.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// Channel-based file tail follower.
/// Spawns a background thread that watches the trace file for new content.
pub struct TraceStream {
    root: PathBuf,
}

impl TraceStream {
    pub fn new(root: &Path) -> Self {
        TraceStream {
            root: root.to_path_buf(),
        }
    }

    /// Start following the trace file. Returns a receiver that gets new lines.
    /// The receiver will be disconnected when the file is removed or the watcher stops.
    pub fn follow(&self) -> std::io::Result<Receiver<String>> {
        let (tx, rx) = mpsc::channel::<String>();
        let path = self.root.join(".ccd.trace.ndjson");

        std::thread::spawn(move || {
            // Read existing content first
            let mut last_len = 0u64;
            if let Ok(content) = std::fs::read_to_string(&path) {
                last_len = content.len() as u64;
                for line in content.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        if tx.send(line.to_string()).is_err() {
                            return;
                        }
                    }
                }
            }

            // Poll for new content (simpler than inotify/kqueue for cross-platform)
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if let Ok(meta) = std::fs::metadata(&path) {
                    let new_len = meta.len();
                    if new_len > last_len {
                        if let Ok(f) = std::fs::File::open(&path) {
                            use std::io::Read;
                            let mut reader = std::io::BufReader::new(f);
                            // Skip to last_len
                            std::io::copy(
                                &mut reader.by_ref().take(last_len),
                                &mut std::io::sink(),
                            )
                            .ok();
                            let mut line = String::new();
                            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                                let trimmed = line.trim().to_string();
                                if !trimmed.is_empty() {
                                    if tx.send(trimmed).is_err() {
                                        return;
                                    }
                                }
                                line.clear();
                            }
                        }
                        last_len = new_len;
                    }
                } else {
                    // File removed — wait for it to reappear
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn trace_stream_follows_appended_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        // Write initial content
        std::fs::write(&path, "{\"type\":\"meta\",\"version\":1}\n").unwrap();

        let stream = TraceStream::new(dir.path());
        let rx = stream.follow().unwrap();

        // Wait for the initial read to complete and drain it
        std::thread::sleep(Duration::from_millis(100));
        // Drain the initial "meta" line
        let initial = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(initial.contains("meta"));
        // Append new content
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"type\":\"s\",\"span_id\":\"sp_001\"}}").unwrap();
        f.flush().unwrap();

        // Should receive the new line
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(received.contains("sp_001"), "got: {received}");
    }

    #[test]
    fn trace_stream_reads_existing_content_on_start() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".ccd.trace.ndjson");
        std::fs::write(
            &path,
            "{\"type\":\"meta\",\"version\":1}\n{\"type\":\"s\",\"span_id\":\"sp_001\"}\n",
        )
        .unwrap();

        let stream = TraceStream::new(dir.path());
        let rx = stream.follow().unwrap();

        let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(first.contains("meta"));
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(second.contains("sp_001"));
    }
}