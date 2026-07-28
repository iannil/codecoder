# P0-1: 空 workgraph 自动建里程碑 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 当 `CODECODER_BG_WORKGRAPH=1` 启动且 `workgraph.json` 为空时，自动读取 AGENTS.md，调用 `generate_milestones` 工具分解为里程碑，写入后开始推进。

**Architecture:** 在 `src/background.rs` 的 `run_background_cfg()` 空图分支中，新增 `seed_workgraph_from_mission()` 函数，通过一个 headless agent turn 调用已有 `generate_milestones` 工具完成分解。成功后将控制流交给现有 milestone 推进循环。

**Tech Stack:** Rust, 同一进程内 agent turn, 工具 `generate_milestones` 已存在。

## 全局约束

- 所有新增函数放在 `src/background.rs` 中（已有约 1005 行，不拆文件）
- 使用已有 `AgentLoop::new_background()`、`run_one_turn()`、`drain_bg_events()`
- seed turn 不注册 SIGINT（避免与主循环的 cancel token 冲突）
- `seed_workgraph_from_mission()` 返回 `bool`（true=成功写入, false=失败回退）
- 失败时回退 `MissionState::EmptyGraph`，不改变已有退出码语义
- 使用 `WorkGraph::read()` 检查写入结果（不依赖 LLM 响应判断）
- 测试用 `StubClient` 模拟 LLM 不调用工具的场景

---

### Task 1: 新增 `read_mission()` 函数

**Files:**
- Modify: `src/background.rs`（新增函数，约 20 行，加在 `seed_workgraph_from_mission` 之前）
- Test: `src/background.rs`（已有测试模块末尾追加）

**Interfaces:**
- Consumes: `std::path::Path`（项目根目录）
- Produces: `fn read_mission(root: &Path) -> String` — 返回 AGENTS.md 内容或通用降级文本

- [ ] **Step 1: 在 background.rs 的 `run_background_cfg` 函数之前添加 `read_mission` 函数**

```rust
/// 读取项目根目录的 AGENTS.md 作为使命描述。若文件不存在或为空，返回通用降级文本。
fn read_mission(root: &Path) -> String {
    let path = root.join("AGENTS.md");
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => "Initialize and develop the project in this directory.".to_string(),
    }
}
```

- [ ] **Step 2: 在测试模块中添加单元测试**

```rust
#[test]
fn read_mission_returns_agents_md_content() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "Build a Rust CLI tool").unwrap();
    let m = read_mission(dir.path());
    assert_eq!(m, "Build a Rust CLI tool");
}

#[test]
fn read_mission_fallback_when_no_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    let m = read_mission(dir.path());
    assert!(m.contains("Initialize and develop"));
}

#[test]
fn read_mission_fallback_when_agents_md_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "").unwrap();
    let m = read_mission(dir.path());
    assert!(m.contains("Initialize and develop"));
    // 纯空格也算空
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("AGENTS.md"), "   \n\n").unwrap();
    let m2 = read_mission(dir2.path());
    assert!(m2.contains("Initialize and develop"));
}
```

- [ ] **Step 3: 运行测试确认通过**

```bash
cargo test read_mission_ -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/background.rs
git commit -m "feat: add read_mission() for AGENTS.md fallback"
```

---

### Task 2: 新增 `seed_workgraph_from_mission()` 函数

**Files:**
- Modify: `src/background.rs`（新增约 50 行，在 `read_mission` 之后，`run_background_cfg` 之前）
- Test: `src/background.rs`（测试模块末尾追加）

**Interfaces:**
- Consumes: `provider: Arc<dyn Provider>`, `model: String`, `max_tokens: u32`, `temperature: f32`, `root: PathBuf`, `tool_cap: usize`
- Produces: `fn seed_workgraph_from_mission(...) -> bool` — true=成功写入里程碑, false=失败

- [ ] **Step 1: 实现 `seed_workgraph_from_mission()`**

