# Worktree + ToolSearch 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 CodeCoder 补全 P2 工具：Worktree 隔离（`enter_worktree`/`exit_worktree`）和 ToolSearch 动态工具发现（`tool_search`），共 3 个工具。

**Architecture:** Worktree 工具通过 `std::process::Command` 调用 `git` 创建/管理 worktree；ToolSearch 把 `Toolbox` 拆分为 core/extra 两层，`tool_search` 通过 `session_meta_mark` 把匹配的工具名传给 `AgentLoop` 自动加载。

**Tech Stack:** Rust, serde_json, std::process::Command, std::sync::Mutex

---

## 全局约束

- 遵循 ADR 0018 的 `Tool` trait 接口：`name()`、`description()`、`schema()`、`permission()`、`run()`
- 权限模型：worktree 工具 = `Permission::Ask { key: "worktree" }`，`tool_search` = `Permission::None`
- 错误处理：统一通过 `ToolOutput::err()` 返回，不 panic
- 测试：单元测试覆盖 WorktreeStore/WorktreeEntry 逻辑，ToolSearch 的搜索逻辑
- 新工具注册到 `src/tool/mod.rs` 的 `Toolbox::builtin()`
- 现有测试基线：533 通过

---

### Task 1: Worktree 工具 — 数据结构 + 2 个工具

**Files:**
- Create: `src/tool/worktree.rs`
- Modify: `src/tool/mod.rs` (添加 `pub mod worktree` + 注册 2 个工具)
- Test: 内联在 `src/tool/worktree.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Tool`, `ToolCtx`, `ToolOutput`, `Permission`
- Produces: `WorktreeEntry` (struct), `WorktreeStore` (全局单例), `EnterWorktree`/`ExitWorktree` (Tool 类型)

- [ ] **Step 1: 定义数据结构**

```rust
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub id: u64,
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_branch: String,
    pub session_id: String,
    pub created_at: SystemTime,
}

pub struct WorktreeStore {
    entries: Vec<WorktreeEntry>,
    next_id: u64,
}

impl WorktreeStore {
    pub fn new() -> Self { Self { entries: Vec::new(), next_id: 1 } }

    pub fn add(&mut self, entry: WorktreeEntry) { self.entries.push(entry); }

    pub fn get(&self, id: u64) -> Option<&WorktreeEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    pub fn list(&self) -> &Vec<WorktreeEntry> { &self.entries }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

pub static WORKTREE_STORE: LazyLock<Mutex<WorktreeStore>> =
    LazyLock::new(|| Mutex::new(WorktreeStore::new()));
```

- [ ] **Step 2: 实现 git 辅助函数**

```rust
/// Run a git command in the given root, return stdout/stderr combined.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git failed: {e}"))
        .and_then(|out| {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.stderr.is_empty() {
                s.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            if out.status.success() { Ok(s) } else { Err(s) }
        })
}
```

- [ ] **Step 3: 实现 EnterWorktree 工具**

```rust
pub struct EnterWorktree;

impl Tool for EnterWorktree {
    fn name(&self) -> &str { "enter_worktree" }
    fn description(&self) -> &str {
        "Create a new git worktree with an isolated branch and session directory. \
         Returns the worktree path and branch name. Call exit_worktree when done."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Optional worktree name. Auto-generated if omitted." },
                "base_branch": { "type": "string", "description": "Branch to fork from. Default: master." }
            }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "worktree".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).map(String::from)
            .unwrap_or_else(|| format!("wt_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()));
        let base_branch = args.get("base_branch").and_then(Value::as_str)
            .unwrap_or("master").to_string();
        let branch = format!("feat/{name}");
        let worktree_path = ctx.root.join(".worktrees").join(&name);

        if worktree_path.exists() {
            return Ok(ToolOutput::err(format!("worktree already exists: {name}")));
        }

        // Ensure .worktrees directory exists
        if let Err(e) = std::fs::create_dir_all(ctx.root.join(".worktrees")) {
            return Ok(ToolOutput::err(format!("cannot create .worktrees dir: {e}")));
        }

        // git worktree add
        let path_str = worktree_path.to_string_lossy().to_string();
        match git(ctx.root, &["worktree", "add", &path_str, "-b", &branch, &base_branch]) {
            Ok(output) => {
                let mut store = WORKTREE_STORE.lock().unwrap();
                let id = store.next_id();
                store.add(WorktreeEntry {
                    id,
                    name: name.clone(),
                    path: worktree_path,
                    branch: branch.clone(),
                    base_branch,
                    session_id: format!("worktree-{name}"),
                    created_at: SystemTime::now(),
                });
                Ok(ToolOutput::ok(json!({ "id": id, "path": format!(".worktrees/{name}"), "branch": branch }).to_string()))
            }
            Err(e) => Ok(ToolOutput::err(format!("worktree creation failed: {e}"))),
        }
    }
}
```

