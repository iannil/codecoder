# ADR 0040 — MCP/LSP 工具模块：JSON-RPC 2.0 over stdio 集成

- **状态**: Accepted
- **日期**: 2026-08-07
- **关联**: ADR 0018(Tool trait + 注册表)、ADR 0037(工具输出截断)、`CONTEXT.md`(领域术语)

## 背景

CodeCoder 需要与外部工具和语言服务进行交互。两类需求：

1. **MCP (Model Context Protocol)**：调用外部 MCP 服务器暴露的工具和资源（如文件系统、数据库、API 网关等）。MCP 是 AI 优先的协议，每个服务器可注册多个工具和资源，由 LLM 按需调用。

2. **LSP (Language Server Protocol)**：对代码文件执行智能查询——跳转到定义、查找引用、悬停提示、文档符号、工作区符号搜索、实现跳转。LSP 是编辑器优先的协议，每个语言服务器对应一种编程语言，由文件扩展名自动探测。

两者虽然底层传输格式相同（JSON-RPC 2.0 over stdio with `Content-Length` header），但生命周期管理、配置方式、使用场景和权限模型差异显著，不宜合并为单一抽象。

## 决策

MCP 和 LSP 作为独立的工具模块实现，共享 JSON-RPC 2.0 framing 辅助函数，但通过不同的 Tool 结构体暴露给 agent。

### 模块结构

```
src/tool/
  mcp.rs    — MCP 客户端: McpManager / McpClient / McpToolCall / McpListResources / McpReadResource
  lsp.rs    — LSP 客户端: LspManager / LspClient / LspTool
```

### 生命周期

- MCP 和 LSP 各自持有全局 `LazyLock<Mutex<Manager>>` 单例，按需启动子进程。
- MCP 服务器在 `codecoder.json` 的 `mcp_servers` 数组中配置，按名称（`server` 参数）按需启动。
- LSP 服务器从文件扩展名自动探测（`.rs` → `rust-analyzer`，`.py` → `pylsp`，`.js/.ts/.jsx/.tsx` → `typescript-language-server`，`.go` → `gopls`，`.c/.h/.cpp/.hpp` → `clangd`），`workspace_symbol` 操作则从项目 manifest 推断（`Cargo.toml` / `package.json` / `go.mod` / `pyproject.toml`）。
- `workspace_symbol` 使用项目 manifest 探测，无需文件路径，但需要项目根目录存在对应 manifest。
- 两个模块均在 `Drop` 中实现子进程清理（shutdown→kill→wait）。

### 工具列表

| 工具名 | 权限 | 功能 |
|--------|------|------|
| `mcp_call_tool` | `Permission::Ask` | 调用 MCP 服务器上的工具。需 ask 权限，因为可能执行任意操作。 |
| `mcp_list_resources` | `Permission::None` | 列出 MCP 服务器暴露的资源（只读）。 |
| `mcp_read_resource` | `Permission::None` | 按 URI 读取 MCP 资源内容（只读）。 |
| `lsp` | `Permission::None` | 执行 LSP 操作（所有操作只读）。 |

### 消息格式

两者均使用 `Content-Length` header framing：

```
Content-Length: <N>\r\n\r\n<JSON body>
```

与 LSP 规范完全一致，MCP 也采用相同传输层。

### 权限模型

- `mcp_call_tool` 需要 `Permission::Ask`，因为 MCP 工具可能执行写操作（数据库写入、文件创建等）。
- `mcp_list_resources` 和 `mcp_read_resource` 是 `Permission::None`（只读）。
- `lsp` 所有操作是 `Permission::None`（纯只读查询）。

## 后果

### 正面

- 各自独立演化，互不干扰。
- 权限粒度清晰：MCP 工具调用需确认，LSP 和 MCP 只读操作无需确认。
- 自动从项目配置/文件扩展名推断服务器，零配置即可使用。
- 共享 framing 实现，代码复用（`read_framed` / `write_framed` 功能相同，但各自独立复制以避免模块耦合）。
- 服务器生命周期按需启动，只在使用时创建子进程。
- `workspace_symbol` 通过项目 manifest 探测工作区类型，无需文件路径参数。

### 代价

- 两份 framing 代码拷贝（`mcp.rs` 和 `lsp.rs` 各有一套 `read_framed`/`write_framed`），避免跨模块依赖。
- 全局 `Mutex<Manager>` 单例意味着同一时间只有一个 tool turn 能访问 MCP/LSP 服务器（这在当前串行工具执行模型中不是问题）。
- 子进程清理在 `Drop` 中执行，若进程 panic 则可能绕过后备清理（但主流路径均覆盖）。

### 不做

- 不嵌入 MCP SDK（避免额外依赖；JSON-RPC 2.0 足够简单，直接序列化/反序列化）。
- 不嵌入 LSP SDK（`lsp-types` crate 提供类型定义，传输层自行实现）。
- 不实现 MCP 服务器端（本仓库只作为客户端消费 MCP 服务）。
- 不实现 LSP 服务器端（本仓库只作为客户端消费 LSP 服务）。
- 不实现 MCP 的 `resources/subscribe` 推送（当前只做一次读取）。
- `workspace_symbol` 暂不支持通过 `file_path` 推断语言（当前只支持通过 manifest 探测）。

## 替代方案

### 使用 MCP SDK

未采用。MCP 协议很轻量（initialize / tools/list / tools/call / resources/list / resources/read 五个方法），JSON-RPC 2.0 的序列化/反序列化用 `serde_json` 直接处理，无需额外 SDK 依赖。

### 嵌入 LSP 服务器

未采用。LSP 服务器是语言特定的（rust-analyzer 用 Rust 编写，pylsp 用 Python 编写），以子进程形式运行。嵌入意味着需要将每个语言服务器链接为库，在 Rust 生态中不可行。

### 合并 MCP 和 LSP 为统一「JSON-RPC 工具」

未采用。虽然传输层相同，但生命周期（MCP 按配置名启动，LSP 按扩展名自动探测）、配置方式（MCP 在 `codecoder.json` 中声明，LSP 内置映射）、权限模型（MCP 工具调用需确认，LSP 全部只读）差异太大，合并反而增加复杂度。

### 使用 `lsp-types` 和 `tower-lsp`

`tower-lsp` 是 LSP 服务器框架，不是客户端库。本仓库只做客户端，`lsp-types` 提供类型定义已足够。