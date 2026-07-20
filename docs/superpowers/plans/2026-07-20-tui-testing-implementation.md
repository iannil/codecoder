# TUI 自动化测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for tracking.

**Goal:** 为 CodeCoder 添加 100% hermetic 的 TUI 自动化测试，覆盖全量 Mode 的渲染快照和 Handler 逻辑。

**Architecture:** 三层测试：① Render 快照层（`src/tui/render.rs` 新增 `snapshot_tests` 模块，用 `ratatui::backend::TestBackend` + `insta` 做纯文本网格快照比对）；② Handler 逻辑层（`src/tui/run.rs` 扩展现有 `tests` 模块，模拟按键/事件后断言状态字段）；③ PTY 端到端（维持现有 L2 门控，不扩展）。

**Tech Stack:** Rust, ratatui (TestBackend), insta (快照), crossterm (KeyEvent 构造)

## Global Constraints

- 所有测试必须 100% hermetic：不依赖真实终端、PTY、网络、API key
- 虚拟终端固定尺寸 80×24
- 测试位于 `src/tui/` 模块内（`#[cfg(test)]`），不创建新测试文件
- 快照管理通过 `cargo insta review` 交互式审查
- 遵循现有代码风格：`#[cfg(test)] mod tests` + `use super::*`

---

### Task 1: 添加 insta 到 dev-dependencies

**Files:**
- Modify: `Cargo.toml:35-39`

- [ ] **Step 1: 编辑 Cargo.toml 添加 insta**

在 `[dev-dependencies]` 区添加 `insta` 和 `similar`：

```
[dev-dependencies]
+insta = { version = "1", features = ["colors"] }
 tempfile = "3"
 portable-pty = "0.8"
```

- [ ] **Step 2: 验证编译**

```bash
cargo check
```
Expected: 编译通过，无新错误。

- [ ] **Step 3: 提交**

```bash
git add Cargo.toml
git commit -m "chore(deps): add insta for TUI snapshot testing"
```

---

### Task 2: 新增 Render 快照测试辅助函数

**Files:**
- Modify: `src/tui/render.rs:596-...`（文件末尾追加 `snapshot_tests` 模块）

**Interfaces:**
- Consumes: `TuiApp`（公开字段）、`draw()` 函数（当前文件）
- Produces: `render_snapshot(app: &TuiApp) -> String` — 在 TestBackend(80,24) 上渲染并返回纯文本网格

- [ ] **Step 1: 在 `src/tui/render.rs` 末尾追加 `snapshot_tests` 模块**

找到文件末尾 `fn centered(...)` 之后，追加：

```rust
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::tui::{TuiApp, Block, Dialog, Popup, PopupKind, PermissionDialog, AskDialog, PlanDialog, ConfirmDialog, TrustDialog, Activity, ToolResultView};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// 在 80×24 TestBackend 上渲染 app 并返回纯文本网格
    fn render_snapshot(app: &TuiApp) -> String {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal.backend().to_string()
    }
```

- [ ] **Step 2: 验证编译**

```bash
cargo check
```
Expected: 编译通过。

- [ ] **Step 3: 提交**

```bash
git add src/tui/render.rs
git commit -m "refactor(tui): add snapshot_tests module with render_snapshot helper"
```

---

### Task 3: Insert Mode 渲染快照（5 个测试）

**Files:**
- Modify: `src/tui/render.rs`（在 `snapshot_tests` 模块内追加测试）

- [ ] **Step 1: 添加空输入测试**

```rust
    #[test]
    fn insert_empty() {
        let app = TuiApp::new("m".into(), std::env::temp_dir());
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("insert_empty", snap);
    }
```

- [ ] **Step 2: 添加含文本输入测试**

```rust
    #[test]
    fn insert_with_text() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.insert_str("hello world");
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("insert_with_text", snap);
    }
```

- [ ] **Step 3: 添加多行输入测试**

```rust
    #[test]
    fn insert_multiline() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.insert_str("line1\nline2\nline3");
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("insert_multiline", snap);
    }
```

- [ ] **Step 4: 添加有活动行测试**

```rust
    #[test]
    fn insert_with_activity() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.activity = Some(Activity {
            label: "running test…".into(),
            started: std::time::Instant::now(),
        });
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("insert_with_activity", snap);
    }
```

- [ ] **Step 5: 添加 Permission Dialog 覆盖测试**

