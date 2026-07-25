## Phase A: 基础设施（P1 + P2 中依赖最小的项）

### Task 1: CLI `--help` / `--version` 入口

**审计缺口:** #2 (P1) — main.rs 无 arg 解析，用户只能通过 env 控制入口

**文件:**
- Modify: `src/main.rs`
- Create: 无（所有改动在 main.rs 内）
- Test: 已在 `src/lib.rs` 有 `bg_mode_from_env` 测试

**接口:**
- Consumes: 无
- Produces: CLI 参数解析，输出 `--help` 和 `--version` 文本

- [ ] **Step 1: 在 main.rs 顶部添加 arg 解析**

在 `fn main()` 的 `let cfg = codecoder::Config::from_env()` 之前插入：

```rust
fn main() -> anyhow::Result<()> {
    codecoder::config::autoload_ccd_env();
    // ── CLI arg 解析（先于 env 路由，--help/--version 直接退出）──
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--help" | "-h" => {
                println!("CodeCoder — autonomous AI agent");
                println!();
                println!("USAGE:");
                println!("  {} [FLAGS]          Start daemon (default mode)", args[0]);
                println!("  {} --help            Show this help", args[0]);
                println!("  {} --version         Show version", args[0]);
                println!();
                println!("Modes (set via environment variable, mutually exclusive):");
                println!("  CODECODER_DAEMON=1           Run as daemon (default)");
                println!("  CODECODER_BG_TASK=<task>     Run one headless task, then exit");
                println!("  CODECODER_BG_WORKGRAPH=1     Run workgraph milestones headless, then exit");
                println!();
                println!("Configuration (env vars, see README.md for full table):");
                println!("  CODECODER_API_KEY        LLM API key (required for real LLM)");
                println!("  CODECODER_MODEL          Model name (default: gpt-4o)");
                println!("  CODECODER_ROOT           Project root (default: CWD)");
                println!("  CODECODER_DAEMON         1 = daemon mode");
                println!("  CODECODER_BG_TASK        Headless one-shot task");
                println!("  CODECODER_BG_WORKGRAPH   1 = headless workgraph mode");
                return Ok(());
            }
            "--version" | "-v" => {
                // 版本号可从 Cargo.toml 编译时注入（env!("CARGO_PKG_VERSION")）
                println!("CodeCoder {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {
                // 未知 flag → 仅警告，不阻止启动
                eprintln!("ccd: unknown flag '{}' (try --help)", args[1]);
            }
        }
    }
    let cfg = codecoder::Config::from_env();
    // ... 后续代码不变
```

- [ ] **Step 2: 确保 Cargo.toml 有 `version` 字段**

```bash
grep '^version' Cargo.toml
# 应该输出类似 version = "0.1.0"
# 如果没有，添加一行
```

- [ ] **Step 3: 手动测试**

```bash
cargo run -- --help
# 应打印帮助信息，不启动 daemon
cargo run -- --version
# 应打印版本号，不启动 daemon
cargo run -- --unknown-flag
# 应警告 unknown flag，继续正常启动（或退回到 daemon 模式）
```

- [ ] **Step 4: 提交**