- [ ] **Step 4: 实现 ExitWorktree 工具**

```rust
pub struct ExitWorktree;

impl Tool for ExitWorktree {
    fn name(&self) -> &str { "exit_worktree" }
    fn description(&self) -> &str {
        "Exit a worktree. Actions: 'merge' (merge back and cleanup), 'keep' (leave as-is), 'discard' (delete worktree and branch)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["merge", "keep", "discard"],
                    "description": "What to do with the worktree."
                }
            },
            "required": ["action"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "worktree".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or_default();
        if action.is_empty() {
            return Ok(ToolOutput::err("exit_worktree requires `action`"));
        }

        // Get the latest worktree entry (most recently created)
        let entry = {
            let store = WORKTREE_STORE.lock().unwrap();
            store.list().last().cloned()
        };
        let Some(entry) = entry else {
            return Ok(ToolOutput::err("no active worktree found"));
        };

        match action {
            "keep" => {
                WORKTREE_STORE.lock().unwrap().remove(entry.id);
                Ok(ToolOutput::ok(format!(
                    "worktree kept at .worktrees/{} on branch {}", entry.name, entry.branch
                )))
            }
            "merge" => {
                // Checkout base branch, merge the worktree branch, then cleanup
                match git(ctx.root, &["checkout", &entry.base_branch])
                    .and_then(|_| git(ctx.root, &["merge", &entry.branch]))
                    .and_then(|_| git(ctx.root, &["worktree", "remove", &entry.path.to_string_lossy()]))
                    .and_then(|_| git(ctx.root, &["branch", "-D", &entry.branch]))
                {
                    Ok(out) => {
                        WORKTREE_STORE.lock().unwrap().remove(entry.id);
                        Ok(ToolOutput::ok(format!("merged {} into {}: {out}", entry.branch, entry.base_branch)))
                    }
                    Err(e) => Ok(ToolOutput::err(format!("merge failed: {e}"))),
                }
            }
            "discard" => {
                match git(ctx.root, &["worktree", "remove", &entry.path.to_string_lossy()])
                    .and_then(|_| git(ctx.root, &["branch", "-D", &entry.branch]))
                {
                    Ok(out) => {
                        WORKTREE_STORE.lock().unwrap().remove(entry.id);
                        Ok(ToolOutput::ok(format!("discarded worktree {}: {out}", entry.name)))
                    }
                    Err(e) => Ok(ToolOutput::err(format!("discard failed: {e}"))),
                }
            }
            _ => Ok(ToolOutput::err(format!("unknown action: {action}"))),
        }
    }
}
```

- [ ] **Step 5: 单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_store_add_and_remove() {
        let mut store = WorktreeStore::new();
        let id = store.next_id();
        store.add(WorktreeEntry {
            id, name: "test".into(), path: PathBuf::from("/tmp/wt"),
            branch: "feat/test".into(), base_branch: "master".into(),
            session_id: "ws-test".into(), created_at: SystemTime::now(),
        });
        assert_eq!(store.list().len(), 1);
        assert!(store.remove(id));
        assert!(store.list().is_empty());
    }

    #[test]
    fn worktree_tools_permission_model() {
        assert!(matches!(EnterWorktree.permission(&json!({}), Path::new(".")),
            Permission::Ask { key } if key == "worktree"));
        assert!(matches!(ExitWorktree.permission(&json!({}), Path::new(".")),
            Permission::Ask { key } if key == "worktree"));
    }

    #[test]
    fn enter_worktree_requires_name() {
        // No validation needed — name is optional, auto-generated
    }

    #[test]
    fn exit_worktree_requires_action() {
        let out = ExitWorktree.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("requires `action`"));
    }
}
```

- [ ] **Step 6: 注册到 Toolbox 并提交**

```bash
cargo build 2>&1 | tail -3
cargo test worktree 2>&1 | tail -5
git add src/tool/worktree.rs src/tool/mod.rs
git commit -m "feat(worktree): add git worktree isolation tools