```rust
    #[test]
    fn insert_permission_dialog() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::ToolPermission(PermissionDialog::new(
            "run_command:git".into(),
            "git commit".into(),
            tx,
        )));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("insert_permission_dialog", snap);
    }
```

- [ ] **Step 6: 运行测试生成快照**

```bash
cargo test -p codecoder -- snapshot_tests::insert_ 2>&1
```
Expected: 测试失败，提示快照不存在（首次运行会创建）。这是预期行为——`insta` 会创建 `.snap` 文件。

查看生成的快照是否符合预期：

```bash
ls -la src/tui/snapshots/
```

- [ ] **Step 7: 接受快照并验证通过**

```bash
cargo insta accept
cargo test -p codecoder -- snapshot_tests::insert_
```
Expected: 所有测试通过（PASS）。

- [ ] **Step 8: 提交**

```bash
git add src/tui/render.rs src/tui/snapshots/
git commit -m "test(tui): add Insert mode render snapshot tests"
```

---

### Task 4: Transcript 渲染快照（7 个测试）

**Files:**
- Modify: `src/tui/render.rs`（在 `snapshot_tests` 模块内追加）

- [ ] **Step 1: 添加空 transcript 测试**

```rust
    #[test]
    fn transcript_empty() {
        let app = TuiApp::new("m".into(), std::env::temp_dir());
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_empty", snap);
    }
```

- [ ] **Step 2: 添加 User 消息测试**

```rust
    #[test]
    fn transcript_user() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.blocks.push(Block::User("hello".into()));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_user", snap);
    }
```

- [ ] **Step 3: 添加 Assistant 消息测试**

```rust
    #[test]
    fn transcript_assistant() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.blocks.push(Block::Assistant("response".into()));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_assistant", snap);
    }
```

- [ ] **Step 4: 添加 Tool 块（无结果）测试**

```rust
    #[test]
    fn transcript_tool_no_result() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.blocks.push(Block::Tool {
            name: "read_file".into(),
            preview: "src/main.rs".into(),
            result: None,
            folded: true,
        });
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_tool_no_result", snap);
    }
```

- [ ] **Step 5: 添加 Tool 块（折叠长结果）测试**

```rust
    #[test]
    fn transcript_tool_long_folded() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let long = (0..15).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        app.blocks.push(Block::Tool {
            name: "run_command".into(),
            preview: "cargo test".into(),
            result: Some(ToolResultView { text: long, is_error: false }),
            folded: true,
        });
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_tool_long_folded", snap);
    }
```

- [ ] **Step 6: 添加 Reasoning 折叠测试**

```rust
    #[test]
    fn transcript_reasoning_folded() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.blocks.push(Block::Reasoning {
            text: "thinking about\nthis problem\nstep by step".into(),
            folded: true,
        });
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_reasoning_folded", snap);
    }
```

- [ ] **Step 7: 添加混合多块测试**

```rust
    #[test]
    fn transcript_mixed_blocks() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.blocks.push(Block::User("explain the code".into()));
        app.blocks.push(Block::Reasoning { text: "analyzing…".into(), folded: true });
        app.blocks.push(Block::Assistant("Here is the analysis:\nThe code does X.".into()));
        app.blocks.push(Block::Tool {
            name: "read_file".into(),
            preview: "src/main.rs".into(),
            result: Some(ToolResultView { text: "fn main() {}".into(), is_error: false }),
            folded: true,
        });
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("transcript_mixed_blocks", snap);
    }
```

- [ ] **Step 8: 运行测试并接受快照**

```bash
cargo test -p codecoder -- snapshot_tests::transcript_ 2>&1
cargo insta accept
cargo test -p codecoder -- snapshot_tests::transcript_
```
Expected: 所有测试通过。

- [ ] **Step 9: 提交**

```bash
git add src/tui/render.rs src/tui/snapshots/
git commit -m "test(tui): add Transcript render snapshot tests"
```

---

### Task 5: Dialog 全类型渲染快照（6 个测试）

**Files:**
- Modify: `src/tui/render.rs`（在 `snapshot_tests` 模块内追加）

- [ ] **Step 1: 添加 ToolPermission（4 选项）测试**

```rust
    #[test]
    fn dialog_permission_full() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::ToolPermission(PermissionDialog::new(
            "write_file".into(),
            "write to /tmp/x.txt".into(),
            tx,
        )));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("dialog_permission_full", snap);
    }
```