```bash
git add -A
git commit -m "feat(cli): add --help and --version CLI entry"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

### Task 2: `cc help` 命令

**审计缺口:** #7 (P2) — 用户无从发现可用命令

**文件:**
- Modify: `src/bin/cc.rs`

**接口:**
- Consumes: 无（纯本地，不连 daemon）
- Produces: 帮助文本打印到 stdout

- [ ] **Step 1: 在 cc.rs 的 `match args.as_slice()` 中添加 `help` 分支**

在 `[]` (REPL) 分支之前，新增：

```rust
[one] if one == "help" || one == "--help" => {
    println!("cc — CodeCoder client");
    println!();
    println!("USAGE:");
    println!("  cc <message>           Send a message (one-shot mode)");
    println!("  cc                     Start interactive REPL");
    println!("  cc help                Show this help");
    println!("  cc shutdown            Stop the daemon gracefully");
    println!("  cc status              Show daemon status");
    println!("  cc services            List running persistent services");
    println!("  cc sessions            List all sessions");
    println!("  cc resume <id>         Resume a session");
    println!("  cc tree                Show session tree");
    println!("  cc fork <id>           Navigate session tree (fork)");
    println!("  cc clone               Clone current session");
    println!("  cc ledger              Show BG task ledger");
    println!("  cc ledger --failed     Show only failed BG tasks");
    println!("  cc ledger --last <n>   Show last N BG tasks");
    println!("  cc ledger --detail     Show detailed last BG task");
    println!();
    println!("REPL commands (inside interactive mode):");
    println!("  /exit                  Exit REPL");
    println!("  /tree                  Show session tree");
    println!("  /fork <id>             Navigate session tree");
    println!("  /clone                 Clone current session");
    Ok(())
}
```

- [ ] **Step 2: 手动测试**

```bash
cargo run --bin cc -- help
```

- [ ] **Step 3: 提交**

```bash
git add -A
git commit -m "feat(cc): add 'cc help' command"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

### Task 3: `cc status` 完整实现

**审计缺口:** #4 (P2) — proto 声明了 Status 但只返回 `"ccd running"`，无实质信息

**文件:**
- Modify: `src/daemon/proto.rs`（新增 `ServerEvent::Status`）
- Modify: `src/daemon/socket.rs`（Status 路由：收集 daemon 状态信息）
- Modify: `src/daemon/mod.rs`（Daemon 暴露状态信息途径）
- Modify: `src/client/mod.rs`（渲染 Status 事件）
- Modify: `src/bin/cc.rs`（已有 `cc status` 入口，不需改）

**接口:**
- Consumes: `Daemon` 实例（通过 `DaemonSessionManager` 或共享状态）
- Produces: `ServerEvent::Status { sessions, threads, uptime, supervisor }`

- [ ] **Step 1: 新增 `ServerEvent::Status` 变体**

在 `src/daemon/proto.rs` 的 `ServerEvent` 枚举中新增：

```rust
/// 对 Status 请求的响应（daemon 健康状态快照）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub uptime_secs: u64,
    pub active_sessions: usize,
    pub session_ids: Vec<String>,
    pub supervisor_count: usize,
    pub supervisor_services: Vec<ServiceStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub address: String,
    pub gave_up: bool,
}
```

在 `ServerEvent` 中新增：

```rust
    /// 对 Status 请求的响应（daemon 健康状态）。
    Status(DaemonStatus),
```

- [ ] **Step 2: 在 `Daemon` 结构或 `DaemonSessionManager` 中记录启动时间**

`DaemonSessionManager` 已有 `root`、`registry` 等字段。新增 `started_at: std::time::Instant`：

```rust
pub struct DaemonSessionManager {
    // ... 现有字段
    pub started_at: std::time::Instant,
    // ... 其他字段
}
```

在 `new()` 中初始化：

```rust
started_at: std::time::Instant::now(),
```

- [ ] **Step 3: 在 `DaemonSessionManager` 中添加 `status()` 方法**

```rust
pub fn status(&self) -> DaemonStatus {
    DaemonStatus {
        uptime_secs: self.started_at.elapsed().as_secs(),
        active_sessions: self.sessions.len(),
        session_ids: self.list(),
        supervisor_count: 0, // 暂缺，Task 4 会填充
        supervisor_services: vec![],
    }
}
```

- [ ] **Step 4: 在 socket.rs 中完善 Status 路由**

在 `ClientRequest::Status =>` 分支中，收集状态信息：

```rust
ClientRequest::Status => {
    let g = mgr.lock().unwrap();
    let status = g.status();
    drop(g);
    let _ = body_tx.send(ServerEvent::Status(status));
    let _ = body_tx.send(ServerEvent::TurnComplete);
}
```

- [ ] **Step 5: 在 `print_event` 中渲染 Status 事件**

在 `src/client/mod.rs` 的 `print_event` 中添加：

