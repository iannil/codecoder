# Worktree + ToolSearch 工具设计

> 为 CodeCoder 补全 P2 工作流工具：Worktree 隔离（`enter_worktree` / `exit_worktree`）和 ToolSearch 动态工具发现（`tool_search`），共 3 个工具。

## 1. Worktree 工具 — `enter_worktree` / `exit_worktree`

### 工具定义

2 个工具：`enter_worktree` / `exit_worktree`

### 数据结构

```rust
struct WorktreeEntry {
    id: u64,
    name: String,
    path: PathBuf,          // 完整路径，如 .worktrees/feat-x/
    branch: String,         // 分支名，如 feat/x
    base_branch: String,    // 基于哪个分支创建，如 master
    session_id: String,     // 隔离的 session id
    created_at: SystemTime,
}

static WORKTREE_STORE: LazyLock<Mutex<WorktreeStore>> = ...;
```

### 存储

`Mutex<Vec<WorktreeEntry>>` 全局单例，内存中。不持久化——daemon 重启后 worktree 目录和分支仍然存在，但管理元数据丢失。

### `enter_worktree` 工具

**参数：**
- `name`(可选) — worktree 目录名，省略则随机生成
- `base_branch`(可选) — 基于哪个分支创建，默认 `master`

**流程：**
1. 生成唯一 name（若未提供）
2. 运行 `git worktree add .worktrees/<name> -b feat/<name> <base_branch>`
3. 记录 WorktreeEntry（路径、分支、session_id、创建时间）
4. 返回 `{ "path": "<path>", "branch": "<branch>" }`

**权限：** `Permission::Ask { key: "worktree" }`

### `exit_worktree` 工具

**参数：**
- `action`(必填) — `merge` | `keep` | `discard`

**流程：**
- `merge`：切回主分支，merge worktree 分支，删除 worktree 和分支，返回 merge 结果
- `keep`：保留 worktree 和分支，仅清理内存记录，返回确认消息
- `discard`：`git worktree remove .worktrees/<name>` + `git branch -D feat/<name>`，返回确认消息

**权限：** `Permission::Ask { key: "worktree" }`

### 错误处理

- `enter_worktree` 时 worktree 已存在 → 返回错误
- `exit_worktree` 时未找到活跃 worktree → 返回错误
- `exit_worktree` 时 `merge` 但未提交 → 使用 `git merge --no-ff --allow-unrelated-histories` 或 `git stash` 先处理

---

## 2. ToolSearch — 动态工具发现

### 核心思想

不再每次 LLM 请求发送全部 40 个工具的 schema。改为：

1. **核心工具集**（~20 个最常用工具）始终在 `tools` 数组中
2. **扩展工具集**（~20 个不太常用的工具）按需加载
3. `tool_search` 工具让 agent 搜索并加载扩展工具到当前会话

### 核心工具集

始终在线的工具（通过 `Toolbox::wire_schemas_core()` 返回）：
```
read_file, write_file, edit_file, run_command, glob, grep, diff,
search_web, search_github, list_directory, use_skill, ask_user, confirm,
agent, plan, milestone, memory, reason, commit, review
```

### 扩展工具集

按需加载的工具（通过 `Toolbox::search()` 搜索）：
```
mcp_call_tool, mcp_list_resources, mcp_read_resource, lsp,
task_create, task_get, task_list, task_update, task_stop,
cron_create, cron_delete, cron_list, send_message,
generate_skill, generate_prompt, promote_prompt, generate_capability,
generate_milestones, run_capability, reverse_api, enter_worktree, exit_worktree
```

### `tool_search` 工具

**参数：**
- `query`(必填) — 搜索关键词，匹配工具名或描述

**流程：**
1. 在扩展工具集中搜索匹配 `query` 的工具
2. 把匹配的工具名加入 `loaded_extra_tools: HashSet<String>`（会话内，AgentLoop 字段）
3. 返回匹配的工具列表（含完整 schema）

**权限：** `Permission::None`

### 实现变更

**1. `Toolbox` 新增：**

