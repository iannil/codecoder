# MCP + LSP 内置工具实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 CodeCoder 补全 MCP 工具族（MCPTool / ListMcpResourcesTool / ReadMcpResourceTool）和 LSPTool，使其具备接入外部工具生态和代码智能分析的能力。

**Architecture:** 两个工具模块各自独立，遵循现有 `tool/net.rs` 和 `tool/dev.rs` 的单文件模式。MCP 模块实现 JSON-RPC 2.0 over stdio 客户端，管理 MCP 服务器生命周期；LSP 模块实现 LSP over stdio 客户端，管理语言服务器生命周期。两者均通过 `serde_json` 构建 JSON-RPC 消息，无需额外 RPC 框架。

**Tech Stack:** Rust, serde_json, lsp-types (LSP 数据结构), std::process (子进程管理)

---

## 全局约束

- 遵循 ADR 0018 的 `Tool` trait 接口：`name()` / `description()` / `schema()` / `permission()` / `run()`
- 权限模型：MCP 工具调用 = `Permission::Ask { key: "mcp_call" }`（可能修改外部状态）；LSP 只读操作 = `Permission::None`；LSP 写操作（如 codeAction） = `Permission::Ask`
- 错误处理：统一通过 `ToolOutput::err()` 返回错误消息，不 panic
- 测试：需包含单元测试和集成测试（可 mock JSON-RPC 响应）
- 配置：MCP 服务器配置通过 `codecoder.json` 加载（`mcp_servers` 字段）
- 新依赖：`lsp-types = "0.95"`（LSP 数据结构），`serde_json`（已有）

---

### Task 1: MCP 基础模块 — JSON-RPC 客户端 + 服务器生命周期管理

**Files:**
- Create: `src/tool/mcp.rs` (前 300 行)
- Modify: `src/tool/mod.rs` (添加 `pub mod mcp` 和注册)
- Test: 内联在 `src/tool/mcp.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Tool`, `ToolCtx`, `ToolOutput`, `Permission` trait/structs
- Produces: `McpClient` (struct), `McpServerConfig` (struct)

- [ ] **Step 1: 设计 MCP 数据结构**

在 `src/tool/mcp.rs` 顶部定义 JSON-RPC 2.0 和 MCP 协议所需的数据结构：

```rust
// JSON-RPC 2.0 消息
#[derive(Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String, // "2.0"
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<Value>,
}

// MCP 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpServerConfig {
    command: String,
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

// MCP 工具定义
#[derive(Deserialize)]
struct McpToolDef {
    name: String,
    description: String,
    #[serde(default)]
    input_schema: Value,
}
```

- [ ] **Step 2: 实现 McpClient**

```rust
/// 管理一个 MCP 服务器的子进程和 JSON-RPC 通信。
struct McpClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    initialized: bool,
}

impl McpClient {
    /// 启动 MCP 服务器子进程并建立连接。
    fn spawn(config: &McpServerConfig) -> anyhow::Result<Self> { ... }

    /// 发送 JSON-RPC 请求并等待响应。
    fn request(&mut self, method: &str, params: Option<Value>) -> anyhow::Result<Value> { ... }

    /// 发送 initialize 握手（MCP 协议第一步）。
    fn initialize(&mut self) -> anyhow::Result<Value> { ... }

    /// 列出所有可用工具（调用 tools/list）。
    fn list_tools(&mut self) -> anyhow::Result<Vec<McpToolDef>> { ... }

    /// 调用一个工具（调用 tools/call）。
    fn call_tool(&mut self, name: &str, args: Value) -> anyhow::Result<String> { ... }

    /// 列出所有资源（调用 resources/list）。
    fn list_resources(&mut self) -> anyhow::Result<Value> { ... }

    /// 读取资源（调用 resources/read）。
    fn read_resource(&mut self, uri: &str) -> anyhow::Result<String> { ... }

    /// 优雅关闭服务器。
    fn shutdown(&mut self) -> anyhow::Result<()> { ... }
}
```

关键实现细节：
- `spawn()`: 用 `std::process::Command` 启动子进程，管道 stdin/stdout
- `request()`: 发送 JSON-RPC 请求行（Content-Length header + JSON body），然后读取响应
- MCP 协议消息格式：`Content-Length: <N>\r\n\r\n<JSON body>`（与 LSP 相同的协议头格式）
- `next_id` 递增保证每个请求有唯一 ID