```rust
ServerEvent::Status(s) => {
    println!("daemon status:");
    println!("  uptime: {}s", s.uptime_secs);
    println!("  sessions: {} ({})", s.active_sessions, s.session_ids.join(", "));
    for svc in &s.supervisor_services {
        let status = if svc.gave_up { "FAILED" } else { "running" };
        println!("  service: {} ({}) {} @ {}", svc.name, status, svc.address);
    }
    false
}
```

- [ ] **Step 6: 写测试 — 验证 DaemonStatus 序列化/反序列化**

在 `src/daemon/proto.rs` 的测试块中（或 `tests/` 下）：

```rust
#[test]
fn daemon_status_round_trips() {
    use crate::daemon::proto::{DaemonStatus, ServiceStatus, ServerEvent};
    let ds = DaemonStatus {
        uptime_secs: 42,
        active_sessions: 1,
        session_ids: vec!["s0000".into()],
        supervisor_count: 2,
        supervisor_services: vec![
            ServiceStatus { name: "web".into(), address: "http://127.0.0.1:8080".into(), gave_up: false },
            ServiceStatus { name: "db".into(), address: "".into(), gave_up: true },
        ],
    };
    let ev = ServerEvent::Status(ds.clone());
    let json = serde_json::to_string(&ev).unwrap();
    let back: ServerEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(ev, back);
}
```

- [ ] **Step 7: 手动测试**

```bash
# 启动 daemon
CODECODER_DAEMON=1 cargo run &
sleep 1
# 查询状态
cargo run --bin cc -- status
# 应输出 uptime、sessions 等信息
```

- [ ] **Step 8: 提交**