Implement enter_worktree and exit_worktree for managing git worktrees
with isolated branches, plus an in-memory WorktreeStore.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Toolbox 重构 — 核心/扩展工具分离

**Files:**
- Modify: `src/tool/mod.rs` (Toolbox 结构体改为 core/extra 双列表)
- Modify: `src/agent.rs` (添加 `loaded_extra_tools: HashSet<String>` 字段，改造 `wire_schemas()`)
- Test: 现有测试继续通过

**Interfaces:**
- Consumes: 现有 `Toolbox::builtin()` 构造点
- Produces: `Toolbox::wire_schemas_core()`, `Toolbox::wire_schemas_subset()`, `Toolbox::search()`, `AgentLoop.loaded_extra_tools`

- [ ] **Step 1: 重构 Toolbox 结构体**

```rust
pub struct Toolbox {
    core_tools: Vec<Box<dyn Tool>>,
    extra_tools: Vec<Box<dyn Tool>>,
}

impl Toolbox {
    pub fn builtin() -> Self {
        let mut core: Vec<Box<dyn Tool>> = vec![
            Box::new(builtin::ReadFile),
            Box::new(builtin::ListDirectory),
            Box::new(builtin::WriteFile),
            Box::new(builtin::EditFile),
            Box::new(builtin::RunCommand),
            Box::new(builtin::UseSkill),
            Box::new(search::Glob),
            Box::new(search::Grep),
            Box::new(net::SearchWeb),
            Box::new(net::SearchGithub),
            Box::new(dev::Commit),
            Box::new(dev::Diff),
            Box::new(dev::Plan),
            Box::new(dev::Milestone),
            Box::new(dev::Memory),
            Box::new(builtin::AskUser),
            Box::new(builtin::Confirm),
            Box::new(builtin::Agent),
            Box::new(reason::Reason),
            Box::new(builtin::Review),
        ];
        let mut extra: Vec<Box<dyn Tool>> = vec![
            Box::new(builtin::RunCapability),
            Box::new(builtin::GenerateSkill),
            Box::new(builtin::GeneratePrompt),
            Box::new(builtin::PromotePrompt),
            Box::new(builtin::GenerateCapability),
            Box::new(net::ReverseApi),
            Box::new(generate_milestones::GenerateMilestones),
            Box::new(mcp::McpToolCall),
            Box::new(mcp::McpListResources),
            Box::new(mcp::McpReadResource),
            Box::new(lsp::LspTool),
            Box::new(task_manage::TaskCreate),
            Box::new(task_manage::TaskGet),
            Box::new(task_manage::TaskList),
            Box::new(task_manage::TaskUpdate),
            Box::new(task_manage::TaskStop),
            Box::new(cron::CronCreate),
            Box::new(cron::CronDelete),
            Box::new(cron::CronList),
            Box::new(send_message::SendMessage),
            Box::new(worktree::EnterWorktree),
            Box::new(worktree::ExitWorktree),
        ];
        // 把所有工具也保留在 unified 列表中用于 get() 和全量 wire_schemas()
        let mut all = Vec::new();
        all.extend(core.iter().map(|t| /* ??? */));
        // 由于 Tool 是 trait object，Vec<Box<dyn Tool>> 不能直接引用。
        // 更好的设计：get() 和 search() 遍历 core + extra
        Self { core_tools, extra_tools }
    }
}
```

**关键设计问题：** `Toolbox` 的 `get()` 方法需要遍历所有工具（core + extra）。`wire_schemas()` 改为只返回 core_tools。新增 `wire_schemas_subset()` 接受 `HashSet<String>` 返回匹配的工具 schema。