- [ ] **Step 2: 添加 ToolPermission（无 project 选项，@shell 降级）测试**

```rust
    #[test]
    fn dialog_permission_no_project() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::ToolPermission(PermissionDialog::new(
            "run_command:git@shell".into(),
            "run git commit".into(),
            tx,
        )));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("dialog_permission_no_project", snap);
    }
```

- [ ] **Step 3: 添加 AskQuestion 测试**

```rust
    #[test]
    fn dialog_ask_question() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::AskQuestion(AskDialog {
            prompt: "What file should I edit?".into(),
            input: "".into(),
            reply_tx: tx,
        }));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("dialog_ask_question", snap);
    }
```

- [ ] **Step 4: 添加 PlanApproval 测试**

```rust
    #[test]
    fn dialog_plan_approval() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::PlanApproval(PlanDialog {
            plan: "1. Read the file\n2. Edit the line\n3. Commit".into(),
            selected: 0,
            reply_tx: tx,
        }));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("dialog_plan_approval", snap);
    }
```

- [ ] **Step 5: 添加 Confirm 测试**

```rust
    #[test]
    fn dialog_confirm() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::Confirm(ConfirmDialog {
            prompt: "Are you sure you want to delete this file?".into(),
            selected: 0,
            reply_tx: tx,
        }));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("dialog_confirm", snap);
    }
```

- [ ] **Step 6: 添加 Trust 测试**

```rust
    #[test]
    fn dialog_trust() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.dialog = Some(Dialog::Trust(TrustDialog {
            root: "/tmp/project".into(),
            selected: 0,
            reply_tx: tx,
        }));
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("dialog_trust", snap);
    }
```

- [ ] **Step 7: 运行测试并接受快照**

```bash
cargo test -p codecoder -- snapshot_tests::dialog_ 2>&1
cargo insta accept
cargo test -p codecoder -- snapshot_tests::dialog_
```
Expected: 所有测试通过。

- [ ] **Step 8: 提交**

```bash
git add src/tui/render.rs src/tui/snapshots/
git commit -m "test(tui): add Dialog render snapshot tests"
```

---

### Task 6: Search / Browse / Help / Verify / Popup 渲染快照（10 个测试）

**Files:**
- Modify: `src/tui/render.rs`（在 `snapshot_tests` 模块内追加）

- [ ] **Step 1: 添加 Search 活跃测试**

```rust
    #[test]
    fn search_active() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.begin_search(false);
        app.search_query = "test".into();
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("search_active", snap);
    }
```

- [ ] **Step 2: 添加 R-Search 活跃测试**

```rust
    #[test]
    fn search_reverse() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.begin_search(true);
        app.search_query = "test".into();
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("search_reverse", snap);
    }
```

- [ ] **Step 3: 添加 Browse 选中测试**

```rust
    #[test]
    fn browse_selected() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.blocks.push(Block::Reasoning { text: "analysis\nline2".into(), folded: true });
        app.blocks.push(Block::Reasoning { text: "more thoughts\nline2".into(), folded: true });
        app.browsing = true;
        app.browse_sel = 0;
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("browse_selected", snap);
    }
```

- [ ] **Step 4: 添加 Help 打开测试**

```rust
    #[test]
    fn help_open() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.help_open = true;
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("help_open", snap);
    }
```

- [ ] **Step 5: 添加 Verify 运行中测试**

```rust
    #[test]
    fn verify_running() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.verify_state.running = true;
        app.verify_state.total_tests = 10;
        app.verify_state.passed = 3;
        app.verify_state.failed = 1;
        app.verify_state.completed = 4;
        // Add one layer with one module with one passed case
        let state = &mut app.verify_state;
        state.layers[0].modules.push(crate::verify::state::ModuleState {
            name: "kernel".into(),
            cases: vec![
                crate::verify::state::CaseState {
                    name: "test_compiles".into(),
                    status: crate::verify::state::CaseStatus::Passed,
                    output: Vec::new(),
                    duration_ms: 120,
                },
                crate::verify::state::CaseState {
                    name: "test_behavior".into(),
                    status: crate::verify::state::CaseStatus::Failed("assertion failed".into()),
                    output: vec!["expected: true".into(), "actual: false".into()],
                    duration_ms: 50,
                },
            ],
            folded: false,
            passed: 1,
            failed: 1,
            skipped: 0,
            running: 0,
        });
        state.layers[0].passed = 1;
        state.layers[0].failed = 1;
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("verify_running", snap);
    }
```

