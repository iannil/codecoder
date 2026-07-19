// tests/l1_l4_verify.rs
// L4 验证框架测试 (hermetic, 使用 stub provider)

use codecoder::verify::scenario::{ScenarioCategory, ScenarioStatus, ScenarioState, VerifyScenario, all_scenarios, FilePredicate, ScenarioStep};
use codecoder::verify::explore::ExploreState;
use codecoder::verify::event::{L4Phase, L4ScenarioProgress, L4ExploreProgress};
use codecoder::verify::state::{L4State, VerifyState};

// ============================================================
// 场景定义加载与验证
// ============================================================

#[test]
fn l4_scenarios_load_and_validate() {
    // 验证场景定义加载正常
    let scenarios = all_scenarios();
    assert!(!scenarios.is_empty(), "场景列表不应为空");

    // 验证每个场景有名称和步骤
    for s in &scenarios {
        assert!(!s.name.is_empty(), "场景名称不应为空");
        assert!(!s.steps.is_empty(), "场景 {} 应有步骤", s.name);
    }

    // 验证工具场景覆盖了所有工具
    let tool_scenarios: Vec<&str> = scenarios.iter()
        .filter(|s| s.category == ScenarioCategory::Tool)
        .map(|s| s.name)
        .collect();
    assert!(!tool_scenarios.is_empty(), "应有工具场景");
    assert!(tool_scenarios.contains(&"read_file_returns_content"), "应有 read_file 场景");
    assert!(tool_scenarios.contains(&"write_file_creates_file"), "应有 write_file 场景");
    assert!(tool_scenarios.contains(&"run_command_executes"), "应有 run_command 场景");
}

#[test]
fn l4_scenarios_has_all_categories() {
    let scenarios = all_scenarios();
    let mut categories: Vec<ScenarioCategory> =
        scenarios.iter().map(|s| s.category).collect();
    categories.sort_by_key(|c| c.name());
    categories.dedup();

    assert!(categories.contains(&ScenarioCategory::Tool), "应有 Tool 类别");
    assert!(categories.contains(&ScenarioCategory::Permission), "应有 Permission 类别");
    assert!(categories.contains(&ScenarioCategory::AgentFlow), "应有 AgentFlow 类别");
    assert!(categories.contains(&ScenarioCategory::Session), "应有 Session 类别");
    assert!(categories.contains(&ScenarioCategory::Meta), "应有 Meta 类别");
}

#[test]
fn l4_scenario_step_variants_constructible() {
    // 验证所有步骤变体可以构造
    let _msg = ScenarioStep::SubmitMessage("hello");
    let _started = ScenarioStep::ExpectToolStarted("read_file");
    let _finished = ScenarioStep::ExpectToolFinished { name: "read_file", expect_ok: true };
    let _stream = ScenarioStep::ExpectStreamContains("hello");
    let _file_assert = ScenarioStep::AssertFile {
        path: "/tmp/test.txt",
        predicate: FilePredicate::Exists,
    };
    let _wait = ScenarioStep::Wait(50);

    // 验证 Contains 和 NotContains 变体
    let _contains = ScenarioStep::AssertFile {
        path: "/tmp/test.txt",
        predicate: FilePredicate::Contains("hello"),
    };
    let _not_contains = ScenarioStep::AssertFile {
        path: "/tmp/test.txt",
        predicate: FilePredicate::NotContains("world"),
    };
    let _not_exists = ScenarioStep::AssertFile {
        path: "/tmp/test.txt",
        predicate: FilePredicate::NotExists,
    };
    let _line_count = ScenarioStep::AssertFile {
        path: "/tmp/test.txt",
        predicate: FilePredicate::LineCount(5),
    };
}

// ============================================================
// ExploreState 初始化与计数
// ============================================================

#[test]
fn l4_explore_state_initializes() {
    let state = ExploreState::new();
    assert_eq!(state.checked_count(), 0);
    assert_eq!(state.failed_count(), 0);
    assert_eq!(state.healed_count(), 0);
    assert!(!state.running);
    assert!(state.current_target.is_none());
    assert!(state.checked_skills.is_empty());
    assert!(state.checked_capabilities.is_empty());
    assert!(state.healed.is_empty());
    assert!(state.failed.is_empty());
}

