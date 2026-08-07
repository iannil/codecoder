// LSP (Language Server Protocol) client — JSON-RPC 2.0 over stdio.
//
// Task 3 of the MCP/LSP tools plan. Manages language server subprocesses,
// speaks the JSON-RPC 2.0 framing shared with MCP (`Content-Length` header),
// and performs the LSP initialize/initialized/shutdown lifecycle.
//
// Spec: https://microsoft.github.io/language-server-protocol/specification

use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use anyhow::anyhow;
use lsp_types::Uri;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::str::FromStr;

// ── URI helper ────────────────────────────────────────────────────────────

/// Convert a file path to a `file://` URI string.
fn path_to_uri(path: &Path) -> anyhow::Result<String> {
    if !path.is_absolute() {
        anyhow::bail!("path must be absolute: {}", path.display());
    }
    // On Unix, absolute paths start with `/`, so `file:///path` is correct.
    Ok(format!("file://{}", path.display()))
}

/// Convert a file path string to a `lsp_types::Uri`.
fn file_path_to_uri(file_path: &str) -> anyhow::Result<Uri> {
    let path = std::path::Path::new(file_path);
    let uri_str = path_to_uri(path)?;
    Uri::from_str(&uri_str).map_err(|e| anyhow!("invalid URI '{uri_str}': {e}"))
}

// ── Framing helpers (identical to mcp.rs: `Content-Length` header) ─────────

/// Write one framed message: `Content-Length: <N>\r\n\r\n<JSON body>`.
fn write_framed<W: Write>(writer: &mut W, body: &[u8]) -> anyhow::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    Ok(())
}

/// Read one framed message from a buffered reader, returning the raw JSON body.
/// Parses the `Content-Length` header (tolerating CRLF/LF line endings) and then
/// reads exactly that many bytes.
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

// ── LSP server discovery ──────────────────────────────────────────────────

/// Detect the language server command for a given file path based on extension.
/// Returns `(command, args)` or `None` if no server is known for the extension.
pub fn detect_lsp_server(file_path: &str) -> Option<(&'static str, Vec<&'static str>)> {
    if file_path.ends_with(".rs") {
        Some(("rust-analyzer", vec![]))
    } else if file_path.ends_with(".py") {
        Some(("pylsp", vec![]))
    } else if file_path.ends_with(".js") || file_path.ends_with(".ts")
        || file_path.ends_with(".jsx") || file_path.ends_with(".tsx")
    {
        Some(("typescript-language-server", vec!["--stdio"]))
    } else if file_path.ends_with(".go") {
        Some(("gopls", vec![]))
    } else if file_path.ends_with(".c") || file_path.ends_with(".h")
        || file_path.ends_with(".cpp") || file_path.ends_with(".hpp")
    {
        Some(("clangd", vec![]))
    } else {
        None
    }
}

// ── LspClient: one server subprocess + JSON-RPC channel ───────────────────

/// Manages a single language server subprocess and its JSON-RPC communication.
pub struct LspClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    server_capabilities: Option<lsp_types::ServerCapabilities>,
    root_uri: Uri,
}