```bash
git add -A
git commit -m "feat(cc): implement 'cc status' with daemon health snapshot"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

## Phase B: Capability 管理（P1）

### Task 4: `cc services` 命令 — 查看 Persistent 服务

**审计缺口:** #1 (P1) — 无法查看/管理正在运行的 Persistent 服务

**文件:**
- Modify: `src/daemon/proto.rs`（新增 `ClientRequest::Services`、补充 `ServerEvent::Status` 的 supervisor 字段）
- Modify: `src/daemon/mod.rs`（Daemon 结构需要暴露 Supervisor 状态）
- Modify: `src/daemon/socket.rs`（路由 Services 请求）
- Modify: `src/client/mod.rs`（渲染 Services 事件）
- Modify: `src/bin/cc.rs`（新增 `cc services` 入口）
- Modify: `src/capability.rs`（Supervisor 暴露状态查询方法）

**接口:**
- Consumes: `Supervisor::states`（已有 `pub states: HashMap<String, SupervisedService>`）
- Produces: `ClientRequest::Services` → `ServerEvent::Services(Vec<ServiceStatus>)`

- [ ] **Step 1: 在 `ServerEvent` 中新增 `Services` 变体**

```rust
/// 列 Persistent 服务的状态响应。
Services(Vec<ServiceStatus>),
```

- [ ] **Step 2: 在 `Supervisor` 中添加 `service_statuses()` 方法**

```rust
impl Supervisor {
    pub fn service_statuses(&self) -> Vec<crate::daemon::proto::ServiceStatus> {
        let mut v = Vec::new();
        for (name, s) in &self.states {
            v.push(crate::daemon::proto::ServiceStatus {
                name: name.clone(),
                address: s.manifest.address.clone(),
                gave_up: s.gave_up,
            });
        }
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}
```

- [ ] **Step 3: Daemon 使 Supervisor 状态可访问**

当前 `Daemon::run()` 中 `supervisor` 是局部变量，在线程闭包中借用。需改为 `Arc<Mutex<Supervisor>>` 共享：

```rust
// 在 daemon/mod.rs 中
let supervisor = Arc::new(Mutex::new(
    crate::capability::Supervisor::start_all(&self.cfg.root, budget)
        .unwrap_or_else(|e| { ... })
));
```

修改监督线程和 shutdown 相应使用 `Arc::clone(&supervisor)`。

同时将 `supervisor` 的 Arc 传给 `handle_connection` 或通过 `DaemonSessionManager` 间接访问。

有两种实现方案，选择**给 mgr 加 supervisor 引用**的方式（改动最小）：

```rust
// 在 DaemonSessionManager 中新增字段
pub supervisor: Option<Arc<Mutex<crate::capability::Supervisor>>>,
```

在 `Daemon::run()` 中传入：

```rust
let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
    provider, model, max_tokens, temperature, root.clone(), registry,
)));
mgr.lock().unwrap().supervisor = Some(Arc::clone(&supervisor));
```

- [ ] **Step 4: 在 `DaemonSessionManager` 中添加 `service_statuses()` 方法**

```rust
pub fn service_statuses(&self) -> Vec<crate::daemon::proto::ServiceStatus> {
    match &self.supervisor {
        Some(sup) => sup.lock().unwrap().service_statuses(),
        None => vec![],
    }
}
```

- [ ] **Step 5: 在 socket.rs 中路由 `ClientRequest::Services`**

```rust
ClientRequest::Services => {
    let g = mgr.lock().unwrap();
    let services = g.service_statuses();
    let _ = body_tx.send(ServerEvent::Services(services));
    let _ = body_tx.send(ServerEvent::TurnComplete);
}
```

- [ ] **Step 6: 在 `print_event` 中渲染 Services 事件**

```rust
ServerEvent::Services(services) => {
    if services.is_empty() {
        println!("(no persistent services)");
    } else {
        println!("persistent services:");
        for svc in services {
            let status = if svc.gave_up { "✗ FAILED" } else { "✓ running" };
            println!("  {}  {}  {}", status, svc.name,
                if svc.address.is_empty() { "(no address)" } else { &svc.address });
        }
    }
    false
}
```

- [ ] **Step 7: 在 cc.rs 中添加 `cc services` 入口**

```rust
[one] if one == "services" => send_one(&sock, ClientRequest::Services),
```

- [ ] **Step 8: 更新 `cc help` 包含 `services`**

- [ ] **Step 9: 写测试 — 验证 Services 协议**

```rust
#[test]
fn services_event_round_trips() {
    let svc = ServiceStatus { name: "web".into(), address: "http://127.0.0.1:8080".into(), gave_up: false };
    let ev = ServerEvent::Services(vec![svc]);
    let json = serde_json::to_string(&ev).unwrap();
    let back: ServerEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(ev, back);
}
```

- [ ] **Step 10: 手动测试**

```bash
# 启动 daemon，确保有 capabilities/ 目录
CODECODER_DAEMON=1 cargo run &
sleep 1
cargo run --bin cc -- services
# 应列出运行中/失败的服务
```

- [ ] **Step 11: 提交**

```bash
git add -A
git commit -m "feat(cc): add 'cc services' command to list persistent services"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

### Task 5: `cc workgraph` 命令 — 查看 workgraph 状态

**审计缺口:** #5 (P2) — 无法直接查看 workgraph 状态

**文件:**
- Modify: `src/daemon/proto.rs`（新增 `ClientRequest::WorkgraphStatus`、`ServerEvent::WorkgraphStatus`）
- Modify: `src/daemon/socket.rs`（路由 WorkgraphStatus）
- Modify: `src/client/mod.rs`（渲染 WorkgraphStatus）
- Modify: `src/bin/cc.rs`（新增 `cc workgraph` 入口）
- Reference: `src/workgraph.rs`（`WorkGraph::read` 获取状态）

**接口:**
- Consumes: `WorkGraph::read(&root)`（已有，读 `workgraph.json`）
- Produces: `ServerEvent::WorkgraphStatus { total, pending, done, needs_fix, blocked, last_advanced }`

- [ ] **Step 1: 新增协议类型**

在 `proto.rs` 的 `ClientRequest` 中：

```rust
    /// 查询 workgraph 状态（走 daemon 读 workgraph.json，不走 agent）。
    WorkgraphStatus,
```

在 `ServerEvent` 中：

```rust
    /// 当前 workgraph 状态快照。
    WorkgraphStatus(WorkgraphStatus),
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkgraphStatus {
    pub total: usize,
    pub pending: usize,
    pub done: usize,
    pub needs_fix: usize,
    pub blocked: usize,
    pub last_advanced: Option<String>,
}
```

