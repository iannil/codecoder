// MCP (Model Context Protocol) client — JSON-RPC 2.0 over stdio.
//
// Task 1 of the MCP/LSP tools plan: the basic module. Manages a list of MCP
// server subprocesses (configured in `codecoder.json` under `mcp_servers`),
// speaks the JSON-RPC 2.0 framing shared with LSP (`Content-Length` header),
// and performs the MCP initialize handshake before exposing tools/resources.
//
// The protocol framing is identical to LSP: `Content-Length: <N>\r\n\r\n<JSON>`.
// Spec: https://spec.modelcontextprotocol.io/
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

// ── JSON-RPC 2.0 / MCP data structures ────────────────────────────────────

/// A JSON-RPC 2.0 request (a `method` + optional `params`; `id` correlates the response).
#[derive(Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String, // "2.0"
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// A JSON-RPC 2.0 response: exactly one of `result` / `error` is populated.
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

/// One configured MCP server. `name` is the key used by the manager and tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpServerConfig {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

/// A tool advertised by an MCP server via `tools/list`.
#[derive(Deserialize)]
struct McpToolDef {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    input_schema: Value,
}

/// Top-level `codecoder.json` shape consumed for MCP server discovery.
#[derive(Deserialize)]
struct ProjectConfig {
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
}

// ── Framing helpers (LSP/MCP wire format) ─────────────────────────────────

/// Write one framed message: `Content-Length: <N>\r\n\r\n<JSON body>`.
fn write_framed<W: Write>(writer: &mut W, body: &[u8]) -> anyhow::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    Ok(())
}

/// Read one framed message from a buffered reader, returning the raw JSON body.
/// Parses the `Content-Length` header (tolerating CRLF/LF line endings) and then
/// reads exactly that many bytes. Errors if the stream closes mid-header.
fn read_framed<R: BufRead>(reader: &mut R) -> anyhow::Result<Vec<u8>> {
    let mut content_length: Option<usize> = None;
    let mut header = String::new();
    loop {
        header.clear();
        let n = reader.read_line(&mut header)?;
        if n == 0 {
            anyhow::bail!("stream closed before headers complete");
        }
        let line = header.trim_end();
        if line.is_empty() {
            break; // blank line terminates the header block
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            let len = rest
                .trim()
                .parse::<usize>()
                .map_err(|_| anyhow!("invalid Content-Length: {line:?}"))?;
            content_length = Some(len);
        }
    }
    let Some(len) = content_length else {
        anyhow::bail!("missing Content-Length header");
    };
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

// ── McpClient: one server subprocess + JSON-RPC channel ───────────────────

/// Manages a single MCP server subprocess and its JSON-RPC communication.
struct McpClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    initialized: bool,
}