#[test]
fn l4_explore_checked_count_aggregates() {
    let mut state = ExploreState::new();
    assert_eq!(state.checked_count(), 0);

    state.checked_skills.push("skills/debug-causal.md".into());
    assert_eq!(state.checked_count(), 1);

    state.checked_capabilities.push("capabilities/build.sh".into());
    assert_eq!(state.checked_count(), 2);
}

#[test]
fn l4_explore_healed_count_filters_applied() {
    use codecoder::verify::explore::HealRecord;

    let mut state = ExploreState::new();
    assert_eq!(state.healed_count(), 0);

    state.healed.push(HealRecord {
        target: "skills/debug-causal.md".into(),
        diagnosis: "fixed typo".into(),
        applied: true,
        diff: String::new(),
    });
    assert_eq!(state.healed_count(), 1);

    state.healed.push(HealRecord {
        target: "skills/skill2.md".into(),
        diagnosis: "could not fix".into(),
        applied: false,
        diff: String::new(),
    });
    // Only applied records count
    assert_eq!(state.healed_count(), 1);
}

// ============================================================
// L4State 基础行为
// ============================================================

#[test]
fn l4_state_initializes() {
    let l4 = L4State::new();
    assert_eq!(l4.phase, L4Phase::Idle);
    assert!(l4.folded);
    assert!(l4.scenarios.is_empty());
    assert_eq!(l4.total_scenarios(), 0);
    assert_eq!(l4.completed_scenarios(), 0);
    assert_eq!(l4.passed_scenarios(), 0);
    assert_eq!(l4.failed_scenarios(), 0);
}

#[test]
fn l4_verify_state_integration() {
    let mut vstate = VerifyState::new();
    // L4 初始状态
    assert_eq!(vstate.l4.phase, L4Phase::Idle);
    assert!(vstate.l4.folded);

    // 加载场景
    vstate.l4.load_scenarios();
    assert!(vstate.l4.total_scenarios() > 0);
    assert_eq!(vstate.l4.phase, L4Phase::Scenarios);
}

#[test]
fn l4_load_scenarios_populates_states() {
    let mut l4 = L4State::new();
    l4.load_scenarios();

    let total = l4.total_scenarios();
    assert!(total > 0, "应加载至少一个场景");

    // 所有场景初始为 Queued
    for s in &l4.scenarios {
        assert_eq!(s.status, ScenarioStatus::Queued);
        assert!(s.error.is_none());
        assert_eq!(s.duration_ms, 0);
    }
}

// ============================================================
// L4ScenarioProgress 应用
// ============================================================

#[test]
fn l4_scenario_progress_apply() {
    let mut vstate = VerifyState::new();
    vstate.l4.load_scenarios();

    // 模拟场景进度更新
    let progress = L4ScenarioProgress {
        name: vstate.l4.scenarios[0].name.clone(),
        category: "工具",
        critical: true,
        status: ScenarioStatus::Passed,
        output: None,
        duration_ms: 100,
    };
    vstate.l4.apply_l4_scenario(&progress);

    assert_eq!(vstate.l4.passed_scenarios(), 1);
    assert_eq!(vstate.l4.completed_scenarios(), 1);
    assert_eq!(vstate.l4.failed_scenarios(), 0);
}

#[test]
fn l4_scenario_progress_failed() {
    let mut vstate = VerifyState::new();
    vstate.l4.load_scenarios();

    let progress = L4ScenarioProgress {
        name: vstate.l4.scenarios[0].name.clone(),
        category: "工具",
        critical: true,
        status: ScenarioStatus::Failed("timeout".into()),
        output: Some("tool did not respond".into()),
        duration_ms: 5000,
    };
    vstate.l4.apply_l4_scenario(&progress);

    assert_eq!(vstate.l4.passed_scenarios(), 0);
    assert_eq!(vstate.l4.failed_scenarios(), 1);
    assert_eq!(vstate.l4.completed_scenarios(), 1);

    let state = &vstate.l4.scenarios[0];
    assert_eq!(state.error, Some("timeout".into()));
    assert_eq!(state.duration_ms, 5000);
}

