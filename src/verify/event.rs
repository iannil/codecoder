use std::sync::mpsc::Sender;

use crate::agent::AgentEvent;

/// Mapping from test file → module name for dashboard display.
pub const TEST_MODULES: &[(&str, &str, Layer)] = &[
    ("l1_kernel", "kernel", Layer::L1),
    ("l1_tools", "tools", Layer::L1),
    ("l1_self_evolution", "self-evolution", Layer::L1),
    ("l1_permission", "permission", Layer::L1),
    ("l1_session", "session", Layer::L1),
    ("l1_subagent", "subagent", Layer::L1),
    ("l1_background", "background", Layer::L1),
    ("l1_interaction", "interaction", Layer::L1),
    ("l1_compaction", "compaction", Layer::L1),
    ("l2_pty_smoke", "pty-smoke", Layer::L2),
    ("l3_llm_smoke", "llm-smoke", Layer::L3),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    L1,
    L2,
    L3,
}

impl Layer {
    pub fn name(&self) -> &'static str {
        match self {
            Layer::L1 => "L1 主干 (hermetic)",
            Layer::L2 => "L2 pty 冒烟",
            Layer::L3 => "L3 真实 LLM",
        }
    }
}

/// Status of a single test case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestStatus {
    Queued,
    Running,
    Passed,
    Failed(String),
    Skipped,
}

/// Info about a test suite (one test file).
#[derive(Debug, Clone)]
pub struct SuiteInfo {
    pub name: String,
    pub module: String,
    pub layer: Layer,
    pub test_count: usize,
    pub test_names: Vec<String>,
}

/// Progress for one test case.
#[derive(Debug, Clone)]
pub struct TestProgress {
    pub suite: String,
    pub case: String,
    pub status: TestStatus,
    pub output: Option<String>,
    pub duration_ms: u64,
}

/// Loaded test suite metadata.
#[derive(Debug, Clone)]
pub struct TestSuiteLoaded {
    pub suites: Vec<SuiteInfo>,
}

/// Completion summary.
#[derive(Debug, Clone)]
pub struct TestSuiteComplete {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total: usize,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    pub error: Option<String>,
}

/// Emit the `TestSuiteLoaded` event.
pub fn emit_loaded(event_tx: &Sender<AgentEvent>, loaded: TestSuiteLoaded) {
    let _ = event_tx.send(AgentEvent::TestSuiteLoaded(loaded));
}

/// Emit a `TestProgress` event.
pub fn emit_progress(event_tx: &Sender<AgentEvent>, progress: TestProgress) {
    let _ = event_tx.send(AgentEvent::TestProgress(progress));
}

/// Emit the `TestSuiteComplete` event.
pub fn emit_complete(event_tx: &Sender<AgentEvent>, complete: TestSuiteComplete) {
    let _ = event_tx.send(AgentEvent::TestSuiteComplete(complete));
}