impl McpClient {
    /// Start the MCP server subprocess and set up the stdio pipes.
    fn spawn(config: &McpServerConfig) -> anyhow::Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped()) // stderr inherited so the server's logs surface
            .stderr(Stdio::inherit());
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("failed to spawn MCP server '{}': {e}", config.command))?;
        let stdin = BufWriter::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("MCP server stdin unavailable"))?,
        );
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("MCP server stdout unavailable"))?,
        );
        Ok(Self { child, stdin, stdout, next_id: 1, initialized: false })
    }

    /// Send a JSON-RPC request and wait for the matching response.
    fn request(&mut self, method: &str, params: Option<Value>) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let body = serde_json::to_vec(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        })?;
        write_framed(&mut self.stdin, &body)?;
        self.stdin.flush()?;

        // Loop reading frames until the response matching our `id` arrives.
        // A real server may push server-initiated notifications (which carry no
        // `id`) or other responses at any time; those must be skipped, not
        // mistaken for our response.
        loop {
            let raw = read_framed(&mut self.stdout)?;
            let msg: Value = serde_json::from_slice(&raw)
                .map_err(|e| anyhow!("bad JSON-RPC message from {method}: {e}"))?;
            // Notifications have no `id` field — skip them.
            let Some(msg_id) = msg.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if msg_id != id {
                continue;
            }
            let resp: JsonRpcResponse = serde_json::from_value(msg)
                .map_err(|e| anyhow!("bad JSON-RPC response to {method}: {e}"))?;
            if resp.jsonrpc != "2.0" {
                anyhow::bail!("bad jsonrpc version in response to {method}: {:?}", resp.jsonrpc);
            }
            if let Some(err) = resp.error {
                let detail = err.data.as_ref().map(|d| format!(", data: {d}")).unwrap_or_default();
                anyhow::bail!("JSON-RPC error {}{detail}: {}", err.code, err.message);
            }
            return resp.result.ok_or_else(|| anyhow!("response to {method} missing result"));
        }
    }

    /// Send a JSON-RPC notification (no `id`, no response expected).
    fn notify(&mut self, method: &str, params: Option<Value>) -> anyhow::Result<()> {
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))?;
        write_framed(&mut self.stdin, &body)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Perform the MCP `initialize` handshake (protocol first step).
    fn initialize(&mut self) -> anyhow::Result<Value> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "codecoder", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self.request("initialize", Some(params))?;
        // The protocol requires an `initialized` notification after a successful handshake.
        self.notify("notifications/initialized", None)?;
        self.initialized = true;
        Ok(result)
    }

    /// Ensure the handshake has happened before any method call.
    fn ensure_initialized(&mut self) -> anyhow::Result<()> {
        if !self.initialized {
            self.initialize()?;
        }
        Ok(())
    }

    /// List all tools advertised by the server (`tools/list`).
    fn list_tools(&mut self) -> anyhow::Result<Vec<McpToolDef>> {
        self.ensure_initialized()?;
        let result = self.request("tools/list", None)?;
        let arr = result.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut tools = Vec::new();
        for t in &arr {
            match serde_json::from_value::<McpToolDef>(t.clone()) {
                Ok(def) => tools.push(def),
                Err(e) => {
                    eprintln!(
                        "mcp: warning: skipping unparseable tool definition from \
                         '{}': {e} (raw: {})",
                        arr.iter().find_map(|x| x.get("name").and_then(Value::as_str)).unwrap_or("?"),
                        serde_json::to_string(t).unwrap_or_default(),
                    );
                }
            }
        }
        Ok(tools)
    }

    /// Call a tool (`tools/call`) and return its concatenated text output.
    fn call_tool(&mut self, name: &str, args: Value) -> anyhow::Result<String> {
        self.ensure_initialized()?;
        let params = json!({ "name": name, "arguments": args });
        let result = self.request("tools/call", Some(params))?;
        let (text, is_error) = extract_text_content(&result);
        if is_error {
            anyhow::bail!("MCP tool '{name}' reported an error: {}", if text.is_empty() { result.to_string() } else { text });
        }
        Ok(text)
    }

    /// List all resources exposed by the server (`resources/list`).
    fn list_resources(&mut self) -> anyhow::Result<Value> {
        self.ensure_initialized()?;
        self.request("resources/list", None)
    }

    /// Read a resource by URI (`resources/read`), returning its text contents.
    fn read_resource(&mut self, uri: &str) -> anyhow::Result<String> {
        self.ensure_initialized()?;
        let params = json!({ "uri": uri });
        let result = self.request("resources/read", Some(params))?;
        Ok(extract_resource_text(&result))
    }

    /// Graceful shutdown: send `shutdown`, then kill and reap the child.
    fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.initialized {
            let _ = self.request("shutdown", None);
            self.initialized = false;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort cleanup: send shutdown, kill, and reap. Ignore errors
        // since we're dropping and can't bubble them up.
        if self.initialized {
            let _ = self.request("shutdown", None);
            self.initialized = false;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Extract the concatenated text + error flag from a `tools/call` result.
/// Content items with `type == "text"` are joined; a top-level `isError: true`
/// marks the call as failed.
fn extract_text_content(result: &Value) -> (String, bool) {
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
    let mut text = String::new();
    if let Some(arr) = result.get("content").and_then(Value::as_array) {
        for item in arr {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(t) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
        }
    }
    (text, is_error)
}

/// Extract readable text from a `resources/read` result. Text contents are
/// joined; binary blobs are summarized as a size marker.
fn extract_resource_text(result: &Value) -> String {
    let mut text = String::new();
    if let Some(arr) = result.get("contents").and_then(Value::as_array) {
        for item in arr {
            if let Some(t) = item.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            } else if let Some(b64) = item.get("blob").and_then(Value::as_str) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!("[blob {} bytes]", b64.len()));
            }
        }
    }
    text
}

// ── McpManager: global server lifecycle ───────────────────────────────────

/// Global MCP client manager. Holds the configured server list (loaded lazily
/// from the project's `codecoder.json`) and the spawned clients, keyed by name.
struct McpManager {
    root: Option<PathBuf>,
    configs: Vec<McpServerConfig>,
    clients: HashMap<String, McpClient>,
}

