# ADR 0042 — Worktree Isolation Tools and Tool Search

- **状态**: Accepted
- **日期**: 2026-08-08
- **关联**: ADR 0018 (Tool Architecture), ADR 0019 (Sub-agent Permissions), ADR 0040 (MCP/LSP Tools), ADR 0041 (Task/Cron/SendMessage)

## 背景

CodeCoder 的内置工具集（原 40 个）需要扩展两种新能力：

1. **工作区隔离（Worktree）**：agent 在开发过程中需要创建隔离的工作区，以便在不影响主工作区的情况下进行实验、并行开发或安全地执行高风险操作。之前的方案依赖于第三方工具（如 `archived/claude-code` 的 worktree 实现），但需要原生的工具支持。
2. **工具搜索（Tool Search）**：随着工具数量增长到 43 个，每次请求都发送全部工具 schema 的 token 开销越来越大。需要一种机制让 agent 按需发现和加载工具，只在需要时发送相关工具的 schema。

## 决策

### 1. Worktree 工具：`enter_worktree` / `exit_worktree`

- 通过 `git worktree` 命令创建和管理隔离分支。
- 内存 `WorktreeStore`（`LazyLock<Mutex<WorktreeStore>>`）记录 worktree 条目，包含 id、name、path、branch、base_branch、session_id、created_at。
- **`enter_worktree`**：创建新的 git worktree，自动生成分支名（`feat/<name>`），在 `.worktrees/<name>` 下创建工作目录，并将条目注册到 `WorktreeStore`。
- **`exit_worktree`**：支持三种操作：
  - `merge`：切回 base_branch，合并 worktree 分支，然后清理 worktree 和分支。
  - `keep`：保留 worktree 目录和分支，仅从 `WorktreeStore` 中移除记录。
  - `discard`：删除 worktree 目录和分支。
- 权限模型：两级 `Permission::Ask { key: "worktree" }`，需要用户确认。
- **非持久化**：`WorktreeStore` 仅在内存中存活——daemon 重启后元数据丢失，但 worktree 目录和 Git 分支持久存在。

### 2. Toolbox 重构：核心/扩展工具分离

`Toolbox` 结构体拆分为两个集合：

- **`core_tools`（20 个）**：始终可见的核心工具，包括 `read_file`、`write_file`、`edit_file`、`run_command`、`glob`、`grep`、`search_web`、`search_github`、`commit`、`diff`、`plan`、`milestone`、`memory`、`ask_user`、`confirm`、`agent`、`reason`、`review`、`use_skill`、`list_directory`。
- **`extra_tools`（23 个）**：可搜索的扩展工具，包括 `run_capability`、`generate_skill`、`generate_prompt`、`promote_prompt`、`generate_capability`、`reverse_api`、`generate_milestones`、3 个 MCP 工具、`lsp`、5 个 task 工具、3 个 cron 工具、`send_message`、`enter_worktree`、`exit_worktree`、`tool_search`。

新接口：
- `wire_schemas_core()`：仅返回核心工具的 schema（始终发送）。
- `wire_schemas_subset(&HashSet<String>)`：返回指定名称的扩展工具 schema。
- `search(query)`：在扩展工具中按名称和描述进行不区分大小写的子串匹配。

### 3. ToolSearch 工具：`tool_search`

- `tool_search` 工具本身在 `extra_tools` 列表中，但它的搜索逻辑在 `AgentLoop::dispatch_tool` 中特殊处理。
- 工作流程：
  1. LLM 调用 `tool_search` 工具，传入 `query` 参数。
  2. `ToolSearch::run` 返回 `ToolOutput` 并设置 `session_meta_mark = { "tool_search_query": query }`。
  3. `AgentLoop::dispatch_tool` 检测到 `name == "tool_search"`，从 `session_meta_mark` 中读取查询，调用 `toolbox.search(query)`。
  4. 匹配的工具名称被加入 `AgentLoop.loaded_extra_tools: HashSet<String>`。
  5. 后续的 LLM 请求中，`tools` 数组包含 `core_tools` + `loaded_extra_tools` 对应的 schema。