- [ ] **Step 2: 在 socket.rs 中路由 WorkgraphStatus**

```rust
ClientRequest::WorkgraphStatus => {
    let root = mgr.lock().unwrap().root().to_path_buf();
    drop(mgr); // 释放锁，读磁盘不需要锁 mgr
    let g = crate::workgraph::WorkGraph::read(&root);
    let mut counts = (0usize, 0usize, 0usize, 0usize, 0usize); // pending, done, needs_fix, blocked
    for n in &g.nodes {
        match n.status {
            crate::workgraph::NodeStatus::Pending => counts.0 += 1,
            crate::workgraph::NodeStatus::Done => counts.1 += 1,
            crate::workgraph::NodeStatus::NeedsFix => counts.2 += 1,
            crate::workgraph::NodeStatus::Blocked => counts.3 += 1,
        }
    }
    let _ = body_tx.send(ServerEvent::WorkgraphStatus(WorkgraphStatus {
        total: g.nodes.len(),
        pending: counts.0,
        done: counts.1,
        needs_fix: counts.2,
        blocked: counts.3,
        last_advanced: None,
    }));
    let _ = body_tx.send(ServerEvent::TurnComplete);
}
```

- [ ] **Step 3: 在 `print_event` 中渲染 WorkgraphStatus**

```rust
ServerEvent::WorkgraphStatus(s) => {
    if s.total == 0 {
        println!("workgraph: (empty — seed workgraph.json first)");
    } else {
        println!("workgraph: {} milestones", s.total);
        println!("  pending:   {}", s.pending);
        println!("  done:      {}", s.done);
        println!("  needs_fix: {}", s.needs_fix);
        println!("  blocked:   {}", s.blocked);
        if let Some(ref t) = s.last_advanced {
            println!("  last:      {}", t);
        }
    }
    false
}
```

- [ ] **Step 4: 在 cc.rs 中添加 `cc workgraph` 入口**

```rust
[one] if one == "workgraph" => send_one(&sock, ClientRequest::WorkgraphStatus),
```

- [ ] **Step 5: 写测试**

```rust
#[test]
fn workgraph_status_round_trips() {
    let ws = WorkgraphStatus { total: 5, pending: 2, done: 2, needs_fix: 1, blocked: 0, last_advanced: None };
    let ev = ServerEvent::WorkgraphStatus(ws.clone());
    let json = serde_json::to_string(&ev).unwrap();
    let back: ServerEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(ev, back);
}
```

- [ ] **Step 6: 手动测试**

```bash
# 创建模拟 workgraph.json
echo '{"nodes":[{"id":1,"title":"x","status":"pending","deps":[],"fix_attempts":0}]}' > /tmp/wg.json
CODECODER_ROOT=/tmp CODECODER_DAEMON=1 cargo run &
sleep 1
CODECODER_ROOT=/tmp cargo run --bin cc -- workgraph
```

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "feat(cc): add 'cc workgraph' command to view milestone status"

Co-Authored-By: Claude <noreply@antic.com>
```

---

## Phase C: 可观测性 & 可配置性（P2 + P3）

### Task 6: run_command timeout 支持

**审计缺口:** #9 (P3) — 子进程无 timeout，命令永不退出时只能 Ctrl+C

**文件:**
- Modify: `src/tool/mod.rs`（`ToolCtx` 新增 timeout 字段）
- Modify: `src/tool/builtin.rs`（`RunCommand` 工具 schema 新增 `timeout_secs`参数，`run_shell_cancellable` 支持 timeout）
- Modify: `src/config.rs`（新增 `CODECODER_COMMAND_TIMEOUT_SECS`）
- Modify: `src/agent.rs`（`ToolCtx` 创建时传入 timeout）

**接口:**
- Consumes: `ToolCtx` 的 `cancel` 字段（已有）+ 新增 `timeout: Option<Duration>`
- Produces: `RunCommand` 工具新增可选参数 `timeout_secs: u32`

- [ ] **Step 1: 在 `Config` 中新增 `command_timeout_secs`**

```rust
// src/config.rs
pub struct Config {
    // ... 现有字段
    pub command_timeout_secs: u32,
}
```

```rust
// 在 from_env() 中
command_timeout_secs: env("CODECODER_COMMAND_TIMEOUT_SECS")
    .and_then(|v| v.parse().ok())
    .unwrap_or(0), // 0 = 无超时（向后兼容）