- [ ] **Step 3: 实现 MCP 服务器管理器**

```rust
use std::sync::Mutex;

/// 全局 MCP 客户端管理器（懒初始化）。
struct McpManager {
    clients: HashMap<String, McpClient>,
    configs: Vec<McpServerConfig>,
}

impl McpManager {
    fn from_config(root: &Path) -> Self { ... }
    fn ensure_initialized(&mut self) -> anyhow::Result<()> { ... }
    fn get_client(&mut self, name: &str) -> Option<&mut McpClient> { ... }
}
```

`from_config()` 从 `codecoder.json` 读取 `mcp_servers` 数组：
```json
{
  "mcp_servers": [
    { "name": "filesystem", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path"] }
  ]
}
```

- [ ] **Step 4: 注册模块并添加到 Toolbox**

在 `src/tool/mod.rs` 中：

```rust
pub mod mcp;  // 新增

// 在 Toolbox::builtin() 中添加：
Box::new(mcp::McpToolCall),
Box::new(mcp::McpListResources),
Box::new(mcp::McpReadResource),
```

- [ ] **Step 5: 单元测试（mock JSON-RPC 响应）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rpc_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "tools/list".into(),
            params: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["method"], "tools/list");
        assert_eq!(json["id"], 1);
    }

    #[test]
    fn mcp_server_config_deserialize() {
        let json = json!({"command": "npx", "args": ["-y", "server"]});
        let cfg: McpServerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.command, "npx");
    }

    #[test]
    fn parse_content_length_header() {
        let header = "Content-Length: 42\r\n\r\n";
        // 解析 Content-Length
        ...
    }
}
```

- [ ] **Step 6: 编译验证并提交**

```bash
cargo build 2>&1 | head -20
cargo test mcp 2>&1
git add src/tool/mcp.rs src/tool/mod.rs
git commit -m "feat(mcp): add MCP client and server lifecycle management

Implement JSON-RPC 2.0 client for MCP protocol over stdio, including
server spawning, initialization, and tool/resource listing.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: MCPToolCall — 动态调用 MCP 工具

**Files:**
- Modify: `src/tool/mcp.rs` (追加 ~200 行)

**Interfaces:**
- Consumes: `McpManager` (全局单例)
- Produces: `McpToolCall` 工具实现

- [ ] **Step 1: 实现 McpToolCall 工具**

```rust
pub struct McpToolCall;

impl Tool for McpToolCall {
    fn name(&self) -> &str { "mcp_call" }
    fn description(&self) -> &str {
        "Call a tool on an MCP server. MCP (Model Context Protocol) servers provide external capabilities like filesystem access, database queries, API integration, etc."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": { "type": "string", "description": "MCP server name" },
                "tool": { "type": "string", "description": "Tool name to call" },
                "arguments": { "type": "object", "description": "Tool arguments (key-value pairs)" }
            },
            "required": ["server", "tool"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "mcp_call".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let server = args.get("server").and_then(Value::as_str).unwrap_or_default();
        let tool = args.get("tool").and_then(Value::as_str).unwrap_or_default();
        let arguments = args.get("arguments").cloned().unwrap_or(json!({}));

        if server.is_empty() || tool.is_empty() {
            return Ok(ToolOutput::err("missing required args: server, tool"));
        }

        // 使用全局管理器获取 MCP 客户端
        let mut manager = MCP_MANAGER.lock().unwrap();
        match manager.call_tool(server, tool, arguments) {
            Ok(output) => Ok(ToolOutput::ok(output)),
            Err(e) => Ok(ToolOutput::err(format!("MCP call failed: {e}"))),
        }
    }
}
```

- [ ] **Step 2: 实现 McpListResources 工具**

```rust
pub struct McpListResources;

impl Tool for McpListResources {
    fn name(&self) -> &str { "mcp_list_resources" }
    fn description(&self) -> &str {
        "List available resources from configured MCP servers."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": { "type": "string", "description": "Optional server name to filter by" }
            }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let server = args.get("server").and_then(Value::as_str);
        let mut manager = MCP_MANAGER.lock().unwrap();
        match manager.list_resources(server) {
            Ok(resources) => Ok(ToolOutput::ok(resources)),
            Err(e) => Ok(ToolOutput::err(format!("list resources failed: {e}"))),
        }
    }
}
```

- [ ] **Step 3: 实现 McpReadResource 工具**