- [ ] **Step 6: 添加 Verify 全部通过测试**

```rust
    #[test]
    fn verify_all_passed() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.verify_state.total_tests = 5;
        app.verify_state.passed = 5;
        app.verify_state.completed = 5;
        app.verify_state.elapsed_ms = 1200;
        let state = &mut app.verify_state;
        state.layers[0].modules.push(crate::verify::state::ModuleState {
            name: "kernel".into(),
            cases: vec![
                crate::verify::state::CaseState {
                    name: "test_a".into(),
                    status: crate::verify::state::CaseStatus::Passed,
                    output: Vec::new(),
                    duration_ms: 100,
                },
            ],
            folded: false,
            passed: 1,
            failed: 0,
            skipped: 0,
            running: 0,
        });
        state.layers[0].passed = 1;
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("verify_all_passed", snap);
    }
```

- [ ] **Step 7: 添加 Verify 有失败测试**

```rust
    #[test]
    fn verify_with_failures() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.verify_state.total_tests = 3;
        app.verify_state.passed = 1;
        app.verify_state.failed = 2;
        app.verify_state.completed = 3;
        app.verify_state.elapsed_ms = 800;
        let state = &mut app.verify_state;
        state.layers[0].modules.push(crate::verify::state::ModuleState {
            name: "tools".into(),
            cases: vec![
                crate::verify::state::CaseState {
                    name: "test_read".into(),
                    status: crate::verify::state::CaseStatus::Passed,
                    output: Vec::new(),
                    duration_ms: 50,
                },
                crate::verify::state::CaseState {
                    name: "test_write".into(),
                    status: crate::verify::state::CaseStatus::Failed("permission denied".into()),
                    output: vec!["Error: EACCES".into()],
                    duration_ms: 30,
                },
            ],
            folded: false,
            passed: 1,
            failed: 1,
            skipped: 0,
            running: 0,
        });
        state.layers[0].passed = 1;
        state.layers[0].failed = 1;
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("verify_with_failures", snap);
    }
```

- [ ] **Step 8: 添加 Slash 补全弹窗测试**

```rust
    #[test]
    fn popup_slash() {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        app.input = "/re".into();
        app.refresh_popup();
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("popup_slash", snap);
    }
```

- [ ] **Step 9: 添加文件补全弹窗测试**

```rust
    #[test]
    fn popup_file() {
        let dir = std::env::temp_dir().join(format!("cc_pop_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "x").unwrap();

        let mut app = TuiApp::new("m".into(), dir.clone());
        app.input = "edit @not".into();
        app.refresh_popup();
        let snap = render_snapshot(&app);
        insta::assert_snapshot!("popup_file", snap);

        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 10: 运行测试并接受快照**

```bash
cargo test -p codecoder -- snapshot_tests:: 2>&1
cargo insta accept
cargo test -p codecoder -- snapshot_tests::
```
Expected: 所有测试通过。

- [ ] **Step 11: 提交**

```bash
git add src/tui/render.rs src/tui/snapshots/
git commit -m "test(tui): add Search/Browse/Help/Verify/Popup render snapshot tests"
```

---

### Task 7: Insert Key Handler 逻辑测试（8 条）

**Files:**
- Modify: `src/tui/run.rs:563-629`（扩展现有 `tests` 模块）

- [ ] **Step 1: 添加提交空文本测试**

```rust
    #[test]
    fn submit_empty_text_does_nothing() {
        let (mut app, _token) = app_with_token();
        app.input = "  ".into();
        let (tx, rx) = channel::<AgentCommand>();
        let block_count = app.blocks.len();

        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Enter), &tx);

        assert_eq!(app.blocks.len(), block_count, "empty input must not add a block");
        assert!(rx.try_recv().is_err(), "empty input must not send ProcessMessage");
    }
```

- [ ] **Step 2: 添加提交普通文本测试**

```rust
    #[test]
    fn submit_text_sends_process_message() {
        let (mut app, _token) = app_with_token();
        let steer = SteerQueue::default();
        app.steer = steer.clone();
        app.activity = None;
        app.input = "hello".into();
        let (tx, rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Enter), &tx);

        assert!(app.blocks.iter().any(|b| matches!(b, Block::User(_))));
        assert!(matches!(rx.try_recv(), Ok(AgentCommand::ProcessMessage(m)) if m == "hello"));
    }