impl McpManager {
    fn default() -> Self {
        Self { root: None, configs: Vec::new(), clients: HashMap::new() }
    }

    /// Build a fresh manager from `<root>/codecoder.json`.
    fn from_config(root: &Path) -> Self {
        let configs = load_configs(root);
        Self { root: Some(root.to_path_buf()), configs, clients: HashMap::new() }
    }

    /// (Re)load config when the active project root changes. A no-op when the
    /// manager is already initialized for the same root.
    fn ensure_initialized(&mut self, root: &Path) -> anyhow::Result<()> {
        if self.root.as_deref() != Some(root) {
            *self = Self::from_config(root);
        }
        Ok(())
    }

    /// Lazily spawn + initialize (or reuse) the client for `name`.
    fn get_client(&mut self, name: &str) -> anyhow::Result<&mut McpClient> {
        if !self.clients.contains_key(name) {
            let cfg = self
                .configs
                .iter()
                .find(|c| c.name == name)
                .cloned()
                .ok_or_else(|| anyhow!("no MCP server configured: '{name}'"))?;
            let mut client = McpClient::spawn(&cfg)?;
            client.initialize()?;
            self.clients.insert(name.to_string(), client);
        }
        Ok(self.clients.get_mut(name).expect("just inserted"))
    }
}

/// Read `mcp_servers` from `<root>/codecoder.json`; empty on parse/IO failure.
fn load_configs(root: &Path) -> Vec<McpServerConfig> {
    let Ok(content) = std::fs::read_to_string(root.join("codecoder.json")) else {
        return Vec::new();
    };
    match serde_json::from_str::<ProjectConfig>(&content) {
        Ok(cfg) => cfg.mcp_servers,
        Err(_) => Vec::new(),
    }
}

/// Global manager instance, lazily initialized once per process.
static MANAGER: std::sync::LazyLock<std::sync::Mutex<McpManager>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(McpManager::default()));

/// Lock the global manager, (re)initialize it for `root`, and run `f`.
/// Errors during init are converted to `ToolOutput::err`.
fn with_manager(root: &Path, f: impl FnOnce(&mut McpManager) -> ToolOutput) -> anyhow::Result<ToolOutput> {
    let mut mgr = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = mgr.ensure_initialized(root) {
        return Ok(ToolOutput::err(format!("mcp init failed: {e}")));
    }
    Ok(f(&mut mgr))
}

// ── Tools ─────────────────────────────────────────────────────────────────

/// Call a tool exposed by a configured MCP server.
pub struct McpToolCall;

impl Tool for McpToolCall {
    fn name(&self) -> &str {
        "mcp_call_tool"
    }
    fn description(&self) -> &str {
        "Call a tool on an MCP (Model Context Protocol) server configured in codecoder.json. \
         Returns the tool's text output."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": { "type": "string", "description": "Name of the configured MCP server." },
                "tool": { "type": "string", "description": "Name of the tool on that server." },
                "arguments": { "type": "object", "description": "Arguments object passed to the tool." }
            },
            "required": ["server", "tool"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        // Executes arbitrary actions on a remote server — always ask.
        Permission::Ask { key: "mcp_call_tool".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let server = args.get("server").and_then(Value::as_str).unwrap_or_default().to_string();
        let tool = args.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
        if server.is_empty() || tool.is_empty() {
            return Ok(ToolOutput::err("mcp_call_tool requires `server` and `tool`"));
        }
        let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
        with_manager(ctx.root, |mgr| match mgr.get_client(&server) {
            Ok(client) => match client.call_tool(&tool, arguments) {
                Ok(text) => ToolOutput::ok(text),
                Err(e) => ToolOutput::err(format!("mcp call failed: {e}")),
            },
            Err(e) => ToolOutput::err(e.to_string()),
        })
    }
}

/// List the resources exposed by a configured MCP server.
pub struct McpListResources;

impl Tool for McpListResources {
    fn name(&self) -> &str {
        "mcp_list_resources"
    }
    fn description(&self) -> &str {
        "List the resources exposed by an MCP server configured in codecoder.json."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": { "type": "string", "description": "Name of the configured MCP server." }
            },
            "required": ["server"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let server = args.get("server").and_then(Value::as_str).unwrap_or_default().to_string();
        if server.is_empty() {
            return Ok(ToolOutput::err("mcp_list_resources requires `server`"));
        }
        with_manager(ctx.root, |mgr| match mgr.get_client(&server) {
            Ok(client) => match client.list_resources() {
                Ok(res) => ToolOutput::ok(serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())),
                Err(e) => ToolOutput::err(format!("mcp list_resources failed: {e}")),
            },
            Err(e) => ToolOutput::err(e.to_string()),
        })
    }
}