```rust
pub struct McpReadResource;

impl Tool for McpReadResource {
    fn name(&self) -> &str { "mcp_read_resource" }
    fn description(&self) -> &str {
        "Read the content of a resource from an MCP server."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": { "type": "string", "description": "MCP server name" },
                "uri": { "type": "string", "description": "Resource URI to read" }
            },
            "required": ["server", "uri"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let server = args.get("server").and_then(Value::as_str).unwrap_or_default();
        let uri = args.get("uri").and_then(Value::as_str).unwrap_or_default();
        // ...
    }
}
```

- [ ] **Step 4: 全局 MCP 管理器**

```rust
use std::sync::LazyLock;

static MCP_MANAGER: LazyLock<Mutex<McpManager>> = LazyLock::new(|| {
    Mutex::new(McpManager::from_config(&std::path::Path::new(".")))
});
```

注意：`LazyLock` 是 Rust 1.80+ 稳定特性。CodeCoder 使用 edition 2024，应支持。

- [ ] **Step 5: 编译测试并提交**

```bash
cargo build 2>&1 | head -20
cargo test mcp 2>&1
git add src/tool/mcp.rs
git commit -m "feat(mcp): add MCP tool call, list resources, and read resource tools

Implement mcp_call, mcp_list_resources, and mcp_read_resource tools
with lazy-initialized MCP server manager.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: LSP 模块 — JSON-RPC 传输 + 语言服务器生命周期

**Files:**
- Create: `src/tool/lsp.rs` (前 400 行)
- Modify: `Cargo.toml` (添加 `lsp-types` 依赖)
- Modify: `src/tool/mod.rs` (添加 `pub mod lsp` 和注册)
- Test: 内联在 `src/tool/lsp.rs` 的 `#[cfg(test)]`

**Interfaces:**
- Consumes: `Tool`, `ToolCtx`, `ToolOutput`, `Permission`
- Produces: `LspClient` (struct), `LspTool` (struct)

- [ ] **Step 1: 添加 lsp-types 依赖**

在 `Cargo.toml` 的 `[dependencies]` 中添加：

```toml
lsp-types = "0.95"
```

- [ ] **Step 2: 实现 LSP 客户端**

```rust
use lsp_types::*;

/// 管理一个语言服务器的子进程和 JSON-RPC 通信。
struct LspClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    server_capabilities: Option<ServerCapabilities>,
    root_uri: Url,
}

impl LspClient {
    /// 启动语言服务器进程。
    fn spawn(command: &str, args: &[String], root: &Path) -> anyhow::Result<Self> { ... }

    /// 发送请求并等待响应（阻塞）。
    fn request<R: DeserializeOwned>(&mut self, method: &str, params: impl Serialize) -> anyhow::Result<R> { ... }

    /// 发送通知（无需响应）。
    fn notify(&mut self, method: &str, params: impl Serialize) -> anyhow::Result<()> { ... }

    /// 执行 initialize 握手。
    fn initialize(&mut self, root_uri: &Url) -> anyhow::Result<ServerCapabilities> { ... }

    /// 发送 initialized 通知。
    fn initialized(&mut self) -> anyhow::Result<()> { ... }

    /// 打开文档（textDocument/didOpen）。
    fn did_open(&mut self, uri: &Url, text: &str, version: i32) -> anyhow::Result<()> { ... }

    // ---- LSP 操作 ----

    fn go_to_definition(&mut self, uri: &Url, line: u32, character: u32) -> anyhow::Result<Vec<Location>> { ... }
    fn find_references(&mut self, uri: &Url, line: u32, character: u32) -> anyhow::Result<Vec<Location>> { ... }
    fn hover(&mut self, uri: &Url, line: u32, character: u32) -> anyhow::Result<Option<HoverContents>> { ... }
    fn document_symbol(&mut self, uri: &Url) -> anyhow::Result<Vec<DocumentSymbol>> { ... }
    fn workspace_symbol(&mut self, query: &str) -> anyhow::Result<Vec<SymbolInformation>> { ... }
    fn go_to_implementation(&mut self, uri: &Url, line: u32, character: u32) -> anyhow::Result<Vec<Location>> { ... }

    /// 关闭服务器。
    fn shutdown(&mut self) -> anyhow::Result<()> { ... }
}
```

关键实现细节：
- LSP 消息格式：`Content-Length: <N>\r\n\r\n<JSON body>`（与 MCP 格式相同）
- `request()` 发送请求，然后从 stdout 读取响应（匹配 `id` 字段）
- 服务器发现：通过 `process_file` 或 `CODECODER_LSP_SERVERS` 环境变量配置