```

- [ ] **Step 3: 添加提交时已有 turn 测试**

```rust
    #[test]
    fn submit_during_turn_steers() {
        let (mut app, _token) = app_with_token();
        let steer = SteerQueue::default();
        app.steer = steer.clone();
        app.activity = Some(Activity { label: "working".into(), started: std::time::Instant::now() });
        app.input = "keep going".into();
        let (tx, rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Enter), &tx);

        assert_eq!(steer.drain(), vec!["keep going".to_string()]);
        assert!(rx.try_recv().is_err(), "no ProcessMessage while a turn is in flight");
    }
```

- [ ] **Step 4: 添加 /exit 命令测试**

```rust
    #[test]
    fn slash_exit_quits() {
        let (mut app, _token) = app_with_token();
        app.input = "/exit".into();
        let (tx, rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Enter), &tx);

        assert!(app.should_quit);
        assert!(matches!(rx.try_recv(), Ok(AgentCommand::Shutdown)));
    }
```

- [ ] **Step 5: 添加 /resume 命令测试**

```rust
    #[test]
    fn slash_resume_sends_resume() {
        let (mut app, _token) = app_with_token();
        app.input = "/resume".into();
        let (tx, rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Enter), &tx);

        assert!(matches!(rx.try_recv(), Ok(AgentCommand::Resume)));
    }
```

- [ ] **Step 6: 添加 /clear 命令测试**

```rust
    #[test]
    fn slash_clear_clears_blocks() {
        let (mut app, _token) = app_with_token();
        app.blocks.push(Block::User("old".into()));
        app.browsing = true;
        app.input = "/clear".into();
        let (tx, _rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Enter), &tx);

        assert!(app.blocks.is_empty());
        assert!(!app.browsing);
    }
```

- [ ] **Step 7: 添加 Ctrl+C 退出测试**

```rust
    #[test]
    fn ctrl_c_quits() {
        let (mut app, _token) = app_with_token();
        let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let (tx, _rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, k, &tx);

        assert!(app.should_quit);
    }
```

- [ ] **Step 8: 添加 Shift+Enter 换行测试**

```rust
    #[test]
    fn shift_enter_inserts_newline() {
        let (mut app, _token) = app_with_token();
        app.input = "hello".into();
        app.cursor = 5;
        let k = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        let (tx, _rx) = channel::<AgentCommand>();

        handle_insert_key(&mut app, k, &tx);

        assert_eq!(app.input, "hello\n");
        assert_eq!(app.cursor, 6);
    }
```

- [ ] **Step 9: 运行测试**

```bash
cargo test -p codecoder -- run::tests::submit_ 2>&1
cargo test -p codecoder -- run::tests::slash_ 2>&1
cargo test -p codecoder -- run::tests::ctrl_c 2>&1
cargo test -p codecoder -- run::tests::shift_enter 2>&1
```
Expected: 所有测试通过。

- [ ] **Step 10: 提交**

```bash
git add src/tui/run.rs
git commit -m "test(tui): add Insert key handler logical tests"
```

---

### Task 8: Dialog Handler 逻辑测试（7 条）

**Files:**
- Modify: `src/tui/run.rs`（在 `tests` 模块内追加）

- [ ] **Step 1: 添加 Permission 选 once 测试**

```rust
    #[test]
    fn permission_select_once() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<PermissionReply>();
        app.dialog = Some(Dialog::ToolPermission(PermissionDialog::new(
            "write_file".into(), "test".into(), tx,
        )));
        // simulate Enter (selected=0 = once)
        handle_input(&mut app, Event::Key(KeyEvent::from(KeyCode::Enter)), &channel::<AgentCommand>().0);
        if let Ok(reply) = rx.try_recv() {
            assert_eq!(reply, PermissionReply::Grant(PermScope::Once));
        } else {
            panic!("expected Grant(Once)");
        }
    }
```

- [ ] **Step 2: 添加 Permission Esc 拒绝测试**

```rust
    #[test]
    fn permission_esc_denies() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<PermissionReply>();
        app.dialog = Some(Dialog::ToolPermission(PermissionDialog::new(
            "write_file".into(), "test".into(), tx,
        )));
        handle_input(&mut app, Event::Key(KeyEvent::from(KeyCode::Esc)), &channel::<AgentCommand>().0);
        if let Ok(reply) = rx.try_recv() {
            assert_eq!(reply, PermissionReply::Deny);
        } else {
            panic!("expected Deny");
        }
    }