```rust
impl Toolbox {
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.core_tools.iter().chain(self.extra_tools.iter())
            .find(|t| t.name() == name).map(|t| t.as_ref())
    }

    pub fn wire_schemas_core(&self) -> Vec<Value> {
        self.core_tools.iter().map(|t| tool_schema(t.as_ref())).collect()
    }

    pub fn wire_schemas_subset(&self, names: &HashSet<String>) -> Vec<Value> {
        self.extra_tools.iter()
            .filter(|t| names.contains(t.name()))
            .map(|t| tool_schema(t.as_ref()))
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<&dyn Tool> {
        let q = query.to_lowercase();
        self.extra_tools.iter()
            .filter(|t| t.name().to_lowercase().contains(&q) || t.description().to_lowercase().contains(&q))
            .map(|t| t.as_ref())
            .collect()
    }

    /// 全量 schemas（用于兼容旧代码/测试）
    pub fn wire_schemas(&self) -> Vec<Value> {
        let mut schemas = self.wire_schemas_core();
        schemas.extend(self.extra_tools.iter().map(|t| tool_schema(t.as_ref())));
        schemas
    }
}

fn tool_schema(t: &dyn Tool) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": t.name(),
            "description": t.description(),
            "parameters": t.schema(),
        }
    })
}
```

- [ ] **Step 2: 改造 AgentLoop**

在 `AgentLoop` 添加 `loaded_extra_tools: HashSet<String>` 字段，并在 `build()` 中初始化：

```rust
pub struct AgentLoop {
    // ... 现有字段 ...
    /// 已通过 tool_search 加载的扩展工具名集合。这些工具会追加到 wire_schemas 中。
    loaded_extra_tools: HashSet<String>,
}
```

在 `build()` 中初始化：`loaded_extra_tools: HashSet::new()`

改造 `wire_schemas()` 方法（在 `process_turn` 中调用 `self.toolbox.wire_schemas()` 的地方）：
```rust
// 在 process_turn 中，构建 tools 数组的地方：
let tools = {
    let mut schemas = self.toolbox.wire_schemas_core();
    schemas.extend(self.toolbox.wire_schemas_subset(&self.loaded_extra_tools));
    schemas
};
```

- [ ] **Step 3: 测试验证**

```bash
cargo build 2>&1 | tail -3
# 验证所有旧测试通过（wire_schemas 全量返回兼容）
cargo test 2>&1 | grep -E "test result:" | grep -v "0 passed" | tail -3
git add src/tool/mod.rs src/agent.rs
git commit -m "refactor(tool): split Toolbox into core/extra tool sets

Separate tools into always-present core (~20) and searchable extra (~20).
Add Toolbox::search(), wire_schemas_core(), wire_schemas_subset() methods.
Add AgentLoop::loaded_extra_tools for dynamic tool loading.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: ToolSearch 工具 — 动态工具发现

**Files:**
- Create: `src/tool/tool_search.rs`
- Modify: `src/tool/mod.rs` (注册 `tool_search`)
- Modify: `src/agent.rs` (在 `dispatch_tool` 中处理 `tool_search` 的 session_meta_mark 自动加载)
- Test: 内联在 `src/tool/tool_search.rs` 的 `#[cfg(test)]`

**Interfaces:**
- Consumes: `Toolbox::search()`, `ToolOutput::session_meta_mark`
- Produces: `ToolSearch` (Tool), `AgentLoop.loaded_extra_tools` 自动加载

- [ ] **Step 1: 实现 ToolSearch 工具**

```rust
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::path::Path;

pub struct ToolSearch;

impl Tool for ToolSearch {
    fn name(&self) -> &str { "tool_search" }
    fn description(&self) -> &str {
        "Search for available tools that are not shown by default. \
         Provide a search query and matching tools will be loaded for this session. \
         Use this when you need a tool that isn't in the current tool list."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search term to match against tool names and descriptions." }
            },
            "required": ["query"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission { Permission::None }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let query = args.get("query").and_then(Value::as_str).unwrap_or_default().to_string();
        if query.is_empty() {
            return Ok(ToolOutput::err("tool_search requires `query`"));
        }
        // 搜索匹配工具
        let matches = crate::tool::ToolSearch::do_search(&query);
        if matches.is_empty() {
            return Ok(ToolOutput::ok(format!("no tools found matching: {query}")));
        }
        // 通过 session_meta_mark 把匹配的工具名传给 AgentLoop 自动加载
        let names: Vec<&str> = matches.iter().map(|t| t.name()).collect();
        let summary = format!(
            "Found {} tool(s): {}\n\nUse them directly by name — they are now available in this session.",
            names.len(), names.join(", ")
        );
        Ok(ToolOutput::ok(summary)
            .with_session_meta_mark(json!({ "tool_search_loaded": names })))
    }
}
```