```rust
impl Toolbox {
    /// 返回核心工具集的 wire schemas（始终在线的工具）
    pub fn wire_schemas_core(&self) -> Vec<Value> { ... }

    /// 在扩展工具集中搜索匹配的工具
    pub fn search(&self, query: &str) -> Vec<&dyn Tool> { ... }

    /// 返回特定工具子集的 wire schemas
    pub fn wire_schemas_subset(&self, names: &HashSet<String>) -> Vec<Value> { ... }
}
```

需要 `Toolbox` 区分核心/扩展工具。可以在 `Toolbox::builtin()` 中给每个工具标记 `is_core` 属性，或维护两个独立的 `Vec<Box<dyn Tool>>` 列表。

**推荐方案：** `Toolbox` 内部维护两个 `Vec`：
```rust
struct Toolbox {
    core_tools: Vec<Box<dyn Tool>>,
    extra_tools: Vec<Box<dyn Tool>>,
}
```

**2. `AgentLoop` 改造：**

```rust
pub struct AgentLoop {
    // ... 现有字段 ...
    /// 已加载的扩展工具名集合
    loaded_extra_tools: HashSet<String>,
}
```

`wire_schemas()` 改为：
```rust
fn wire_schemas(&self) -> Vec<Value> {
    let mut schemas = self.toolbox.wire_schemas_core();
    schemas.extend(self.toolbox.wire_schemas_subset(&self.loaded_extra_tools));
    schemas
}
```

**3. `tool_search` 工具实现：**

```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
    if query.is_empty() {
        return Ok(ToolOutput::err("tool_search requires `query`"));
    }
    // 搜索扩展工具集
    let matches = self.toolbox.search(query);
    // 记录匹配的工具名 → session_meta_mark 让 AgentLoop 加载
    // 通过 ToolOutput::session_meta_mark 传递匹配的工具名列表
    ...
}
```

**关键设计问题：** `tool_search` 的 `run()` 如何把匹配结果传给 `AgentLoop` 加载到 `loaded_extra_tools`？

**方案 A（推荐）：** `tool_search` 通过 `ToolOutput::session_meta_mark` 返回匹配的工具名，`AgentLoop::dispatch_tool` 在 tool 返回后检查 `session_meta_mark`，把匹配的工具名加入 `loaded_extra_tools`。

**方案 B：** `tool_search` 直接通过 `unsafe` 或 `Arc<Mutex<>>` 修改 `AgentLoop` 的状态（不推荐，破坏封装）。

**方案 C：** `tool_search` 返回匹配的工具名列表文本，agent 在下一次 request 时自行决定是否使用这些工具（不自动加载，依赖 agent 的推理能力）。**最简洁，但需要 agent 理解"这些工具现在可用"。**

**推荐方案 C 的变体：** `tool_search` 返回匹配的工具名 + schema，agent 可以在后续调用中直接使用这些工具名。`Toolbox::get()` 始终能找到所有工具（不分核心/扩展），所以 agent 只要知道工具名就能调用。`wire_schemas()` 默认只返回核心集 + 已加载的扩展集，但 `get()` 不受限。这样 agent 只需 `tool_search` 找到工具名，后续直接调用即可。

---

## 实现计划

### 依赖关系

```
Task 1: Worktree 工具 ─── 独立
Task 2: Toolbox 改造 ─── 独立（Toolbox 内部区分核心/扩展）
Task 3: ToolSearch 工具 ─── 依赖 Task 2
```

### 文件变更

| 文件 | 变更 |
|------|------|
| `src/tool/worktree.rs` | 新建，2 个工具 |
| `src/tool/tool_search.rs` | 新建，1 个工具 |
| `src/tool/mod.rs` | Toolbox 重构（core/extra 分离）+ 注册新工具 |
| `src/agent.rs` | 添加 `loaded_extra_tools` 字段，改造 `wire_schemas` |
| `README.md` | 工具表更新（40→43） |
| `docs/adr/0042-worktree-toolsearch.md` | 新建 ADR |