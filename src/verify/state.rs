use std::time::Instant;

use crate::verify::event::*;

/// TUI-side state tree for the verify dashboard.
#[derive(Debug, Clone)]
pub struct VerifyState {
    pub layers: [LayerState; 3],
    pub focus: VerifyFocus,
    pub running: bool,
    pub started_at: Instant,
    pub total_tests: usize,
    pub completed: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub elapsed_ms: u64,
    pub error: Option<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct LayerState {
    pub name: &'static str,
    pub modules: Vec<ModuleState>,
    pub folded: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Which layer (0=L1, 1=L2, 2=L3)
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct ModuleState {
    pub name: String,
    pub cases: Vec<CaseState>,
    pub folded: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub running: usize,
}

#[derive(Debug, Clone)]
pub struct CaseState {
    pub name: String,
    pub status: CaseStatus,
    pub output: Vec<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseStatus {
    Queued,
    Running,
    Passed,
    Failed(String),
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyFocus {
    None,
    Layer(usize),
    Module { layer: usize, module: usize },
    Case { layer: usize, module: usize, case: usize },
}

impl VerifyState {
    pub fn new() -> Self {
        Self {
            layers: [
                LayerState { name: "L1 主干 (hermetic)", modules: Vec::new(), folded: false, passed: 0, failed: 0, skipped: 0, index: 0 },
                LayerState { name: "L2 pty 冒烟", modules: Vec::new(), folded: true, passed: 0, failed: 0, skipped: 0, index: 1 },
                LayerState { name: "L3 真实 LLM", modules: Vec::new(), folded: true, passed: 0, failed: 0, skipped: 0, index: 2 },
            ],
            focus: VerifyFocus::None,
            running: false,
            started_at: Instant::now(),
            total_tests: 0,
            completed: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            elapsed_ms: 0,
            error: None,
            cancelled: false,
        }
    }

    /// Handle `TestSuiteLoaded` event — build module/case state tree.
    pub fn apply_loaded(&mut self, loaded: &TestSuiteLoaded) {
        for suite in &loaded.suites {
            let layer_idx = match suite.layer {
                Layer::L1 => 0,
                Layer::L2 => 1,
                Layer::L3 => 2,
            };
            let layer = &mut self.layers[layer_idx];

            // Check if module already exists.
            if !layer.modules.iter().any(|m| m.name == suite.module) {
                layer.modules.push(ModuleState {
                    name: suite.module.clone(),
                    cases: Vec::new(),
                    folded: true,
                    passed: 0,
                    failed: 0,
                    skipped: 0,
                    running: 0,
                });
            }
        }
        self.running = true;
        self.started_at = Instant::now();
    }

    /// Handle `TestProgress` event — update the case state.
    pub fn apply_progress(&mut self, progress: &TestProgress) {
        let layer_idx = if progress.suite.starts_with("l1_") {
            0
        } else if progress.suite.starts_with("l2_") {
            1
        } else {
            2
        };

        // Find the module for this suite.
        let module_name = progress.suite.split("::").next().unwrap_or(&progress.suite).to_string();
        let layer = &mut self.layers[layer_idx];

        // Check if this is a suite-level event (__suite__ marker).
        if progress.case == "__suite__" {
            return;
        }

        // Find or create module.
        let module = if let Some(m) = layer.modules.iter_mut().find(|m| m.name == module_name) {
            m
        } else {
            // Create a module entry for tests not in TEST_MODULES (e.g. from testkit_compiles).
            layer.modules.push(ModuleState {
                name: module_name.clone(),
                cases: Vec::new(),
                folded: true,
                passed: 0,
                failed: 0,
                skipped: 0,
                running: 0,
            });
            layer.modules.last_mut().unwrap()
        };

        // Find or create case.
        let case = if let Some(c) = module.cases.iter_mut().find(|c| c.name == progress.case) {
            c
        } else {
            module.cases.push(CaseState {
                name: progress.case.clone(),
                status: CaseStatus::Queued,
                output: Vec::new(),
                duration_ms: 0,
            });
            self.total_tests += 1;
            module.cases.last_mut().unwrap()
        };

        case.duration_ms = progress.duration_ms;

        match &progress.status {
            TestStatus::Queued => {
                case.status = CaseStatus::Queued;
            }
            TestStatus::Running => {
                case.status = CaseStatus::Running;
                module.running += 1;
            }
            TestStatus::Passed => {
                case.status = CaseStatus::Passed;
                module.passed += 1;
                layer.passed += 1;
                self.passed += 1;
                self.completed += 1;
            }
            TestStatus::Failed(reason) => {
                case.status = CaseStatus::Failed(reason.clone());
                module.failed += 1;
                layer.failed += 1;
                self.failed += 1;
                self.completed += 1;
                if let Some(output) = &progress.output {
                    case.output = output.lines().map(|l| l.to_string()).collect();
                }
            }
            TestStatus::Skipped => {
                case.status = CaseStatus::Skipped;
                module.skipped += 1;
                layer.skipped += 1;
                self.skipped += 1;
                self.completed += 1;
            }
        }
    }

    /// Handle `TestSuiteComplete` event.
    pub fn apply_complete(&mut self, complete: &TestSuiteComplete) {
        self.running = false;
        self.elapsed_ms = complete.elapsed_ms;
        self.error = complete.error.clone();
        self.cancelled = complete.cancelled;
    }

    /// Reset state for a new verify run.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Percentage of completed tests.
    pub fn pct(&self) -> u8 {
        if self.total_tests == 0 {
            return 0;
        }
        ((self.completed * 100) / self.total_tests) as u8
    }
}