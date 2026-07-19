// src/verify/scenario.rs
// 骨架场景定义框架 (L4 阶段 1)

/// 场景类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioCategory {
    Tool,
    Permission,
    AgentFlow,
    Session,
    Capability,
    Skill,
    Meta,
}

impl ScenarioCategory {
    pub fn name(&self) -> &'static str {
        match self {
            ScenarioCategory::Tool => "工具",
            ScenarioCategory::Permission => "权限",
            ScenarioCategory::AgentFlow => "对话流程",
            ScenarioCategory::Session => "Session",
            ScenarioCategory::Capability => "能力",
            ScenarioCategory::Skill => "Skill",
            ScenarioCategory::Meta => "自检",
        }
    }
}

/// 场景执行状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioStatus {
    Queued,
    Running,
    Passed,
    Failed(String),
    Skipped,
}

/// 文件系统断言
#[derive(Debug, Clone)]
pub enum FilePredicate {
    Exists,
    NotExists,
    Contains(&'static str),
    NotContains(&'static str),
    LineCount(usize),
}

/// 场景步骤
#[derive(Debug, Clone)]
pub enum ScenarioStep {
    /// 向 agent 提交一条用户消息
    SubmitMessage(&'static str),
    /// 期望收到 ToolStarted 事件，匹配工具名
    ExpectToolStarted(&'static str),
    /// 期望收到 ToolFinished 事件，匹配工具名，可选断言非错误
    ExpectToolFinished { name: &'static str, expect_ok: bool },
    /// 期望收到 StreamDelta，包含某文本
    ExpectStreamContains(&'static str),
    /// 断言文件系统状态
    AssertFile { path: &'static str, predicate: FilePredicate },
    /// 等待 N ms
    Wait(u64),
}

/// 场景定义
#[derive(Debug, Clone)]
pub struct VerifyScenario {
    pub name: &'static str,
    pub category: ScenarioCategory,
    /// true = 失败即停止（核心工具错误）
    pub critical: bool,
    pub steps: Vec<ScenarioStep>,
}

/// 场景运行时状态
#[derive(Debug, Clone)]
pub struct ScenarioState {
    pub name: String,
    pub category: ScenarioCategory,
    pub critical: bool,
    pub status: ScenarioStatus,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl ScenarioState {
    pub fn new(scenario: &VerifyScenario) -> Self {
        Self {
            name: scenario.name.to_string(),
            category: scenario.category,
            critical: scenario.critical,
            status: ScenarioStatus::Queued,
            error: None,
            duration_ms: 0,
        }
    }
}

/// 所有骨架场景的清单
pub fn all_scenarios() -> Vec<VerifyScenario> {
    vec![
        // ===== 工具场景 (critical = true) =====
        tool_scenario("read_file_returns_content", "read_file", "read src/lib.rs", true),
        tool_scenario("write_file_creates_file", "write_file", "write hello.txt with 'hello world'", true),
        tool_scenario("list_directory_shows_entries", "list_directory", "list files in src/", true),
        tool_scenario("run_command_executes", "run_command", "run echo hello-verify", true),
        tool_scenario("glob_finds_files", "glob", "find all .rs files in src/", true),
        tool_scenario("grep_finds_pattern", "grep", "search for 'fn main' in src/", true),
        tool_scenario("diff_shows_changes", "diff", "show git diff", true),
        tool_scenario("use_skill_loads", "use_skill", "use skill debug-causal", false),
        tool_scenario("agent_spawns_subagent", "agent", "agent: read Cargo.toml", true),
        tool_scenario("memory_stores_and_recalls", "memory", "remember: test-key = test-value", false),
        // edit_file 需要先 write 再 edit
        VerifyScenario {
            name: "edit_file_modifies_content",
            category: ScenarioCategory::Tool,
            critical: true,
            steps: vec![
                ScenarioStep::SubmitMessage("write /tmp/cc-edit-test.txt with 'alpha beta'"),
                ScenarioStep::ExpectToolStarted("write_file"),
                ScenarioStep::Wait(100),
                ScenarioStep::SubmitMessage("edit /tmp/cc-edit-test.txt, replace 'beta' with 'gamma'"),
                ScenarioStep::ExpectToolStarted("edit_file"),
                ScenarioStep::Wait(100),
            ],
        },
        // ===== 权限场景 (critical = true) =====
        VerifyScenario {
            name: "grant_once_allows_one_call",
            category: ScenarioCategory::Permission,
            critical: true,
            steps: vec![
                ScenarioStep::SubmitMessage("write /tmp/cc-perm-test.txt with 'perm'"),
                ScenarioStep::ExpectToolStarted("write_file"),
                ScenarioStep::Wait(100),
            ],
        },
        // ===== Agent 对话流程场景 (critical = false) =====
        VerifyScenario {
            name: "cancel_interrupts_turn",
            category: ScenarioCategory::AgentFlow,
            critical: false,
            steps: vec![
                ScenarioStep::SubmitMessage("run sleep 10"),
                ScenarioStep::ExpectToolStarted("run_command"),
                ScenarioStep::Wait(50),
                // 注意：取消由外部的 cancel token 触发，这里只验证启动
            ],
        },
        // ===== Session 场景 (critical = false) =====
        VerifyScenario {
            name: "session_persists_to_disk",
            category: ScenarioCategory::Session,
            critical: false,
            steps: vec![
                ScenarioStep::SubmitMessage("hello, this is a session test"),
                ScenarioStep::ExpectStreamContains("hello"),
                ScenarioStep::Wait(100),
            ],
        },
        // ===== Meta 场景 (critical = false) =====
        VerifyScenario {
            name: "readme_allows_without_gh_token",
            category: ScenarioCategory::Meta,
            critical: false,
            steps: vec![
                ScenarioStep::SubmitMessage("read README.md"),
                ScenarioStep::ExpectToolStarted("read_file"),
                ScenarioStep::Wait(100),
            ],
        },
    ]
}

fn tool_scenario(name: &'static str, tool: &'static str, msg: &'static str, critical: bool) -> VerifyScenario {
    VerifyScenario {
        name,
        category: ScenarioCategory::Tool,
        critical,
        steps: vec![
            ScenarioStep::SubmitMessage(msg),
            ScenarioStep::ExpectToolStarted(tool),
            ScenarioStep::Wait(100),
        ],
    }
}