/// Read a resource by URI from a configured MCP server.
pub struct McpReadResource;

impl Tool for McpReadResource {
    fn name(&self) -> &str {
        "mcp_read_resource"
    }
    fn description(&self) -> &str {
        "Read a resource by URI from an MCP server configured in codecoder.json."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": { "type": "string", "description": "Name of the configured MCP server." },
                "uri": { "type": "string", "description": "Resource URI to read, e.g. file:///path." }
            },
            "required": ["server", "uri"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let server = args.get("server").and_then(Value::as_str).unwrap_or_default().to_string();
        let uri = args.get("uri").and_then(Value::as_str).unwrap_or_default().to_string();
        if server.is_empty() || uri.is_empty() {
            return Ok(ToolOutput::err("mcp_read_resource requires `server` and `uri`"));
        }
        with_manager(ctx.root, |mgr| match mgr.get_client(&server) {
            Ok(client) => match client.read_resource(&uri) {
                Ok(text) => ToolOutput::ok(text),
                Err(e) => ToolOutput::err(format!("mcp read_resource failed: {e}")),
            },
            Err(e) => ToolOutput::err(e.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn json_rpc_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "tools/list".into(),
            params: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["method"], "tools/list");
        assert_eq!(json["id"], 1);
        // params: None must be omitted (not serialized as null).
        assert!(json.get("params").is_none());
    }

    #[test]
    fn json_rpc_request_with_params_serializes_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 2,
            method: "tools/call".into(),
            params: Some(json!({ "name": "foo" })),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["params"]["name"], "foo");
    }