```

- [ ] **Step 3: 添加 Ask 输入并提交测试**

```rust
    #[test]
    fn ask_question_submit() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<String>();
        app.dialog = Some(Dialog::AskQuestion(AskDialog {
            prompt: "Enter name:".into(),
            input: "Alice".into(),
            reply_tx: tx,
        }));
        // Simulate: type 's' (append to input), then Enter
        handle_input(&mut app, Event::Key(KeyEvent::from(KeyCode::Char('s'))), &channel::<AgentCommand>().0);
        // Then Enter
        let (tx2, _) = channel::<AgentCommand>();
        handle_input(&mut app, Event::Key(KeyEvent::from(KeyCode::Enter)), &tx2);
        if let Ok(answer) = rx.try_recv() {
            assert_eq!(answer, "Alices");
        } else {
            panic!("expected answer to be sent");
        }
    }
```

- [ ] **Step 4: 添加 Ask Esc 取消测试**

```rust
    #[test]
    fn ask_question_esc_cancels() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<String>();
        app.dialog = Some(Dialog::AskQuestion(AskDialog {
            prompt: "Enter name:".into(),
            input: "".into(),
            reply_tx: tx,
        }));
        let (tx2, _) = channel::<AgentCommand>();
        handle_input(&mut app, Event::Key(KeyEvent::from(KeyCode::Esc)), &tx2);
        if let Ok(answer) = rx.try_recv() {
            assert_eq!(answer, "");
        } else {
            panic!("expected empty string on cancel");
        }
    }
```

- [ ] **Step 5: 添加 Plan 审批测试**

```rust
    #[test]
    fn plan_approve() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<bool>();
        app.dialog = Some(Dialog::PlanApproval(PlanDialog {
            plan: "do something".into(),
            selected: 0,
            reply_tx: tx,
        }));
        let (tx2, _) = channel::<AgentCommand>();
        // 'a' hotkey approves
        handle_input(&mut app, Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)), &tx2);
        if let Ok(approved) = rx.try_recv() {
            assert!(approved);
        } else {
            panic!("expected approved=true");
        }
    }
```

- [ ] **Step 6: 添加 Plan 拒绝测试**

```rust
    #[test]
    fn plan_reject() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<bool>();
        app.dialog = Some(Dialog::PlanApproval(PlanDialog {
            plan: "do something".into(),
            selected: 0,
            reply_tx: tx,
        }));
        let (tx2, _) = channel::<AgentCommand>();
        // 'r' hotkey rejects
        handle_input(&mut app, Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)), &tx2);
        if let Ok(approved) = rx.try_recv() {
            assert!(!approved);
        } else {
            panic!("expected approved=false");
        }
    }
```

- [ ] **Step 7: 添加 Confirm 确认测试**

```rust
    #[test]
    fn confirm_yes() {
        let (mut app, _token) = app_with_token();
        let (tx, rx) = channel::<bool>();
        app.dialog = Some(Dialog::Confirm(ConfirmDialog {
            prompt: "Are you sure?".into(),
            selected: 0,
            reply_tx: tx,
        }));
        let (tx2, _) = channel::<AgentCommand>();
        // Enter with selected=0 = yes
        handle_input(&mut app, Event::Key(KeyEvent::from(KeyCode::Enter)), &tx2);
        if let Ok(yes) = rx.try_recv() {
            assert!(yes);
        } else {
            panic!("expected confirmed=true");
        }
    }
```

- [ ] **Step 8: 运行测试**

```bash
cargo test -p codecoder -- run::tests::permission_ 2>&1
cargo test -p codecoder -- run::tests::ask_ 2>&1
cargo test -p codecoder -- run::tests::plan_ 2>&1
cargo test -p codecoder -- run::tests::confirm_ 2>&1
```
Expected: 所有测试通过。

- [ ] **Step 9: 提交**

```bash
git add src/tui/run.rs
git commit -m "test(tui): add Dialog handler logical tests"
```

---

### Task 9: Agent Event Handler 逻辑测试（5 条）

**Files:**
- Modify: `src/tui/run.rs`（在 `tests` 模块内追加）

- [ ] **Step 1: 添加连续 StreamDelta 测试**

```rust
    #[test]
    fn stream_delta_accumulates() {
        let (mut app, _token) = app_with_token();
        handle_agent(&mut app, AgentEvent::StreamDelta("Hello ".into()));
        handle_agent(&mut app, AgentEvent::StreamDelta("world".into()));
        handle_agent(&mut app, AgentEvent::StreamDelta("!".into()));

        assert!(app.streaming);
        assert_eq!(app.blocks.len(), 1);
        let last = &app.blocks[0];
        if let Block::Assistant(text) = last {
            assert_eq!(text, "Hello world!");
        } else {
            panic!("expected Assistant block");
        }
    }