impl LspClient {
    /// Start the language server subprocess and set up the stdio pipes.
    pub fn spawn(command: &str, args: &[String], root: &Path) -> anyhow::Result<Self> {
        let root_uri_str = path_to_uri(root)?;
        let root_uri = Uri::from_str(&root_uri_str)
            .map_err(|e| anyhow!("invalid root URI '{root_uri_str}': {e}"))?;
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("failed to spawn LSP server '{command}': {e}"))?;
        let stdin = BufWriter::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("LSP server stdin unavailable"))?,
        );
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("LSP server stdout unavailable"))?,
        );
        Ok(Self { child, stdin, stdout, next_id: 1, server_capabilities: None, root_uri })
    }

    /// Send a JSON-RPC request and wait for the matching response.
    pub fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        write_framed(&mut self.stdin, &body)?;
        self.stdin.flush()?;

        // Loop reading frames until the response matching our `id` arrives.
        loop {
            let raw = read_framed(&mut self.stdout)?;
            let msg: Value = serde_json::from_slice(&raw)
                .map_err(|e| anyhow!("bad JSON-RPC message from {method}: {e}"))?;
            // Skip notifications (no `id` field) and non-matching responses.
            let Some(msg_id) = msg.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if msg_id != id {
                continue;
            }
            if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                anyhow::bail!("bad jsonrpc version in response to {method}");
            }
            if let Some(err) = msg.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown");
                let detail = err
                    .get("data")
                    .map(|d| format!(", data: {d}"))
                    .unwrap_or_default();
                anyhow::bail!("JSON-RPC error {code}: {message}{detail}");
            }
            return msg.get("result").cloned()
                .ok_or_else(|| anyhow!("response to {method} missing result"));
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))?;
        write_framed(&mut self.stdin, &body)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Perform the LSP `initialize` handshake.
    pub fn initialize(&mut self) -> anyhow::Result<lsp_types::ServerCapabilities> {
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "codecoder", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": {},
            "rootUri": self.root_uri.as_str(),
        });
        let result = self.request("initialize", params)?;
        let init_result: lsp_types::InitializeResult = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse initialize result: {e}"))?;
        // Send initialized notification after successful handshake.
        self.notify("initialized", json!({}))?;
        self.server_capabilities = Some(init_result.capabilities.clone());
        Ok(init_result.capabilities)
    }

    /// Send `initialized` notification.
    pub fn initialized(&mut self) -> anyhow::Result<()> {
        self.notify("initialized", json!({}))
    }

    /// Open a document (textDocument/didOpen).
    pub fn did_open(&mut self, uri: &Uri, text: &str, version: i32) -> anyhow::Result<()> {
        let params = json!({
            "textDocument": {
                "uri": uri.as_str(),
                "languageId": "",
                "version": version,
                "text": text,
            }
        });
        self.notify("textDocument/didOpen", params)
    }

    // ── LSP query methods ────────────────────────────────────────────────

    /// Resolve the definition location of a symbol at a given position.
    pub fn go_to_definition(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        let params = json!({
            "textDocument": { "uri": uri.as_str() },
            "position": { "line": line, "character": character },
        });
        let result = self.request("textDocument/definition", params)?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let response: lsp_types::GotoDefinitionResponse = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse definition response: {e}"))?;
        match response {
            lsp_types::GotoDefinitionResponse::Scalar(loc) => Ok(vec![loc]),
            lsp_types::GotoDefinitionResponse::Array(locs) => Ok(locs),
            lsp_types::GotoDefinitionResponse::Link(_) => Ok(Vec::new()),
        }
    }

    /// Find all references to a symbol at a given position.
    pub fn find_references(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        let params = json!({
            "textDocument": { "uri": uri.as_str() },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true },
        });
        let result = self.request("textDocument/references", params)?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let locations: Vec<lsp_types::Location> = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse references response: {e}"))?;
        Ok(locations)
    }

    /// Get hover information at a given position.
    pub fn hover(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Option<lsp_types::Hover>> {
        let params = json!({
            "textDocument": { "uri": uri.as_str() },
            "position": { "line": line, "character": character },
        });
        let result = self.request("textDocument/hover", params)?;
        if result.is_null() {
            return Ok(None);
        }
        let hover: lsp_types::Hover = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse hover response: {e}"))?;
        Ok(Some(hover))
    }

    /// List all symbols in a document.
    pub fn document_symbol(
        &mut self,
        uri: &Uri,
    ) -> anyhow::Result<Vec<lsp_types::DocumentSymbol>> {
        let params = json!({
            "textDocument": { "uri": uri.as_str() },
        });
        let result = self.request("textDocument/documentSymbol", params)?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let response: lsp_types::DocumentSymbolResponse = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse documentSymbol response: {e}"))?;
        match response {
            lsp_types::DocumentSymbolResponse::Nested(symbols) => Ok(symbols),
            lsp_types::DocumentSymbolResponse::Flat(_) => Ok(Vec::new()),
        }
    }

    /// Search for symbols across the workspace.
    pub fn workspace_symbol(
        &mut self,
        query: &str,
    ) -> anyhow::Result<Vec<lsp_types::SymbolInformation>> {
        let params = json!({ "query": query });
        let result = self.request("workspace/symbol", params)?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let response: lsp_types::WorkspaceSymbolResponse = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse workspaceSymbol response: {e}"))?;
        match response {
            lsp_types::WorkspaceSymbolResponse::Flat(symbols) => Ok(symbols),
            lsp_types::WorkspaceSymbolResponse::Nested(_) => Ok(Vec::new()),
        }
    }

    /// Resolve the implementation location of a symbol.
    pub fn go_to_implementation(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<lsp_types::Location>> {
        let params = json!({
            "textDocument": { "uri": uri.as_str() },
            "position": { "line": line, "character": character },
        });
        let result = self.request("textDocument/implementation", params)?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let response: lsp_types::GotoDefinitionResponse = serde_json::from_value(result)
            .map_err(|e| anyhow!("failed to parse implementation response: {e}"))?;
        match response {
            lsp_types::GotoDefinitionResponse::Scalar(loc) => Ok(vec![loc]),
            lsp_types::GotoDefinitionResponse::Array(locs) => Ok(locs),
            lsp_types::GotoDefinitionResponse::Link(_) => Ok(Vec::new()),
        }
    }

    /// Graceful shutdown: send shutdown request, then exit notification, then kill.
    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.server_capabilities.is_some() {
            let _ = self.request("shutdown", json!({}));
            let _ = self.notify("exit", json!({}));
            self.server_capabilities = None;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Best-effort cleanup: send shutdown, exit, kill, and reap.
        if self.server_capabilities.is_some() {
            let _ = self.request("shutdown", json!({}));
            let _ = self.notify("exit", json!({}));
            self.server_capabilities = None;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── LspManager: global server lifecycle ───────────────────────────────────

/// Global LSP client manager. Holds spawned clients keyed by server command name.
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    root: Option<std::path::PathBuf>,
}

impl LspManager {
    fn new() -> Self {
        Self { clients: HashMap::new(), root: None }
    }

    /// Get or create a client for the given server command.
    pub fn get_client(
        &mut self,
        command: &str,
        args: &[String],
        root: &Path,
    ) -> anyhow::Result<&mut LspClient> {
        if !self.clients.contains_key(command) {
            self.root = Some(root.to_path_buf());
            let mut client = LspClient::spawn(command, args, root)?;
            client.initialize()?;
            self.clients.insert(command.to_string(), client);
        }
        Ok(self.clients.get_mut(command).expect("just inserted"))
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        for (_, client) in &mut self.clients {
            let _ = client.shutdown();
        }
    }
}

/// Global manager instance, lazily initialized once per process.
static MANAGER: std::sync::LazyLock<std::sync::Mutex<LspManager>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(LspManager::new()));

// ── LspTool ────────────────────────────────────────────────────────────────

/// Perform LSP (Language Server Protocol) operations on a file.
///
/// Supported operations:
/// - `go_to_definition`: resolve definition location of a symbol
/// - `find_references`: find all references to a symbol
/// - `hover`: get hover information at a position
/// - `document_symbol`: list all symbols in a document
/// - `workspace_symbol`: search for symbols across the workspace
/// - `go_to_implementation`: resolve implementation location of a symbol
///
/// The language server is auto-detected from the file extension.
pub struct LspTool;

impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }
    fn description(&self) -> &str {
        "Perform Language Server Protocol operations on a file: \
         go_to_definition, find_references, hover, document_symbol, \
         workspace_symbol, go_to_implementation. The server is auto-detected \
         from the file extension (.rs -> rust-analyzer, .py -> pylsp, \
         .js/.ts/.jsx/.tsx -> typescript-language-server, .go -> gopls, \
         .c/.h/.cpp/.hpp -> clangd)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "go_to_definition",
                        "find_references",
                        "hover",
                        "document_symbol",
                        "workspace_symbol",
                        "go_to_implementation"
                    ],
                    "description": "The LSP operation to perform."
                },
                "file_path": {
                    "type": "string",
                    "description": "Path to the file (required for all operations except workspace_symbol)."
                },
                "line": {
                    "type": "integer",
                    "description": "0-based line number (required for go_to_definition, find_references, hover, go_to_implementation)."
                },
                "character": {
                    "type": "integer",
                    "description": "0-based character offset (required for go_to_definition, find_references, hover, go_to_implementation)."
                },
                "query": {
                    "type": "string",
                    "description": "Symbol query string (required for workspace_symbol)."
                }
            },
            "required": ["operation"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let operation = args
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if operation.is_empty() {
            return Ok(ToolOutput::err("lsp requires `operation`"));
        }
        // Validate the operation name before doing any work.
        const OPERATIONS: [&str; 6] = [
            "go_to_definition",
            "find_references",
            "hover",
            "document_symbol",
            "workspace_symbol",
            "go_to_implementation",
        ];
        if !OPERATIONS.contains(&operation.as_str()) {
            return Ok(ToolOutput::err(format!("unknown lsp operation: {operation}")));
        }

        // Validate workspace_symbol's query early, before server detection.
        if operation == "workspace_symbol" {
            let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
            if query.is_empty() {
                return Ok(ToolOutput::err("workspace_symbol requires `query`"));
            }
        }

        // workspace_symbol does not need a file_path.
        let file_path = if operation == "workspace_symbol" {
            String::new()
        } else {
            let fp = args.get("file_path").and_then(Value::as_str).unwrap_or_default();
            if fp.is_empty() {
                return Ok(ToolOutput::err(
                    "lsp requires `file_path` for this operation",
                ));
            }
            fp.to_string()
        };

        // Detect the language server from the file extension.
        let (command, args_list) = match detect_lsp_server(&file_path) {
            Some((cmd, a)) => (cmd, a.iter().map(|&s| s.to_string()).collect::<Vec<_>>()),
            None => {
                return Ok(ToolOutput::err(format!(
                    "no LSP server configured for: {file_path}"
                )));
            }
        };

        // Lock the global manager and get or spawn the client.
        let mut mgr = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
        let client = match mgr.get_client(command, &args_list, ctx.root) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput::err(format!("lsp server spawn failed: {e}")));
            }
        };

        let result = match operation.as_str() {
            "go_to_definition" => {
                let line = args.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
                let character = args.get("character").and_then(Value::as_u64).unwrap_or(0) as u32;
                let uri = file_path_to_uri(&file_path)
                    .map_err(|_| anyhow!("invalid file path: {file_path}"))?;
                let locs = client.go_to_definition(&uri, line, character)?;
                serde_json::to_string_pretty(&locs)?
            }
            "find_references" => {
                let line = args.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
                let character = args.get("character").and_then(Value::as_u64).unwrap_or(0) as u32;
                let uri = file_path_to_uri(&file_path)
                    .map_err(|_| anyhow!("invalid file path: {file_path}"))?;
                let locs = client.find_references(&uri, line, character)?;
                serde_json::to_string_pretty(&locs)?
            }
            "hover" => {
                let line = args.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
                let character = args.get("character").and_then(Value::as_u64).unwrap_or(0) as u32;
                let uri = file_path_to_uri(&file_path)
                    .map_err(|_| anyhow!("invalid file path: {file_path}"))?;
                let h = client.hover(&uri, line, character)?;
                serde_json::to_string_pretty(&h)?
            }
            "document_symbol" => {
                let uri = file_path_to_uri(&file_path)
                    .map_err(|_| anyhow!("invalid file path: {file_path}"))?;
                let syms = client.document_symbol(&uri)?;
                serde_json::to_string_pretty(&syms)?
            }
            "workspace_symbol" => {
                let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
                // query already validated early, but guard defensively.
                if query.is_empty() {
                    return Ok(ToolOutput::err("workspace_symbol requires `query`"));
                }
                let syms = client.workspace_symbol(query)?;
                serde_json::to_string_pretty(&syms)?
            }
            "go_to_implementation" => {
                let line = args.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
                let character = args.get("character").and_then(Value::as_u64).unwrap_or(0) as u32;
                let uri = file_path_to_uri(&file_path)
                    .map_err(|_| anyhow!("invalid file path: {file_path}"))?;
                let locs = client.go_to_implementation(&uri, line, character)?;
                serde_json::to_string_pretty(&locs)?
            }
            // All unknown operations are caught by the early validation above,
            // but the compiler can't see that — keep the wildcard arm.
            _ => unreachable!(),
        };

        Ok(ToolOutput::ok(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ── Server discovery tests ───────────────────────────────────────────

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
    fn detect_typescript_server() {
        assert_eq!(
            detect_lsp_server("component.tsx").map(|(cmd, _)| cmd),
            Some("typescript-language-server")
        );
    }

    #[test]
    fn detect_javascript_server() {
        assert_eq!(
            detect_lsp_server("index.js").map(|(cmd, _)| cmd),
            Some("typescript-language-server")
        );
    }

    #[test]
    fn detect_go_server() {
        assert_eq!(
            detect_lsp_server("main.go").map(|(cmd, _)| cmd),
            Some("gopls")
        );
    }

    #[test]
    fn detect_c_server() {
        assert_eq!(
            detect_lsp_server("main.c").map(|(cmd, _)| cmd),
            Some("clangd")
        );
    }

    #[test]
    fn detect_cpp_server() {
        assert_eq!(
            detect_lsp_server("main.cpp").map(|(cmd, _)| cmd),
            Some("clangd")
        );
    }

    #[test]
    fn detect_header_server() {
        assert_eq!(
            detect_lsp_server("utils.h").map(|(cmd, _)| cmd),
            Some("clangd")
        );
    }

    #[test]
    fn unknown_extension_returns_none() {
        assert!(detect_lsp_server("readme.md").is_none());
        assert!(detect_lsp_server("Makefile").is_none());
        assert!(detect_lsp_server("").is_none());
    }

    // ── Framing tests ────────────────────────────────────────────────────

    #[test]
    fn content_length_parsing() {
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
    fn read_framed_empty_stream_errors() {
        let mut cur = Cursor::new(b"".to_vec());
        assert!(read_framed(&mut cur).is_err());
    }

    // ── Tool permission and validation tests ─────────────────────────────

    #[test]
    fn lsp_tool_permission_is_none() {
        assert!(matches!(
            LspTool.permission(&json!({}), Path::new(".")),
            Permission::None
        ));
    }

    #[test]
    fn lsp_tool_requires_operation() {
        let result = LspTool.run(json!({}), &mut ToolCtx::new(Path::new("."))).unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("requires `operation`"));
    }

    #[test]
    fn lsp_tool_requires_file_path() {
        let result = LspTool
            .run(
                json!({"operation": "go_to_definition"}),
                &mut ToolCtx::new(Path::new(".")),
            )
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("requires `file_path`"));
    }

    #[test]
    fn lsp_tool_unknown_extension() {
        let result = LspTool
            .run(
                json!({"operation": "go_to_definition", "file_path": "readme.md", "line": 0, "character": 0}),
                &mut ToolCtx::new(Path::new(".")),
            )
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("no LSP server configured"));
    }

    #[test]
    fn workspace_symbol_requires_query() {
        // workspace_symbol with file_path but no query should fail early.
        let result = LspTool
            .run(
                json!({"operation": "workspace_symbol", "file_path": "src/main.rs"}),
                &mut ToolCtx::new(Path::new(".")),
            )
            .unwrap();
        assert!(result.is_error);
        // With the early query check, it fails before server detection.
        assert!(result.content.contains("requires `query`"), "got: {}", result.content);
    }

    #[test]
    fn unknown_operation() {
        let result = LspTool
            .run(
                json!({"operation": "unknown_op", "file_path": "src/main.rs"}),
                &mut ToolCtx::new(Path::new(".")),
            )
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("unknown lsp operation"), "got: {}", result.content);
    }

    // ── Request/response simulation tests ────────────────────────────────

    /// Simulate a server that pushes a notification before the response.
    /// The id-skip logic in `request()` should skip the notification (no `id`)
    /// and pick up the actual response frame.
    #[test]
    fn request_skips_server_notifications() {
        let response_body = br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let notification_body =
            br#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{}}"#;
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
    fn request_rejects_missing_id() {
        // A response with a non-matching id should be skipped.
        let response_body = br#"{"jsonrpc":"2.0","id":99,"result":null}"#;
        let mut framed = Vec::new();
        write_framed(&mut framed, response_body).unwrap();

        let mut reader = BufReader::new(Cursor::new(framed));
        // Read a frame with id=99 — another caller's response.
        let raw = read_framed(&mut reader).unwrap();
        let msg: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(msg["id"], 99);
    }

    #[test]
    fn goto_definition_response_deserialize_scalar() {
        let json = json!({
            "uri": "file:///test.rs",
            "range": { "start": { "line": 10, "character": 5 }, "end": { "line": 10, "character": 20 } }
        });
        let resp: lsp_types::GotoDefinitionResponse = serde_json::from_value(json).unwrap();
        assert!(matches!(resp, lsp_types::GotoDefinitionResponse::Scalar(_)));
    }

    #[test]
    fn goto_definition_response_deserialize_array() {
        let json = json!([
            { "uri": "file:///test.rs", "range": { "start": { "line": 10, "character": 5 }, "end": { "line": 10, "character": 20 } } }
        ]);
        let resp: lsp_types::GotoDefinitionResponse = serde_json::from_value(json).unwrap();
        assert!(matches!(resp, lsp_types::GotoDefinitionResponse::Array(_)));
    }

    #[test]
    fn document_symbol_response_deserialize_nested() {
        let json = json!([
            { "name": "foo", "kind": 6, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } }, "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } } }
        ]);
        let resp: lsp_types::DocumentSymbolResponse = serde_json::from_value(json).unwrap();
        assert!(matches!(resp, lsp_types::DocumentSymbolResponse::Nested(_)));
    }

    #[test]
    fn workspace_symbol_response_deserialize_flat() {
        let json = json!([
            { "name": "foo", "kind": 6, "location": { "uri": "file:///test.rs", "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } } } }
        ]);
        let resp: lsp_types::WorkspaceSymbolResponse = serde_json::from_value(json).unwrap();
        assert!(matches!(resp, lsp_types::WorkspaceSymbolResponse::Flat(_)));
    }
}