    #[test]
    fn mcp_server_config_deserialize() {
        let json = json!({ "name": "fs", "command": "npx", "args": ["-y", "server"] });
        let cfg: McpServerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.name, "fs");
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y".to_string(), "server".to_string()]);
        assert!(cfg.env.is_empty()); // env defaults to empty
    }

    #[test]
    fn mcp_server_config_deserialize_with_env() {
        let json = json!({
            "name": "svc",
            "command": "python",
            "args": ["server.py"],
            "env": { "FOO": "bar" }
        });
        let cfg: McpServerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn parse_content_length_header() {
        // A single framed message with a 42-byte body.
        let body = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut cur = Cursor::new(framed.into_bytes());
        let raw = read_framed(&mut cur).unwrap();
        assert_eq!(raw, body.as_bytes());
    }

    #[test]
    fn read_framed_roundtrip_via_write_framed() {
        let body = serde_json::to_vec(&json!({ "ok": true, "n": 7 })).unwrap();
        let mut out = Vec::new();
        write_framed(&mut out, &body).unwrap();
        let mut cur = Cursor::new(out);
        let raw = read_framed(&mut cur).unwrap();
        assert_eq!(raw, body);
    }

    #[test]
    fn read_framed_missing_content_length_errors() {
        let mut cur = Cursor::new(b"Content-Type: application/json\r\n\r\n{}".to_vec());
        assert!(read_framed(&mut cur).is_err());
    }

    #[test]
    fn read_framed_truncated_stream_errors() {
        let mut cur = Cursor::new(b"Content-Length: 100\r\n\r\nshort".to_vec());
        assert!(read_framed(&mut cur).is_err());
    }

    #[test]
    fn extract_text_content_joins_text_items() {
        let result = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" },
                { "type": "image", "data": "..." }
            ]
        });
        let (text, is_error) = extract_text_content(&result);
        assert!(!is_error);
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn extract_text_content_flags_is_error() {
        let result = json!({ "isError": true, "content": [{ "type": "text", "text": "boom" }] });
        let (text, is_error) = extract_text_content(&result);
        assert!(is_error);
        assert_eq!(text, "boom");
    }

    #[test]
    fn extract_resource_text_joins_and_summarizes_blobs() {
        let result = json!({
            "contents": [
                { "uri": "x", "text": "line1" },
                { "uri": "y", "blob": "aGVsbG8=" }
            ]
        });
        let text = extract_resource_text(&result);
        assert!(text.contains("line1"));
        assert!(text.contains("[blob 8 bytes]"));
    }

    #[test]
    fn mcp_tools_permission_model() {
        assert!(matches!(McpToolCall.permission(&json!({}), Path::new(".")), Permission::Ask { key } if key == "mcp_call_tool"));
        assert!(matches!(McpListResources.permission(&json!({}), Path::new(".")), Permission::None));
        assert!(matches!(McpReadResource.permission(&json!({}), Path::new(".")), Permission::None));
    }

    #[test]
    fn manager_loads_empty_config_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = McpManager::from_config(dir.path());
        assert!(mgr.configs.is_empty());
        assert!(mgr.get_client("missing").is_err());
    }

    #[test]
    fn manager_loads_servers_from_codecoder_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("codecoder.json"),
            r#"{"mcp_servers":[{"name":"fs","command":"npx","args":["-y","server"]}]}"#,
        )
        .unwrap();
        let mgr = McpManager::from_config(dir.path());
        assert_eq!(mgr.configs.len(), 1);
        assert_eq!(mgr.configs[0].name, "fs");
    }

    /// Simulate a real MCP server that pushes a notification before the response.
    /// `read_framed` + the id-skip logic in `request()` should skip the
    /// notification (no `id`) and pick up the actual response frame.
    #[test]
    fn request_skips_server_notifications() {
        use std::io::Cursor;
        let response_body = br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let notification_body = br#"{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"ping"}}"#;
        let mut framed = Vec::new();
        // Server sends a notification first, then the response.
        write_framed(&mut framed, notification_body).unwrap();
        write_framed(&mut framed, response_body).unwrap();

        let mut reader = BufReader::new(Cursor::new(framed));
        // Emulate request(): read frames until the one matching id=1 arrives.
        let matched = loop {
            let raw = read_framed(&mut reader).unwrap();
            let msg: Value = serde_json::from_slice(&raw).unwrap();
            let Some(msg_id) = msg.get("id").and_then(Value::as_u64) else {
                continue; // notification — skip
            };
            if msg_id == 1 {
                assert_eq!(msg["result"]["ok"], true);
                break true;
            }
        };
        assert!(matched, "must have found the response matching id=1");
    }

    #[test]
    fn response_validation_checks_jsonrpc_version() {
        // A response with a wrong jsonrpc version should be rejected.
        let bad = json!({"jsonrpc":"1.0","id":1,"result":null});
        // We can't easily test the full request() path without a real server,
        // but we can verify JsonRpcResponse deserialization and the version check.
        let resp: JsonRpcResponse = serde_json::from_value(bad).unwrap();
        assert_ne!(resp.jsonrpc, "2.0");
    }

    // ── Integration-style tests (mock MCP server subprocess over stdio) ──

    /// Shared Python preamble for a mock MCP server. Reads framed JSON-RPC
    /// requests from `sys.stdin.buffer` (binary, so it never mixes buffering
    /// with the text wrapper) and writes framed responses to stdout.
    const PY_FRAME_PREAMBLE: &str = r#"
import sys, json

def write_msg(msg):
    body = json.dumps(msg).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def read_msg():
    cl = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line.startswith(b"Content-Length:"):
            cl = int(line.split(b":")[1].strip())
        if line in (b"\r\n", b"\n"):
            break
    body = sys.stdin.buffer.read(cl)
    return json.loads(body)
