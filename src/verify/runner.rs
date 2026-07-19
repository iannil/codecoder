use std::io::BufRead;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use crate::agent::AgentEvent;
use crate::verify::event::*;

/// Control timeout for the test suite.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// A running verify session. Spawns `cargo test` as a subprocess, reads its
/// JSON output line-by-line via a background thread, and forwards parsed events.
pub struct VerifyRunner {
    child: Option<Child>,
    reader: Option<JoinHandle<()>>,
    event_rx: Receiver<()>,
    started_at: Instant,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl VerifyRunner {
    /// Start L1 tests (default). L2/L3 are only included when their gate env
    /// vars are set.
    pub fn start_l1(root: &Path, event_tx: Sender<AgentEvent>) -> Self {
        // Build the test file list: L1 always, L2/L3 gated.
        let mut files: Vec<&str> = TEST_MODULES
            .iter()
            .filter(|(_, _, layer)| *layer == Layer::L1)
            .map(|(file, _, _)| *file)
            .collect();

        // L2: RUN_PTY_SMOKE=1
        if std::env::var("RUN_PTY_SMOKE").is_ok() {
            files.push("l2_pty_smoke");
        }
        // L3: RUN_LLM_SMOKE=1 + CODECODER_API_KEY
        if std::env::var("RUN_LLM_SMOKE").is_ok() && std::env::var("CODECODER_API_KEY").is_ok() {
            files.push("l3_llm_smoke");
        }

        Self::start_tests(&files, root, event_tx)
    }

    /// Start specific test files. Emits `TestSuiteLoaded` first, then spawns
    /// the subprocess and begins reading output.
    pub fn start_tests(files: &[&str], root: &Path, event_tx: Sender<AgentEvent>) -> Self {
        // Emit TestSuiteLoaded — pre-scan all test suites.
        let suites: Vec<SuiteInfo> = files
            .iter()
            .filter_map(|file| {
                TEST_MODULES.iter().find(|(f, _, _)| *f == *file).map(|(f, module, layer)| {
                    SuiteInfo {
                        name: f.to_string(),
                        module: module.to_string(),
                        layer: *layer,
                        // We don't know test_count until we run; mark as 0
                        // and update on the fly.
                        test_count: 0,
                        test_names: Vec::new(),
                    }
                })
            })
            .collect();
        emit_loaded(&event_tx, TestSuiteLoaded { suites });

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let (done_tx, done_rx) = channel();

        // Build the cargo test command.
        // Build args: `cargo test --test <file1> --test <file2> ... -- --format json`
        let mut cmd = Command::new("cargo");
        cmd.arg("test");
        for file in files {
            cmd.arg("--test");
            cmd.arg(file);
        }
        // `--nocapture` so test output isn't swallowed by the test harness.
        cmd.arg("--");
        cmd.arg("--format");
        cmd.arg("json");
        cmd.arg("--nocapture");

        // Run from the project root.
        cmd.current_dir(root);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = if e.kind() == std::io::ErrorKind::NotFound {
                    "cargo not found — is Rust installed?".to_string()
                } else {
                    format!("failed to spawn cargo test: {e}")
                };
                emit_complete(&event_tx, TestSuiteComplete {
                    passed: 0, failed: 0, skipped: 0, total: 0,
                    elapsed_ms: 0, cancelled: false,
                    error: Some(err_msg),
                });
                return Self {
                    child: None,
                    reader: None,
                    event_rx: done_rx,
                    started_at: Instant::now(),
                    cancel,
                };
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let started_at = Instant::now();

        // Background reader thread: read stdout line by line, parse JSON.
        let reader = std::thread::Builder::new()
            .name("verify-reader".into())
            .spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                let mut passed = 0usize;
                let mut failed = 0usize;
                let mut skipped = 0usize;
                let mut total = 0usize;

                for line in reader.lines() {
                    if cancel_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => break,
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    // Parse JSON event from cargo test.
                    if let Some(event) = parse_cargo_test_json(&line) {
                        match event {
                            CargoTestEvent::TestStarted { name, suite } => {
                                emit_progress(&event_tx, TestProgress {
                                    suite: suite.clone(),
                                    case: name.clone(),
                                    status: TestStatus::Running,
                                    output: None,
                                    duration_ms: 0,
                                });
                                total += 1;
                            }
                            CargoTestEvent::TestOk { name, suite, exec_time } => {
                                passed += 1;
                                emit_progress(&event_tx, TestProgress {
                                    suite: suite.clone(),
                                    case: name.clone(),
                                    status: TestStatus::Passed,
                                    output: None,
                                    duration_ms: (exec_time * 1000.0) as u64,
                                });
                            }
                            CargoTestEvent::TestFailed { name, suite, stdout: output, exec_time } => {
                                failed += 1;
                                emit_progress(&event_tx, TestProgress {
                                    suite: suite.clone(),
                                    case: name.clone(),
                                    status: TestStatus::Failed(output.clone().unwrap_or_default()),
                                    output,
                                    duration_ms: (exec_time * 1000.0) as u64,
                                });
                            }
                            CargoTestEvent::TestIgnored { name, suite } => {
                                skipped += 1;
                                emit_progress(&event_tx, TestProgress {
                                    suite: suite.clone(),
                                    case: name.clone(),
                                    status: TestStatus::Skipped,
                                    output: None,
                                    duration_ms: 0,
                                });
                            }
                            CargoTestEvent::SuiteOk { suite, passed: s_passed, failed: s_failed, ignored: s_ignored } => {
                                // Update suite-level counts — re-emit progress for the suite.
                                emit_progress(&event_tx, TestProgress {
                                    suite: suite.clone(),
                                    case: format!("__suite__"),
                                    status: TestStatus::Passed,
                                    output: Some(format!("passed={s_passed} failed={s_failed} ignored={s_ignored}")),
                                    duration_ms: 0,
                                });
                            }
                            CargoTestEvent::SuiteFailed { suite, passed: s_passed, failed: s_failed, ignored: s_ignored } => {
                                emit_progress(&event_tx, TestProgress {
                                    suite: suite.clone(),
                                    case: format!("__suite__"),
                                    status: TestStatus::Failed(String::new()),
                                    output: Some(format!("passed={s_passed} failed={s_failed} ignored={s_ignored}")),
                                    duration_ms: 0,
                                });
                            }
                        }
                    }
                }

                // Drain remaining time.
                let elapsed = started_at.elapsed();

                // Check if cancelled.
                let cancelled = cancel_clone.load(std::sync::atomic::Ordering::Relaxed);

                if !cancelled && passed + failed + skipped == 0 {
                    // No test events at all — something went wrong (e.g. build error).
                    // Try to read stderr for the error message.
                    emit_complete(&event_tx, TestSuiteComplete {
                        passed: 0, failed: 0, skipped: 0, total: 0,
                        elapsed_ms: elapsed.as_millis() as u64,
                        cancelled: false,
                        error: Some("no test output received — cargo test may have failed to build".into()),
                    });
                } else {
                    emit_complete(&event_tx, TestSuiteComplete {
                        passed, failed, skipped, total,
                        elapsed_ms: elapsed.as_millis() as u64,
                        cancelled,
                        error: None,
                    });
                }
                let _ = done_tx.send(());
            })
            .expect("verify reader thread");