```

- [ ] **Step 2: 添加 ToolStarted 结束流测试**

```rust
    #[test]
    fn tool_started_ends_stream() {
        let (mut app, _token) = app_with_token();
        handle_agent(&mut app, AgentEvent::StreamDelta("hello".into()));
        assert!(app.streaming);

        handle_agent(&mut app, AgentEvent::ToolStarted {
            name: "read_file".into(),
            preview: "src/main.rs".into(),
        });

        assert!(!app.streaming);
        assert_eq!(app.blocks.len(), 2); // assistant + tool
        if let Block::Tool { name, result, .. } = &app.blocks[1] {
            assert_eq!(name, "read_file");
            assert!(result.is_none());
        } else {
            panic!("expected Tool block");
        }
    }
```

- [ ] **Step 3: 添加 ToolFinished 填充结果测试**

```rust
    #[test]
    fn tool_finished_fills_result() {
        let (mut app, _token) = app_with_token();
        handle_agent(&mut app, AgentEvent::ToolStarted {
            name: "read_file".into(),
            preview: "src/main.rs".into(),
        });
        handle_agent(&mut app, AgentEvent::ToolFinished {
            name: "read_file".into(),
            is_error: false,
            output: "fn main() {}".into(),
        });

        if let Block::Tool { result, folded, .. } = &app.blocks[0] {
            assert!(result.is_some());
            assert_eq!(result.as_ref().unwrap().text, "fn main() {}");
            assert!(!folded, "short result should not be folded");
        } else {
            panic!("expected Tool block");
        }
    }
```

- [ ] **Step 4: 添加 PermissionRequest 弹出 Dialog 测试**

```rust
    #[test]
    fn permission_request_opens_dialog() {
        let (mut app, _token) = app_with_token();
        let (tx, _rx) = channel::<PermissionReply>();
        handle_agent(&mut app, AgentEvent::PermissionRequest {
            key: "write_file".into(),
            preview: "write test".into(),
            reply_tx: tx,
        });

        assert!(app.dialog.is_some());
        assert!(matches!(app.dialog, Some(Dialog::ToolPermission(_))));
    }
```

- [ ] **Step 5: 添加 TurnComplete 清空状态测试**

```rust
    #[test]
    fn turn_complete_clears_activity() {
        let (mut app, _token) = app_with_token();
        app.streaming = true;
        app.activity = Some(Activity { label: "thinking".into(), started: std::time::Instant::now() });

        handle_agent(&mut app, AgentEvent::TurnComplete);

        assert!(!app.streaming);
        assert!(app.activity.is_none());
    }
```

- [ ] **Step 6: 运行测试**

```bash
cargo test -p codecoder -- run::tests::stream_delta 2>&1
cargo test -p codecoder -- run::tests::tool_started 2>&1
cargo test -p codecoder -- run::tests::tool_finished 2>&1
cargo test -p codecoder -- run::tests::permission_request 2>&1
cargo test -p codecoder -- run::tests::turn_complete 2>&1
```
Expected: 所有测试通过。

- [ ] **Step 7: 提交**

```bash
git add src/tui/run.rs
git commit -m "test(tui): add Agent Event handler logical tests"
```

---

### Task 10: 全量运行验证

- [ ] **Step 1: 运行全部 TUI 测试**

```bash
cargo test -p codecoder -- tui:: 2>&1
```
Expected: 所有测试通过。

- [ ] **Step 2: 运行完整测试套件（不含 L2/L3）**

```bash
cargo test 2>&1
```
Expected: 全部 L1 测试通过（含新增的 ~45 个 TUI 测试）。

- [ ] **Step 3: 最终提交（如果还有未提交的变更）**

```bash
git add .
git commit -m "test(tui): finalize TUI test suite — all snapshot and handler tests passing"
```