## 理由

- **git worktree 是标准 Git 功能**：不需要额外工具或依赖，`git worktree` 是 Git 2.5+ 的内置命令，在所有主流平台上可用。
- **核心/扩展分离减少 token 消耗**：每次请求约 20 个核心工具（约 20 个 schema）vs. 全部 43 个工具。Agent 可以根据任务需要按需加载扩展工具，大幅减少 LLM 请求的 token 消耗。
- **`session_meta_mark` 侧通道**：工具 `run` 方法没有 `Toolbox` 引用，但 `dispatch_tool` 有。通过 `ToolOutput.session_meta_mark` 传递查询参数，避免了将 `Toolbox` 引用注入 `ToolCtx` 的架构变更。
- **`tool_search` 在 extra_tools 中**：虽然 `tool_search` 本身是搜索工具，但它放在 extra_tools 中意味着它也需要被搜索到才能使用。这是有意为之——LLM 的 system prompt 会告知 agent `tool_search` 的存在，确保 agent 知道如何首次加载它。

## 后果

### 正面
- 工作区隔离使 agent 可以安全地进行实验性开发，不影响主工作区。
- 核心/扩展分离减少了每次 LLM 请求的 token 消耗（约 50% 的 schema 减少）。
- ToolSearch 的动态加载机制使 agent 可以按需发现工具，无需预先知道所有工具名称。
- 工具总数从 40 增加到 43（新增 3 个工具）。

### 负面
- `tool_search` 本身是 extra_tools 的一员——system prompt 必须明确告知 agent 它的存在，否则 agent 无法发现它。
- `WorktreeStore` 是内存唯一的——daemon 重启后元数据丢失，但 worktree 目录和 Git 分支持久存在，可通过 `git worktree list` 手动恢复。
- `loaded_extra_tools` 是会话级别的——跨 session 不会保留已加载的扩展工具列表。
- `exit_worktree` 当前只操作最新的 worktree（`store.list().last()`），不支持按 id 指定。

### 限制与缓解
- system prompt 中包含 `tool_search` 的说明，确保 agent 知道首次如何使用它。
- 用户可以通过 system prompt 或 skill 告知 agent 预加载某些扩展工具。
- Worktree 的持久性由 Git 保证——即使 daemon 重启，`.worktrees/` 目录和 Git 分支仍然存在。

## 替代方案

### 全量工具 schema 发送
- **未采用**原因：每次请求发送 43 个工具 schema 的 token 开销太大。核心工具（~20 个）足够覆盖大部分日常操作，扩展工具按需加载更高效。

### ToolSearch 作为核心工具
- **未采用**原因：`tool_search` 放在 extra_tools 中避免了发任何无关 schema，但需要 system prompt 提示（推荐做法）。如果未来发现 agent 总是无法找到 `tool_search`，可以将其移到 core_tools。

### 独立的工具注册表服务
- **未采用**原因：`toolbox.search()` 已经足够简单高效，不需要引入外部服务或复杂的注册表机制。

### 文件持久化 WorktreeStore
- **未采用**原因：Worktree 元数据（id、session_id）是会话相关的，持久化价值有限。Git 分支和 worktree 目录本身已经持久存在。

## 实现

### 新增文件

```
src/tool/worktree.rs         — WorktreeEntry 结构体 + WorktreeStore + EnterWorktree/ExitWorktree 工具
src/tool/tool_search.rs      — ToolSearch 工具（搜索逻辑在 dispatch_tool 中特殊处理）
```

### 修改文件

```
src/tool/mod.rs              — Toolbox 重构为核心/extra 分离，新增 search() 方法；工具计数 40→43
src/agent.rs                 — loaded_extra_tools 字段、wire_schemas 改用 core+subset、dispatch_tool 特殊处理 tool_search
```

### 工具计数

内置工具：40 → 43（新增 `enter_worktree`/`exit_worktree`/`tool_search`）