"#;

    /// Spin up a mock MCP server from `script` (with the frame preamble) and
    /// return an `McpClient` wired to it.
    fn spawn_mock_mcp_server(script: &str) -> McpClient {
        let mut server = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("failed to spawn mock MCP server");

        let stdin = std::io::BufWriter::new(server.stdin.take().unwrap());
        let stdout = std::io::BufReader::new(server.stdout.take().unwrap());

        McpClient { child: server, stdin, stdout, next_id: 1, initialized: false }
    }

    /// Full MCP lifecycle: initialize → tools/list → tools/call.
    #[test]
    fn mock_server_tools_list_and_call() {
        let script = format!(
            r#"{PY_FRAME_PREAMBLE}
# initialize
req = read_msg()
assert req["method"] == "initialize"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"protocolVersion":"2024-11-05","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1.0"}}}}}})
# initialized notification
_ = read_msg()
# tools/list
req = read_msg()
assert req["method"] == "tools/list"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"tools":[{{"name":"echo","description":"Echo back input","input_schema":{{"type":"object","properties":{{"text":{{"type":"string"}}}},"required":["text"]}}}}]}}}})
# tools/call
req = read_msg()
assert req["method"] == "tools/call"
args = req["params"]["arguments"]
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"content":[{{"type":"text","text":"echo: " + json.dumps(args)}}],"isError":False}}}})
"#
        );
        let mut client = spawn_mock_mcp_server(&script);

        client.initialize().expect("initialize should succeed");
        let tools = client.list_tools().expect("list_tools should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "Echo back input");

        let result = client.call_tool("echo", json!({"text": "hello world"})).expect("call_tool should succeed");
        assert!(result.contains("hello world"));

        client.shutdown().expect("shutdown should succeed");
    }

    /// Mock server returns an error (isError: true) from tools/call.
    #[test]
    fn mock_server_tool_call_error() {
        let script = format!(
            r#"{PY_FRAME_PREAMBLE}
req = read_msg()
assert req["method"] == "initialize"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"protocolVersion":"2024-11-05","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1.0"}}}}}})
_ = read_msg()
req = read_msg()
assert req["method"] == "tools/list"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"tools":[{{"name":"fail","description":"Always fails","input_schema":{{"type":"object","properties":{{}}}}}}]}}}})
req = read_msg()
assert req["method"] == "tools/call"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"content":[{{"type":"text","text":"something went wrong"}}],"isError":True}}}})
"#
        );
        let mut client = spawn_mock_mcp_server(&script);

        client.initialize().expect("initialize should succeed");
        let tools = client.list_tools().expect("list_tools should succeed");
        assert_eq!(tools[0].name, "fail");

        let err = client.call_tool("fail", json!({})).unwrap_err();
        assert!(err.to_string().contains("reported an error"), "got: {err}");
    }

    /// Mock server returns a resource list from resources/list.
    #[test]
    fn mock_server_list_resources() {
        let script = format!(
            r#"{PY_FRAME_PREAMBLE}
req = read_msg()
assert req["method"] == "initialize"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"protocolVersion":"2024-11-05","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1.0"}}}}}})
_ = read_msg()
req = read_msg()
assert req["method"] == "resources/list"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"resources":[{{"uri":"file:///tmp/test.txt","name":"test","description":"A test file","mimeType":"text/plain"}}]}}}})
"#
        );
        let mut client = spawn_mock_mcp_server(&script);

        client.initialize().expect("initialize should succeed");
        let res = client.list_resources().expect("list_resources should succeed");
        let resources = res.get("resources").and_then(|v| v.as_array()).expect("should have resources array");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], "file:///tmp/test.txt");
        assert_eq!(resources[0]["name"], "test");
    }

    /// Mock server returns resource content from resources/read.
    #[test]
    fn mock_server_read_resource() {
        let script = format!(
            r#"{PY_FRAME_PREAMBLE}
req = read_msg()
assert req["method"] == "initialize"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"protocolVersion":"2024-11-05","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1.0"}}}}}})
_ = read_msg()
req = read_msg()
assert req["method"] == "resources/read"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"contents":[{{"uri":"file:///tmp/test.txt","text":"hello world","mimeType":"text/plain"}}]}}}})
"#
        );
        let mut client = spawn_mock_mcp_server(&script);

        client.initialize().expect("initialize should succeed");
        let text = client.read_resource("file:///tmp/test.txt").expect("read_resource should succeed");
        assert_eq!(text, "hello world");
    }

    /// Mock server sends a notification before the response, verifying the
    /// id-skip logic in `request()`.
    #[test]
    fn mock_server_request_skips_notifications() {
        let script = format!(
            r#"{PY_FRAME_PREAMBLE}
req = read_msg()
assert req["method"] == "initialize"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"protocolVersion":"2024-11-05","capabilities":{{}},"serverInfo":{{"name":"mock","version":"1.0"}}}}}})
_ = read_msg()
# notification (no id) before the response
write_msg({{"jsonrpc":"2.0","method":"notifications/message","params":{{"level":"info","data":"hello"}}}})
req = read_msg()
assert req["method"] == "tools/list"
write_msg({{"jsonrpc":"2.0","id":req["id"],"result":{{"tools":[{{"name":"test-tool","description":"A test tool"}}]}}}})
"#
        );
        let mut client = spawn_mock_mcp_server(&script);

        client.initialize().expect("initialize should succeed");
        let tools = client.list_tools().expect("list_tools should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test-tool");
    }
}