#[test]
fn l4_scenario_progress_skipped() {
    let mut vstate = VerifyState::new();
    vstate.l4.load_scenarios();

    let progress = L4ScenarioProgress {
        name: vstate.l4.scenarios[0].name.clone(),
        category: "工具",
        critical: false,
        status: ScenarioStatus::Skipped,
        output: None,
        duration_ms: 0,
    };
    vstate.l4.apply_l4_scenario(&progress);

    assert_eq!(vstate.l4.passed_scenarios(), 0);
    assert_eq!(vstate.l4.failed_scenarios(), 0);
    assert_eq!(vstate.l4.completed_scenarios(), 1);
}

#[allow(clippy::borrow_interior_mutable_const)]
#[test]
fn l4_scenario_progress_multiple() {
    let mut l4 = L4State::new();
    l4.load_scenarios();
    let total = l4.total_scenarios();

    // Collect names first to avoid borrow conflict
    let names: Vec<String> = l4.scenarios.iter().map(|s| s.name.clone()).collect();

    // Mark first half as passed, second half as failed
    let mid = total / 2;
    for (i, name) in names.iter().enumerate() {
        let progress = L4ScenarioProgress {
            name: name.clone(),
            category: "test",
            critical: i < mid,
            status: if i < mid { ScenarioStatus::Passed } else { ScenarioStatus::Failed("error".into()) },
            output: None,
            duration_ms: 10,
        };
        l4.apply_l4_scenario(&progress);
    }

    assert_eq!(l4.passed_scenarios(), mid);
    assert_eq!(l4.failed_scenarios(), total - mid);
    assert_eq!(l4.completed_scenarios(), total);
}

// ============================================================
// L4ExploreProgress 应用
// ============================================================

#[test]
fn l4_explore_progress_apply() {
    let mut vstate = VerifyState::new();
    vstate.l4.load_scenarios();

    // 模拟探索进度
    let progress = L4ExploreProgress {
        target: "skills/debug-causal".into(),
        status: "checking",
        detail: None,
    };
    vstate.l4.apply_l4_explore(&progress);
    assert_eq!(vstate.l4.explore.current_target, Some("skills/debug-causal".into()));

    let ok_progress = L4ExploreProgress {
        target: "skills/debug-causal".into(),
        status: "ok",
        detail: None,
    };
    vstate.l4.apply_l4_explore(&ok_progress);
    assert_eq!(vstate.l4.explore.checked_skills.len(), 1);
    assert!(vstate.l4.explore.current_target.is_none());
}

#[test]
fn l4_explore_fixed_updates_healed() {
    let mut vstate = VerifyState::new();
    vstate.l4.load_scenarios();

    let progress = L4ExploreProgress {
        target: "skills/broken-skill".into(),
        status: "fixed",
        detail: Some("replaced deprecated API call".into()),
    };
    vstate.l4.apply_l4_explore(&progress);

    assert_eq!(vstate.l4.explore.healed.len(), 1);
    assert_eq!(vstate.l4.explore.healed[0].target, "skills/broken-skill");
    assert!(vstate.l4.explore.healed[0].applied);
    assert_eq!(vstate.l4.explore.healed[0].diagnosis, "replaced deprecated API call");
    assert!(vstate.l4.explore.current_target.is_none());
}

#[test]
fn l4_explore_failed_updates_failed_list() {
    let mut vstate = VerifyState::new();
    vstate.l4.load_scenarios();

    let progress = L4ExploreProgress {
        target: "skills/unfixable".into(),
        status: "failed",
        detail: Some("cannot parse file".into()),
    };
    vstate.l4.apply_l4_explore(&progress);

    assert_eq!(vstate.l4.explore.failed.len(), 1);
    assert_eq!(vstate.l4.explore.failed[0], "skills/unfixable");
    assert!(vstate.l4.explore.current_target.is_none());
}