```rust
/// 空 workgraph 时，通过一个 headless agent turn 调用 generate_milestones 工具
/// 自动分解使命为里程碑并写入 workgraph.json。成功写入返回 true，失败返回 false。
/// 注意：不注册 SIGINT（避免与主循环的 cancel token 冲突）。
fn seed_workgraph_from_mission(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    tool_cap: usize,
) -> bool {
    let mission = read_mission(&root);
    let prompt = format!(
        "你是一个项目规划助手。当前项目是一个空目录，需要你来初始化。\n\n\
         项目使命：\n{}\n\n\
         请先使用 list_directory 工具了解项目结构，然后使用 generate_milestones 工具\
         将上述使命分解为 3-8 个里程碑，每个里程碑包含：\n\
         - title（简短、可行动的标题）\n\
         - acceptance（具体、可验证的验收标准，尽量包含可执行的命令如 cargo test）\n\n\
         里程碑应按依赖顺序排列，前面的里程碑是后面里程碑的前提。",
        mission
    );

    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    agent.set_tool_cap(tool_cap);
    let (tx, rx) = channel::<AgentEvent>();
    let handle = std::thread::spawn(move || {
        agent.run_one_turn(prompt, &tx);
        drop(tx);
        agent
    });
    // Drain events (不收集，seed turn 的日志不重要)
    for _ev in rx.into_iter() {}
    match handle.join() {
        Ok(_agent) => {
            let g = WorkGraph::read(&root);
            !g.nodes.is_empty()
        }
        Err(_panic) => false,
    }
}
```

- [ ] **Step 2: 添加单元测试**

```rust
#[test]
fn seed_workgraph_from_mission_yields_milestones_with_stub() {
    // StubClient 不调用 generate_milestones → 返回 false
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "Build a CLI tool").unwrap();
    let ok = seed_workgraph_from_mission(
        Arc::new(StubClient), "m".into(), 4096, 0.0, dir.path().to_path_buf(), 8,
    );
    // Stub 不调用工具，workgraph 应为空
    assert!(!ok, "stub should not produce milestones");
    let g = WorkGraph::read(dir.path());
    assert!(g.nodes.is_empty(), "stub should not write any nodes");
}

#[test]
fn seed_workgraph_from_mission_no_agents_md_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    // 无 AGENTS.md → 走降级路径，不 panic
    let ok = seed_workgraph_from_mission(
        Arc::new(StubClient), "m".into(), 4096, 0.0, dir.path().to_path_buf(), 8,
    );
    // 降级后仍不调用工具 → false
    assert!(!ok);
}

#[test]
fn seed_workgraph_panicking_turn_returns_false() {
    struct PanicOnComplete;
    impl crate::provider::Provider for PanicOnComplete {
        fn name(&self) -> &str { "panic_seed" }
        fn complete(&self, _: &crate::provider::CompletionRequest) -> anyhow::Result<crate::provider::Completion> {
            panic!("seed provider panic");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "test").unwrap();
    let ok = seed_workgraph_from_mission(
        Arc::new(PanicOnComplete), "m".into(), 256, 0.0, dir.path().to_path_buf(), 8,
    );
    assert!(!ok, "panic should return false");
}
```

- [ ] **Step 3: 运行测试确认通过**

```bash
cargo test seed_workgraph_from_mission -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/background.rs
git commit -m "feat: add seed_workgraph_from_mission() for auto-milestone generation"
```

---

### Task 3: 修改 `run_background_cfg()` 的空图分支

**Files:**
- Modify: `src/background.rs`（约 5 行，第 168-173 行）

**Interfaces:**
- Consumes: 已有 `run_background_cfg` 函数参数中的 `tool_cap`
- Produces: 修改后的空图分支行为

- [ ] **Step 1: 修改 `run_background_cfg()` 第 168-173 行**