- [ ] **Step 3: 实现 LSP 服务器发现**

根据文件扩展名自动选择语言服务器：

```rust
fn detect_lsp_server(file_path: &str) -> Option<(&'static str, Vec<&'static str>)> {
    if file_path.ends_with(".rs") {
        Some(("rust-analyzer", vec![]))
    } else if file_path.ends_with(".py") {
        Some(("pylsp", vec![]))  // 或 pyright
    } else if file_path.ends_with(".js") || file_path.ends_with(".ts") || file_path.ends_with(".jsx") || file_path.ends_with(".tsx") {
        Some(("typescript-language-server", vec!["--stdio"]))
    } else if file_path.ends_with(".go") {
        Some(("gopls", vec![]))
    } else if file_path.ends_with(".c") || file_path.ends_with(".h") || file_path.ends_with(".cpp") || file_path.ends_with(".hpp") {
        Some(("clangd", vec![]))
    } else {
        None
    }
}
```

- [ ] **Step 4: 实现 LSP 服务器管理器**

```rust
struct LspManager {
    servers: HashMap<String, LspClient>, // key = server command name
}
```

- [ ] **Step 5: 单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_rust_server() {
        assert_eq!(
            detect_lsp_server("src/main.rs").map(|(cmd, _)| cmd),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn detect_python_server() {
        assert_eq!(
            detect_lsp_server("app.py").map(|(cmd, _)| cmd),
            Some("pylsp")
        );
    }

    #[test]
    fn unknown_extension_returns_none() {
        assert!(detect_lsp_server("readme.md").is_none());
    }

    #[test]
    fn content_length_parsing() {
        let header = "Content-Length: 42\r\n\r\n";
        // 验证解析
        assert_eq!(parse_content_length(header), Ok(42));
    }
}
```

- [ ] **Step 6: 编译验证并提交**

```bash
cargo build 2>&1 | head -20
cargo test lsp 2>&1
git add Cargo.toml src/tool/lsp.rs src/tool/mod.rs
git commit -m "feat(lsp): add LSP client with server lifecycle and JSON-RPC transport

Implement LSP JSON-RPC client over stdio with server lifecycle management
(initialize, initialized, shutdown, exit) and language server discovery
by file extension.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: LSPTool — 统一代码智能工具

**Files:**
- Modify: `src/tool/lsp.rs` (追加 ~250 行)

**Interfaces:**
- Consumes: `LspClient` (Task 3)
- Produces: `LspTool` 工具实现

- [ ] **Step 1: 实现 LspTool（统一入口，多操作）**

```rust
pub struct LspTool;

impl Tool for LspTool {
    fn name(&self) -> &str { "lsp" }
    fn description(&self) -> &str {
        "Query a language server for code intelligence. Supports operations: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol", "workspaceSymbol", "goToImplementation", "prepareCallHierarchy", "incomingCalls", "outgoingCalls"]
                },
                "filePath": { "type": "string", "description": "Path to the file (relative to project root)" },
                "line": { "type": "integer", "description": "Line number (1-based)" },
                "character": { "type": "integer", "description": "Character offset (1-based)" },
                "query": { "type": "string", "description": "Symbol name to search for (workspaceSymbol only)" }
            },
            "required": ["operation", "filePath"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None  // 所有 LSP 操作都是只读的
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let operation = args.get("operation").and_then(Value::as_str).unwrap_or_default();
        let file_path = args.get("filePath").and_then(Value::as_str).unwrap_or_default();
        let line = args.get("line").and_then(Value::as_u64).unwrap_or(1) as u32;
        let character = args.get("character").and_then(Value::as_u64).unwrap_or(1) as u32;
        let query = args.get("query").and_then(Value::as_str).unwrap_or_default();

        if operation.is_empty() || file_path.is_empty() {
            return Ok(ToolOutput::err("missing required args: operation, filePath"));
        }

        // 将相对路径转换为绝对路径
        let abs_path = ctx.root.join(file_path);
        let uri = Url::from_file_path(&abs_path).map_err(|_| anyhow::anyhow!("invalid path"))?;

        // 获取或创建 LSP 客户端
        let mut manager = LSP_MANAGER.lock().unwrap();
        let client = manager.get_or_spawn(&abs_path)?;

        let result = match operation {
            "goToDefinition" => format_goto(client.go_to_definition(&uri, line - 1, character - 1)?),
            "findReferences" => format_references(client.find_references(&uri, line - 1, character - 1)?),
            "hover" => format_hover(client.hover(&uri, line - 1, character - 1)?),
            "documentSymbol" => format_document_symbols(client.document_symbol(&uri)?),
            "workspaceSymbol" => format_workspace_symbols(client.workspace_symbol(query)?),
            "goToImplementation" => format_goto(client.go_to_implementation(&uri, line - 1, character - 1)?),
            _ => return Ok(ToolOutput::err(format!("unknown operation: {operation}"))),
        };

        Ok(ToolOutput::ok(result))
    }
}
```

关键设计：
- 统一 `lsp` 工具名，通过 `operation` 参数区分不同操作
- line/character 从 1-based 转为 0-based（LSP 内部使用 0-based）
- 懒初始化语言服务器，首次使用 `workspaceSymbol` 或文件操作时自动启动
- 结果格式化函数将 LSP 类型转换为人类可读文本

- [ ] **Step 2: 实现格式化函数**

```rust
fn format_goto(locations: Vec<Location>) -> String { ... }
fn format_references(locations: Vec<Location>) -> String { ... }
fn format_hover(contents: Option<HoverContents>) -> String { ... }
fn format_document_symbols(symbols: Vec<DocumentSymbol>) -> String { ... }
fn format_workspace_symbols(symbols: Vec<SymbolInformation>) -> String { ... }
```

- [ ] **Step 3: 编译测试并提交**

```bash
cargo build 2>&1 | head -20
cargo test lsp 2>&1
git add src/tool/lsp.rs
git commit -m "feat(lsp): add LSPTool for code intelligence

Implement unified lsp tool supporting goToDefinition, findReferences,
hover, documentSymbol, workspaceSymbol, and goToImplementation operations
with lazy server initialization.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 集成测试 + 文档更新

**Files:**
- Modify: `src/tool/lsp.rs` (添加集成测试)
- Modify: `src/tool/mcp.rs` (添加集成测试)
- Modify: `README.md` (工具表更新)
- Create: `docs/adr/0040-mcp-lsp-tools.md` (架构决策记录)

- [ ] **Step 1: 为 MCP 模块添加集成测试**

```rust
#[cfg(test)]
mod integration_tests {
    // 使用 mock stdin/stdout 模拟 MCP 服务器响应
    // 测试：工具列表解析、工具调用格式化、资源列表解析
}
```

- [ ] **Step 2: 为 LSP 模块添加集成测试**

```rust
#[cfg(test)]
mod integration_tests {
    // 使用 mock JSON-RPC 响应验证 LSP 请求/响应格式
    // 测试：所有操作类型的请求序列化、响应反序列化
}
```

- [ ] **Step 3: 更新 README.md 工具表**

在工具表中添加三行：
```
| `mcp_call` | 调用 MCP 服务器上的工具 |
| `mcp_list_resources` | 列出 MCP 服务器资源 |
| `mcp_read_resource` | 读取 MCP 资源内容 |
| `lsp` | 代码智能查询（定义跳转、引用查找、悬停提示等） |
```

- [ ] **Step 4: 编写 ADR 0040**

创建 `docs/adr/0040-mcp-lsp-tools.md`，记录：
- 决策：MCP 和 LSP 作为独立工具模块实现
- 理由：JSON-RPC 2.0 over stdio 是两者共同的传输协议，但生命周期和配置不同
- 后果：MCP 工具调用需要 ask 权限，LSP 只读操作无需权限
- 替代方案：使用 MCP SDK（未采用，避免额外依赖；JSON-RPC 足够简单）

- [ ] **Step 5: 运行完整测试套件**

```bash
cargo test 2>&1 | tail -20
git add -A
git commit -m "docs: add ADR 0040, update README for MCP/LSP tools

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 执行顺序

1. **Task 1** → MCP 基础模块（JSON-RPC 客户端 + 服务器生命周期）
2. **Task 2** → MCP 工具调用（mcp_call / mcp_list_resources / mcp_read_resource）
3. **Task 3** → LSP 基础模块（JSON-RPC 传输 + 服务器生命周期）
4. **Task 4** → LSPTool（统一代码智能工具）
5. **Task 5** → 集成测试 + 文档更新

每个任务完成后可独立编译和测试，互不阻塞。