#[test]
fn l4_explore_capability_routing() {
    let mut l4 = L4State::new();

    // Skills ending with .md or containing "skill" go to checked_skills
    let skill_progress = L4ExploreProgress {
        target: "skills/debug-causal.md".into(),
        status: "ok",
        detail: None,
    };
    l4.apply_l4_explore(&skill_progress);
    assert_eq!(l4.explore.checked_skills.len(), 1);
    assert_eq!(l4.explore.checked_capabilities.len(), 0);

    // Capabilities go to checked_capabilities
    let cap_progress = L4ExploreProgress {
        target: "capabilities/build.sh".into(),
        status: "ok",
        detail: None,
    };
    l4.apply_l4_explore(&cap_progress);
    assert_eq!(l4.explore.checked_skills.len(), 1);
    assert_eq!(l4.explore.checked_capabilities.len(), 1);
}

// ============================================================
// ScenarioState 构造
// ============================================================

#[test]
fn l4_scenario_state_from_verify() {
    let scenario = VerifyScenario {
        name: "test_scenario",
        category: ScenarioCategory::Tool,
        critical: true,
        steps: vec![ScenarioStep::SubmitMessage("hello")],
    };

    let state = ScenarioState::new(&scenario);
    assert_eq!(state.name, "test_scenario");
    assert_eq!(state.category, ScenarioCategory::Tool);
    assert!(state.critical);
    assert_eq!(state.status, ScenarioStatus::Queued);
    assert!(state.error.is_none());
    assert_eq!(state.duration_ms, 0);
}

// ============================================================
// 场景存在性验证
// ============================================================

#[test]
fn l4_runner_creates_scenarios() {
    let scenarios = all_scenarios();
    // 验证至少有一个工具场景、一个权限场景
    let has_tool = scenarios.iter().any(|s| s.category == ScenarioCategory::Tool);
    let has_perm = scenarios.iter().any(|s| s.category == ScenarioCategory::Permission);
    assert!(has_tool, "应有工具场景");
    assert!(has_perm, "应有权限场景");
}

#[test]
fn l4_critical_scenarios_exist() {
    let scenarios = all_scenarios();
    let critical: Vec<&str> = scenarios.iter()
        .filter(|s| s.critical)
        .map(|s| s.name)
        .collect();

    // Core file tools should be critical
    assert!(critical.contains(&"read_file_returns_content"), "read_file 应为 critical");
    assert!(critical.contains(&"write_file_creates_file"), "write_file 应为 critical");
    assert!(critical.contains(&"run_command_executes"), "run_command 应为 critical");
    assert!(critical.contains(&"edit_file_modifies_content"), "edit_file 应为 critical");
    assert!(critical.contains(&"grant_once_allows_one_call"), "grant_once 应为 critical");

    // Non-critical tools
    assert!(!critical.contains(&"use_skill_loads"), "use_skill 不应为 critical");
    assert!(!critical.contains(&"memory_stores_and_recalls"), "memory 不应为 critical");
}

// ============================================================
// L4Phase 名称验证
// ============================================================

#[test]
fn l4_phase_names() {
    assert_eq!(L4Phase::Idle.name(), "空闲");
    assert_eq!(L4Phase::Scenarios.name(), "骨架场景");
    assert_eq!(L4Phase::Exploration.name(), "自驱动探索");
    assert_eq!(L4Phase::Complete.name(), "完成");
    assert_eq!(L4Phase::Failed.name(), "失败");
}

// ============================================================
// ScenarioCategory 名称验证
// ============================================================

#[test]
fn l4_scenario_category_names() {
    assert_eq!(ScenarioCategory::Tool.name(), "工具");
    assert_eq!(ScenarioCategory::Permission.name(), "权限");
    assert_eq!(ScenarioCategory::AgentFlow.name(), "对话流程");
    assert_eq!(ScenarioCategory::Session.name(), "Session");
    assert_eq!(ScenarioCategory::Capability.name(), "能力");
    assert_eq!(ScenarioCategory::Skill.name(), "Skill");
    assert_eq!(ScenarioCategory::Meta.name(), "自检");
}