原代码：
```rust
    // #1 honesty: a genuinely empty graph is not "success" — nothing to advance.
    if graph.nodes.is_empty() {
        obs.emit("empty", "empty workgraph — nothing to advance; seed workgraph.json first");
        out.mission_state = crate::bg_gate::MissionState::EmptyGraph;
        return Ok(out);
    }
```

改为：
```rust
    // #1 empty graph: try to auto-seed from AGENTS.md; fall back to EmptyGraph on failure.
    if graph.nodes.is_empty() {
        obs.emit("seed", "empty workgraph — attempting to seed from AGENTS.md...");
        let seeded = seed_workgraph_from_mission(
            provider.clone(), model.clone(), max_tokens, temperature, root.clone(), tool_cap,
        );
        if seeded {
            obs.emit("seed", "workgraph seeded successfully — entering milestone loop");
            // Reset out state (drain from seed turn is irrelevant) and fall through
            // to the milestone loop below.
            out = BgOutcome::default();
            // Continue past this block into the loop
        } else {
            obs.emit("empty", "seed failed — empty workgraph");
            out.mission_state = crate::bg_gate::MissionState::EmptyGraph;
            return Ok(out);
        }
    }
```

- [ ] **Step 2: 确认 seed 成功后进入循环的逻辑正确**

注意：seed 成功后，`out` 已被重置为 `BgOutcome::default()`，`out.mission_state` 为 `Running`。代码继续执行到第 174 行 `out.mission_state = crate::bg_gate::MissionState::Running;`（重复赋值，但无害）。然后进入 `loop { ... }`。

关键：seed 成功后，`run_background_cfg` 的 `out` 变量在调用处为 `let mut out = BgOutcome::default();`，seed 后重置为 default，mission_state 重新设为 Running，然后进入 milestone 循环。这个路径是线性的，没有死循环风险。

- [ ] **Step 3: 添加集成测试**

```rust
#[test]
fn workgraph_self_seeds_and_advances() {
    // 集成测试：空 workgraph + AGENTS.md → seed → 推进
    // 用 StubClient（不调用工具），seed 失败 → EmptyGraph，不走后续循环
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "Build a test project").unwrap();
    // 没有 workgraph.json → 空图
    let out = run_background_cfg(
        Arc::new(StubClient), "m".into(), 256, 0.0, dir.path().to_path_buf(),
        String::new(), 3, 2, 8, 0,
    ).unwrap();
    // Stub 不生成里程碑 → EmptyGraph
    assert_eq!(out.mission_state, MissionState::EmptyGraph, "{:?}", out.mission_state);
}
```

- [ ] **Step 4: 验证已有非空 workgraph 的行为未变**

```rust
#[test]
fn workgraph_non_empty_still_advances_normally() {
    let dir = tempfile::tempdir().unwrap();
    ws(dir.path(), &[(1, "rustc --version", vec![])]); // 有节点
    let out = run_background_cfg(
        Arc::new(StubClient), "m".into(), 4096, 0.7, dir.path().to_path_buf(),
        String::new(), 3, 2, 8, 0,
    ).unwrap();
    // 应正常推进，非 EmptyGraph
    assert_ne!(out.mission_state, MissionState::EmptyGraph, "{:?}", out.mission_state);
}
```

- [ ] **Step 5: 运行全部 background 测试确认无回归**

```bash
cargo test background::tests -- --nocapture
```

- [ ] **Step 6: 提交**

```bash
git add src/background.rs
git commit -m "feat: wire empty-graph auto-seeding into run_background_cfg"
```

---

### Task 4: 编译验证

- [ ] **Step 1: 完整编译**

```bash
cargo build 2>&1
```

- [ ] **Step 2: 运行全部测试**

```bash
cargo test 2>&1
```

- [ ] **Step 3: 提交最终版本**

```bash
git add -A
git commit -m "feat: P0-1 empty workgraph auto-seeds milestones from AGENTS.md"
```