        Self {
            child: Some(child),
            reader: Some(reader),
            event_rx: done_rx,
            started_at,
            cancel,
        }
    }

    /// Poll for completion. Returns `Some(())` when the verify session is done,
    /// `None` if still running.
    pub fn poll(&mut self) -> Option<()> {
        match self.event_rx.try_recv() {
            Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.join();
                Some(())
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Check timeout.
                if self.started_at.elapsed() > VERIFY_TIMEOUT {
                    self.cancel();
                    self.join();
                    return Some(());
                }
                None
            }
        }
    }

    /// Cancel the running verify session.
    pub fn cancel(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }
    }

    fn join(&mut self) {
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        if let Some(ref mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}

/// Parsed JSON test event from `cargo test --format json`.
enum CargoTestEvent {
    TestStarted { name: String, suite: String },
    TestOk { name: String, suite: String, exec_time: f64 },
    TestFailed { name: String, suite: String, stdout: Option<String>, exec_time: f64 },
    TestIgnored { name: String, suite: String },
    SuiteOk { suite: String, passed: usize, failed: usize, ignored: usize },
    SuiteFailed { suite: String, passed: usize, failed: usize, ignored: usize },
}

/// Parse a single line of `cargo test --format json` output.
fn parse_cargo_test_json(line: &str) -> Option<CargoTestEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let event_type = v.get("type")?.as_str()?;
    match event_type {
        "test" => {
            let name = v.get("name")?.as_str()?.to_string();
            // Extract suite name from the test name: `l1_tools::read_file_returns_content` → "l1_tools"
            let suite = name.split("::").next().unwrap_or(&name).to_string();
            let event = v.get("event")?.as_str()?;
            let exec_time = v.get("exec_time").and_then(|t| t.as_f64()).unwrap_or(0.0);
            match event {
                "started" => Some(CargoTestEvent::TestStarted { name, suite }),
                "ok" => Some(CargoTestEvent::TestOk { name, suite, exec_time }),
                "failed" => {
                    let stdout = v.get("stdout").and_then(|s| s.as_str()).map(|s| s.to_string());
                    Some(CargoTestEvent::TestFailed { name, suite, stdout, exec_time })
                }
                "ignored" => Some(CargoTestEvent::TestIgnored { name, suite }),
                _ => None,
            }
        }
        "suite" => {
            let suite = v.get("name")?.as_str()?.to_string();
            let event = v.get("event")?.as_str()?;
            let passed = v.get("passed").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            let failed = v.get("failed").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            let ignored = v.get("ignored").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            match event {
                "ok" => Some(CargoTestEvent::SuiteOk { suite, passed, failed, ignored }),
                "failed" => Some(CargoTestEvent::SuiteFailed { suite, passed, failed, ignored }),
                _ => None,
            }
        }
        _ => None,
    }
}