```

- [ ] **Step 2: 在 `ToolCtx` 中新增 timeout 字段**

```rust
pub struct ToolCtx<'a> {
    pub root: &'a Path,
    pub cancel: Option<&'a crate::agent::CancelToken>,
    /// 命令超时（0 = 无超时）。从 config 传入，run_command 工具可被参数覆盖。
    pub command_timeout: std::time::Duration,
}
```

更新 `new()` 和 `with_cancel()`：

```rust
pub fn new(root: &'a Path) -> Self {
    ToolCtx { root, cancel: None, command_timeout: std::time::Duration::from_secs(0) }
}
pub fn with_cancel(root: &'a Path, cancel: &'a crate::agent::CancelToken) -> Self {
    let cfg = crate::config::Config::from_env();
    ToolCtx { root, cancel: Some(cancel), command_timeout: std::time::Duration::from_secs(cfg.command_timeout_secs as u64) }
}
```

- [ ] **Step 3: 在 `RunCommand` schema 中新增 `timeout_secs` 可选参数**

```rust
fn schema(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "cmd": { "type": "string", "description": "The shell command line." },
            "timeout_secs": { "type": "integer", "description": "Timeout in seconds (0 = no timeout).", "default": 0 }
        },
        "required": ["cmd"]
    })
}
```

- [ ] **Step 4: 在 `run_shell_cancellable` 中支持 timeout**

在 poll 循环中同时检查 timeout：

```rust
pub(crate) fn run_shell_cancellable(mut command: Command, ctx: &ToolCtx) -> anyhow::Result<ToolOutput> {
    // ... 现有 spawn + pipe reader 代码 ...
    let deadline = if ctx.command_timeout.as_secs() > 0 {
        Some(std::time::Instant::now() + ctx.command_timeout)
    } else {
        None
    };
    let status = loop {
        if ctx.is_cancelled() {
            let _ = child.kill(); let _ = child.wait(); break None;
        }
        // timeout 检查
        if let Some(dead) = deadline {
            if std::time::Instant::now() >= dead {
                let _ = child.kill(); let _ = child.wait();
                return Ok(ToolOutput::err("timed out"));
            }
        }
        match child.try_wait()? {
            Some(status) => break Some(status),
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    // ... 后续代码不变 ...
}
```

- [ ] **Step 5: 在 `RunCommand::run` 中解析 timeout_secs 参数并覆盖 ctx 的 timeout**

```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    let cmd = args.get("cmd").and_then(Value::as_str).unwrap_or_default();
    let timeout_secs = args.get("timeout_secs").and_then(Value::as_u64).unwrap_or(0);
    let effective_timeout = if timeout_secs > 0 {
        std::time::Duration::from_secs(timeout_secs)
    } else {
        ctx.command_timeout
    };
    let mut ctx_override = ToolCtx {
        root: ctx.root,
        cancel: ctx.cancel,
        command_timeout: effective_timeout,
    };
    // 使用 ctx_override 而非 ctx
    // ...
}
```

注意：`ctx` 是 `&mut ToolCtx`，但 `run_shell_cancellable` 接受 `&ToolCtx`。可以传递 `&ctx_override`。

- [ ] **Step 6: 写测试**

```rust
#[test]
fn run_command_with_timeout_kills_child() {
    use crate::tool::builtin::RunCommand;
    use crate::tool::{ToolCtx, Tool};
    use serde_json::json;
    use std::sync::Arc;
    use crate::agent::CancelToken;

    let cancel = CancelToken::default();
    let dir = std::env::temp_dir().join("cc_timeout_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = ToolCtx::with_cancel(&dir, &cancel);
    ctx.command_timeout = std::time::Duration::from_millis(100); // 100ms 超时

    let tool = RunCommand;
    let result = tool.run(
        json!({"cmd": "sleep 10", "timeout_secs": 0}), // 不用参数，用 ctx 的 timeout
        &mut ctx,
    ).unwrap();
    assert!(result.content.contains("timed out"), "should timeout: {}", result.content);
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "feat(tool): add run_command timeout support via timeout_secs param"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

### Task 7: 硬编码间隔可配置化（P3）

**审计缺口:** #6 (P2) workgraph 推进间隔、#10 (P3) OnDemand reaper、#11 (P3) Supervisor supervise 周期

**文件:**
- Modify: `src/config.rs`（新增 3 个 env 变量）
- Modify: `src/daemon/mod.rs`（workgraph tick 间隔、supervisor tick 间隔从 config 读取）
- Modify: `src/tool/builtin.rs`（OnDemand reaper 延迟从 config 读取）

**接口:**
- Consumes: `Config` 的新字段
- Produces: 三个新环境变量

- [ ] **Step 1: 在 Config 中新增 3 个字段**

```rust
pub struct Config {
    // ... 现有字段
    /// daemon workgraph 推进线程间隔（秒）。默认 30。
    pub wg_tick_secs: u64,
    /// Supervisor 监督线程间隔（秒）。默认 1。
    pub supervisor_tick_secs: u64,
    /// OnDemand capability 自动 reaper 延迟（秒）。默认 5。
    pub ondemand_reaper_secs: u64,
}
```

在 `from_env()` 中：

```rust
wg_tick_secs: env("CODECODER_WG_TICK_SECS")
    .and_then(|v| v.parse().ok())
    .unwrap_or(30),
supervisor_tick_secs: env("CODECODER_SUPERVISOR_TICK_SECS")
    .and_then(|v| v.parse().ok())
    .unwrap_or(1),
ondemand_reaper_secs: env("CODECODER_ONDEMAND_REAPER_SECS")
    .and_then(|v| v.parse().ok())
    .unwrap_or(5),
```

- [ ] **Step 2: 在 `Daemon::run()` 中使用配置的间隔**

```rust
// workgraph 推进线程
let wg_tick = std::time::Duration::from_secs(self.cfg.wg_tick_secs);
// ... 在 sleep 处用 wg_tick 替代硬编码的 secs(30)

// supervisor 监督线程
let sup_tick = std::time::Duration::from_secs(self.cfg.supervisor_tick_secs);
// ... 在 sleep 处用 sup_tick 替代硬编码的 secs(1)
```

- [ ] **Step 3: 在 `run_ondemand` 中使用配置的 reaper 延迟**

在 `src/tool/builtin.rs` 的 `run_ondemand` 函数中，找到硬编码的 `std::time::Duration::from_secs(5)`，改为从 `Config::from_env()` 读取：

```rust
let cfg = crate::config::Config::from_env();
let reap_delay = std::time::Duration::from_secs(cfg.ondemand_reaper_secs);
```

- [ ] **Step 4: 写测试 — 验证 Config 默认值**

```rust
#[test]
fn config_interval_defaults() {
    let cfg = crate::config::Config::from_env();
    assert_eq!(cfg.wg_tick_secs, 30);
    assert_eq!(cfg.supervisor_tick_secs, 1);
    assert_eq!(cfg.ondemand_reaper_secs, 5);
}
```

- [ ] **Step 5: 手动测试**

```bash
CODECODER_WG_TICK_SECS=10 CODECODER_DAEMON=1 cargo run
# 验证 workgraph 推进间隔变为 10s
```

- [ ] **Step 6: 提交**

```bash
git add -A
git commit -m "feat(config): make hardcoded intervals configurable via env vars"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

### Task 8: daemon 后台线程可观测（P2）

**审计缺口:** #3 (P2) — daemon 5 个后台线程均无状态暴露

**文件:**
- Modify: `src/daemon/mod.rs`（每个后台线程记录心跳到共享状态）
- Modify: `src/daemon/proto.rs`（`DaemonStatus` 扩展线程状态字段）
- Modify: `src/daemon/socket.rs`（Status 路由收集线程状态）

**接口:**
- Consumes: 共享 `Arc<Mutex<DaemonThreadStatus>>` 结构
- Produces: `ServerEvent::Status` 包含线程状态信息

- [ ] **Step 1: 定义线程状态共享结构**

在 `src/daemon/mod.rs` 中（或 `proto.rs` 中）：

```rust
/// 可被 daemon 后台线程定期更新的心跳状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStatus {
    pub name: &'static str,
    pub last_tick: Option<u64>, // unix timestamp secs
    pub tick_count: u64,
    pub last_event: String,
}
```

- [ ] **Step 2: 创建共享状态并注入每个后台线程**

```rust
// 在 Daemon::run() 中
let thread_status = Arc::new(Mutex::new(Vec::<ThreadStatus>::new()));
```

监控线程：

```rust
let ts = Arc::clone(&thread_status);
thread::spawn(move || {
    let mut count = 0u64;
    while !shutdown.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
        count += 1;
        let mut status = ts.lock().unwrap();
        if let Some(s) = status.iter_mut().find(|s| s.name == "monitor") {
            s.last_tick = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
            s.tick_count = count;
        }
    }
});
```

其他线程类似（supervisor、workgraph、reload）。

- [ ] **Step 3: 在 `DaemonStatus` 中扩展线程状态字段**

```rust
pub struct DaemonStatus {
    // ... 现有字段
    pub threads: Vec<ThreadStatus>,
}
```

- [ ] **Step 4: 在 `Daemon::run()` 的循环中定期更新 thread_status 到 mgr**

或将 `thread_status` 的 Arc 传给 `DaemonSessionManager`，在 `status()` 方法中收集。

- [ ] **Step 5: 在 `print_event` 的 Status 渲染中显示线程状态**

```rust
for t in &s.threads {
    println!("  thread: {} ({} ticks, last: {:?})", t.name, t.tick_count, t.last_event);
}
```

- [ ] **Step 6: 提交**

```bash
git add -A
git commit -m "feat(daemon): expose background thread heartbeat in cc status"

Co-Authored-By: Claude <noreply@anthropic.com>
```

---

## 任务依赖关系

```
Task 1 (--help/--version)        ← 无依赖
Task 2 (cc help)                 ← 无依赖
Task 3 (cc status)               ← 无依赖，但 Task 4 会增强其 supervisor 字段
Task 4 (cc services)             ← Task 3（共享 Status 框架）
Task 5 (cc workgraph)            ← 无依赖
Task 6 (run_command timeout)     ← 无依赖
Task 7 (间隔可配置)              ← 无依赖
Task 8 (线程可观测)              ← Task 3（依赖 Status 框架）

可并行执行的分组：
  A: Task 1, 2, 3, 5, 6, 7    ← 全部无依赖，可并行
  B: Task 4                    ← 依赖 Task 3 的 Status 框架
  C: Task 8                    ← 依赖 Task 3 的 Status 框架
```

## 测试策略

- 每个命令的协议层测试：序列化/反序列化 round-trip（`src/daemon/proto.rs` 测试块）
- 每个命令的手动端到端测试：启动 daemon → 发命令 → 验证输出
- Task 6 的 timeout 测试：用 `sleep 10` 配合 100ms timeout 验证超时 kill
- Task 7 的配置测试：验证 `Config::from_env()` 默认值 + 环境变量覆盖

## 文档更新

所有任务完成后，更新 `README.md` 的环境变量表，新增：
- `CODECODER_WG_TICK_SECS`
- `CODECODER_SUPERVISOR_TICK_SECS`
- `CODECODER_ONDEMAND_REAPER_SECS`
- `CODECODER_COMMAND_TIMEOUT_SECS`

更新 `README.md` 的 CLI 命令列表，新增：
- `cc help`
- `cc services`
- `cc workgraph`