**注意：** `ToolSearch::do_search` 需要访问 `Toolbox`。但 Tool 的 `run()` 方法只有 `ToolCtx` 没有 `Toolbox` 引用。这意味着 `tool_search` 不能直接调用 `Toolbox::search()`。

**解决方案：** 在 `ToolCtx` 中增加 `toolbox` 引用，或把 `tool_search` 的搜索逻辑放在 `AgentLoop::dispatch_tool` 中。

**推荐方案：** 在 `AgentLoop::dispatch_tool` 中特殊处理 `tool_search` 的返回——检查 `session_meta_mark` 中的 `tool_search_loaded` 键，通过 `self.toolbox.search()` 获取匹配工具名，加入 `self.loaded_extra_tools`。

```rust
// 在 dispatch_tool 中，处理 session_meta_mark 之后：
if name == "tool_search" {
    if let Some(mark) = &output.session_meta_mark {
        if let Some(names) = mark.get("tool_search_loaded").and_then(Value::as_array) {
            for n in names {
                if let Some(s) = n.as_str() {
                    self.loaded_extra_tools.insert(s.to_string());
                }
            }
        }
    }
}
```

- [ ] **Step 2: 测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_search_requires_query() {
        let out = ToolSearch.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("requires `query`"));
    }

    #[test]
    fn tool_search_permission_none() {
        assert!(matches!(ToolSearch.permission(&json!({}), Path::new(".")), Permission::None));
    }
}
```

- [ ] **Step 3: 注册 ToolSearch 并提交**

```bash
cargo build 2>&1 | tail -3
cargo test tool_search 2>&1 | tail -5
git add src/tool/tool_search.rs src/tool/mod.rs src/agent.rs
git commit -m "feat(tool_search): add dynamic tool discovery tool

Implement tool_search that searches the extra tool set and auto-loads
matching tools into the session via session_meta_mark.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 文档更新 + 集成验证

**Files:**
- Modify: `README.md` (工具表 40→43, 添加 3 个工具)
- Create: `docs/adr/0042-worktree-toolsearch.md` (架构决策记录)

- [ ] **Step 1: 更新 README 工具表**

工具计数 40→43，添加：
```
| `enter_worktree` | 创建 git worktree 隔离工作区 |
| `exit_worktree` | 退出 worktree（合并/保留/丢弃） |
| `tool_search` | 搜索并按需加载扩展工具 |
```

- [ ] **Step 2: 编写 ADR 0042**

创建 `docs/adr/0042-worktree-toolsearch.md`，记录：
- **决策：** Worktree 工具通过 `git worktree` 命令管理隔离分支 + 内存 `WorktreeStore`；ToolSearch 通过 `Toolbox` 核心/扩展工具分离 + `session_meta_mark` 自动加载
- **理由：** git worktree 是标准 Git 功能，无需额外工具；ToolSearch 延迟加载减少每次请求的 token 消耗
- **后果：** 核心工具约 20 个始终在线；扩展工具约 20 个需搜索后加载；`tool_search` 是唯一一个不在 wire_schemas 中的工具——它由 `dispatch_tool` 特殊处理
- **替代方案：** 全量发送（不采用，token 浪费）；仅文本搜索不自动加载（不采用，LLM 需要 schema 才能正确调用）

- [ ] **Step 3: 运行完整测试套件并提交**

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "test result:" | grep -v "0 passed" | tail -3
git add README.md docs/adr/0042-worktree-toolsearch.md
git commit -m "docs: add ADR 0042, update README for worktree/toolsearch tools

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 执行顺序

1. **Task 1** → Worktree 工具（`enter_worktree`/`exit_worktree`，独立）
2. **Task 2** → Toolbox 重构（核心/扩展分离，核心架构变更）
3. **Task 3** → ToolSearch 工具（依赖 Task 2）
4. **Task 4** → 文档更新 + ADR 0042