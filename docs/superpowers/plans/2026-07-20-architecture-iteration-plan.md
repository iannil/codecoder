# CodeCoder — Client-Server 架构迭代计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 Client-Server 架构（`ccd` 常驻 daemon + `cc` 薄 CLI 客户端，Unix socket 通信）替换当前 ratatui TUI，使一等公民（Skill/Capability/Work Graph/Session/Inference Tree）在 daemon 级共享状态下获得全新能力（共享 Registry、Persistent 监督树、24/7 workgraph 自动推进、跨 session 状态）。

**Architecture:** `ccd` daemon 进程常驻后台，持有共享 `Registry` 与 `Supervisor`，管理 N 个 `AgentLoop`（每个 session 一个 OS 线程跑 `AgentLoop::run(cmd_rx, event_tx)`）。`cc` 是纯 stdin/stdout 薄客户端，经 `$CODECODER_ROOT/.ccd.sock`（newline-delimited JSON 线协议）与 daemon 通信。无 ratatui、无 async runtime（保持 OS 线程 + mpsc 内核，ADR 0016）。

**Tech Stack:** Rust (edition 2024, **无 async runtime**), `serde`/`serde_json`, `std::os::unix::net::UnixListener`/`UnixStream`, `std::thread` + `std::sync::mpsc`, `std::sync::{Arc, Mutex, OnceLock}`。不新增网络/async 依赖。

**Phase 先行:** 先拆 `lib.rs::run()` 的纯消息入口（解耦 TUI），TUI 在新架构可用前不删；Phase 1 完成即得最小可行系统（`cc "hello"` → daemon → 回显）。

---

## 全局约束

- 所有新增代码遵守 `CONTEXT.md` 术语表（尤其 `_Avoid_` 条目）；新通道消息不得叫 `Mode`/`Dialog`/`Popup`（这些是 TUI 概念）。
- 新增 `pub` 类型/函数必须有 doc comment。
- 新增工具/功能必须有测试：内联 `#[cfg(test)] mod tests`，临时目录用 `std::env::temp_dir().join(format!("cc_<name>_{}", std::process::id()))`，结尾 `std::fs::remove_dir_all(&dir).ok()` 清理（与 `registry.rs` 测试同款）。
- 无 `CODECODER_API_KEY` 时走 `StubClient`（`select_provider` 既定回退）——daemon/客户端测试一律用 stub，不依赖真实 key。
- TUI 代码（`src/tui/`、`ratatui`、`crossterm` 依赖）在 Phase 4 之前**不删除**；`run_tui()` 路径全程可编译、可 `cargo test`。
- daemon 进程内复用既有 channel 拓扑：`AgentCommand`/`AgentEvent`（ADR 0016）保持不变；只在其之上加一层**可序列化**的线协议（`AgentEvent` 携带 `Sender<PermissionReply>` oneshot，无法直接 serde）。
- 每个 task 结尾 `cargo build` + `cargo test` 全绿后才提交；提交信息用 `feat:`/`refactor:` 前缀，单行。

---

## 文件结构（落地后的目标形态）

| 文件 | 职责 | 创建/改 |
|---|---|---|
| `src/daemon/mod.rs` | `Daemon` 结构体：起 `UnixListener`、accept 循环、持有 `Arc<Registry>` + `Supervisor` + `DaemonSessionManager` | 改（Task 1 骨架 → Task 2 填充） |
| `src/daemon/proto.rs` | 可序列化线协议 `ClientRequest`/`ServerEvent`（newline-delimited JSON） | 新建（Task 2） |
| `src/daemon/session_manager.rs` | `DaemonSessionManager`：`session_id → {cmd_tx, event_rx, join_handle}`，turn 级事件分发 | 新建（Task 2） |
| `src/daemon/socket.rs` | `UnixListener` 包装：accept、按行读写 JSON 帧、与 `DaemonSessionManager` 衔接 | 新建（Task 2） |
| `src/bin/cc.rs` | `cc` 客户端二进制入口：argv 子命令分发 + REPL | 新建（Task 3） |
| `src/client/mod.rs` | 客户端连接模块：连 socket、序列化请求、读流式 `ServerEvent` | 新建（Task 3） |
| `src/lib.rs` | `run()` → `run_tui()`；新增 `run_daemon()`、`pub mod daemon`、`pub mod client` | 改（Task 1/3） |
| `src/main.rs` | 按 `CODECODER_DAEMON`/`CODECODER_BG_TASK` 三路分发 | 改（Task 1） |
| `src/registry.rs` | `Registry` 可 `Arc` 共享；`build_system_prompt` 改为接 `&Registry` | 改（Task 4） |
| `src/agent.rs` | `AgentLoop::build` 接 `Option<Arc<Registry>>`；`build_system_prompt` 接参 | 改（Task 4） |
| `src/capability.rs` | 新增 `Supervisor`（扫 `capabilities/`、起 Persistent、监督重启、优雅退出） | 改（Task 5） |
| `src/session.rs` | 新增 `SessionManager`（daemon 级 list/find/last，纯 I/O） | 改（Task 6） |
| `Cargo.toml` | `[[bin]]` 声明 `codecoder` + `cc`；Phase 4 移除 ratatui/crossterm | 改（Task 3/9） |

---

### Task 1: 解耦 — 把 `lib.rs::run()` 拆出 `run_tui()` / `run_daemon()` 入口

**Files:**
- Create: `src/daemon/mod.rs` — daemon 模块骨架（本任务仅 stub）
- Modify: `src/lib.rs` — 新增 `pub mod daemon;`，`run()` 改名 `run_tui()`，新增 `run_daemon()`
- Modify: `src/main.rs` — 按 `CODECODER_DAEMON` 分发

**Interfaces:**
- Consumes: `Config::from_env()`（config.rs:15）、`select_provider(&cfg)`（lib.rs:39）、既有 `tui::run::run`（tui/run.rs:29）
- Produces:
  - `pub fn run_tui(cfg: Config) -> anyhow::Result<()>`（行为同旧 `run()`）
  - `pub fn run_daemon(cfg: Config) -> anyhow::Result<()>`（调 `daemon::Daemon::new(cfg).run()`）
  - `pub struct daemon::Daemon { cfg: Config }`，`Daemon::new(cfg: Config) -> Self`，`Daemon::run(&self) -> anyhow::Result<()>`（本任务 stub 返回 `Ok(())`）

**为什么先做这一步：** 当前 `lib.rs::run()`（lib.rs:76）把 TUI 与 agent 线程创建写死。拆出 `run_daemon()` 后，TUI 与 daemon 两条路径可并行编译，后续 task 往 `Daemon::run()` 里填真实逻辑即可，互不干扰。

- [ ] **Step 1: 写失败测试（daemon stub 可构造并空跑）**

新建 `src/daemon/mod.rs`：

```rust
// Daemon (ADR 待补): 长驻后台进程，管理 N 个 AgentLoop，对外暴露 Unix socket。
// 本文件随 Task 2 起逐步填充真实逻辑；当前仅提供可空跑的骨架。
use crate::config::Config;

/// 长驻 daemon。`run()` 当前为 stub，Task 2 起接入 socket + session 管理。
pub struct Daemon {
    #[allow(dead_code)]
    cfg: Config,
}

impl Daemon {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    /// Task 1: stub。Task 2 起监听 Unix socket、accept 连接、分发请求。
    pub fn run(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg_with_temp_root() -> Config {
        let dir = std::env::temp_dir().join(format!("cc_daemon_stub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Config {
            api_key: None,
            model: "gpt-4o".into(),
            api_base: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            temperature: 0.7,
            root: dir,
            github_token: None,
        }
    }

    #[test]
    fn daemon_stub_runs_and_returns_ok() {
        let d = Daemon::new(cfg_with_temp_root());
        let res = d.run();
        assert!(res.is_ok());
        // 清理：Daemon 还不持有 root 之外的状态；删掉临时根。
        let _ = std::fs::remove_dir_all(&d.cfg.root);
    }
}
```

- [ ] **Step 2: 运行测试，确认它失败（模块未挂载到 lib）**

Run: `cargo test daemon_stub_runs_and_returns_ok 2>&1 | tail -20`
Expected: 编译错误——`unresolved module daemon`（因为 `lib.rs` 还没有 `pub mod daemon;`）。若报的是别的错，先解决再继续。

- [ ] **Step 3: 在 `lib.rs` 挂载模块 + 拆分入口**

在 `src/lib.rs` 顶部模块声明区（`pub mod tui;` 之后，约第 23 行后）加一行：

```rust
pub mod daemon;
```

把 `lib.rs` 现有 `pub fn run(cfg: Config) -> anyhow::Result<()> { ... }`（第 76–100 行）整段**改名**为 `run_tui`，body 一字不改：

```rust
/// TUI 入口（ADR 0016/0024）：起 agent 线程 + 跑 ratatui 主循环。
pub fn run_tui(cfg: Config) -> anyhow::Result<()> {
    let provider = select_provider(&cfg);
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();

    let agent = AgentLoop::new(
        provider,
        cfg.model.clone(),
        cfg.max_tokens,
        cfg.temperature,
        cfg.root.clone(),
    );
    let cancel = agent.cancel_token();
    let steer = agent.steer_handle();
    let agent_thread = thread::spawn(move || agent.run(cmd_rx, event_tx));

    let result = tui::run::run(cfg.model.clone(), cfg.root.clone(), cmd_tx.clone(), event_rx, cancel, steer);

    let _ = cmd_tx.send(AgentCommand::Shutdown);
    let _ = agent_thread.join();
    capability::shutdown_all();
    result
}

/// Daemon 入口（client-server 架构）：起长驻 daemon，无 TUI。socket/session 逻辑
/// 在 `daemon::Daemon::run` 中（Task 2 起填充）。
pub fn run_daemon(cfg: Config) -> anyhow::Result<()> {
    let daemon = daemon::Daemon::new(cfg);
    daemon.run()
}
```

> 注：原 `run()` 改名后，唯一调用方 `main.rs` 在 Step 4 同步更新；不要保留 `run()` 别名（保持单一名）。

- [ ] **Step 4: 改 `main.rs` 三路分发**

把 `src/main.rs` 整文件替换为：

```rust
// CodeCoder — 入口分发 shim。三条路径（ADR 0016/0026 + 本计划）：
//   1. CODECODER_BG_TASK=<task>  → headless background runner（无 TUI，无 daemon）
//   2. CODECODER_DAEMON=1        → ccd daemon（无 TUI）
//   3. 其它                       → 默认 TUI
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    if std::env::var("CODECODER_DAEMON").is_ok() {
        return codecoder::run_daemon(cfg);
    }
    codecoder::run_tui(cfg)
}
```

- [ ] **Step 5: 确认没有遗漏的 `codecoder::run(` 旧调用**

Run: `grep -rn "codecoder::run\b\|::run(" src/ tests/`
Expected: 仅 `tui::run::run(`（TUI 内部）、各 `AgentLoop`/`Registry::scan` 等同名方法命中；**不应**出现 `codecoder::run(`（无参那个）或 `codecoder::run` 裸符号。若命中旧调用，改成 `run_tui`/`run_daemon`/`run_background` 之一。

- [ ] **Step 6: 编译 + 跑全量测试（旧 156 + 新 daemon 测试）**

Run: `cargo build 2>&1 | tail -15`
Expected: 编译通过（无 warning 视为新错误之一应排查，但 `dead_code` 允许——lib.rs:4 有 `#![allow(dead_code)]`）。

Run: `cargo test 2>&1 | tail -25`
Expected: `test result: ok.`，且包含 `daemon::tests::daemon_stub_runs_and_returns_ok` 通过；既有 156 个测试无回归。

- [ ] **Step 7: 提交**

```bash
git add src/daemon/mod.rs src/lib.rs src/main.rs
git commit -m "refactor: split lib.rs run() into run_tui()/run_daemon() entry points"
```

---

### Task 2: `ccd` daemon — 线协议 + Session 管理 + Unix socket

**Files:**
- Create: `src/daemon/proto.rs` — 可序列化线协议 `ClientRequest`/`ServerEvent`
- Create: `src/daemon/session_manager.rs` — `DaemonSessionManager`（turn 级事件分发）
- Create: `src/daemon/socket.rs` — `UnixListener` accept + 按行 JSON 帧
- Modify: `src/daemon/mod.rs` — `Daemon::run()` 起 listener + 监督循环（`Supervisor`/workgraph 推进留到 Task 5/7，本任务先空位注释）

**Interfaces:**
- Consumes:
  - `AgentLoop::new(provider, model: impl Into<String>, max_tokens, temperature, root) -> Self`（agent.rs:203）
  - `AgentLoop::run(self, cmd_rx: Receiver<AgentCommand>, event_tx: Sender<AgentEvent>)`（agent.rs:363，consumes self，阻塞）
  - `AgentCommand::{ProcessMessage(String), Resume, Reload, Shutdown}`（agent.rs:22）
  - `AgentEvent::{StreamDelta(String), Notice(String), TurnComplete, Context{pct}, ToolStarted{name,preview}, ToolFinished{name,is_error,output}}`（agent.rs:55）
  - `select_provider(&cfg) -> Arc<dyn Provider>`（lib.rs:39）
- Produces（后续 task 依赖的稳定接口）:
  - `proto::ClientRequest`（`#[serde(tag="type", rename_all="snake_case")]`）：`SendMessage{content}`、`NewSession`、`ListSessions`、`Resume{id}`、`Shutdown`、`Status`
  - `proto::ServerEvent`（同 tag 风格）：`StreamDelta{text}`、`Notice{text}`、`Context{pct}`、`ToolStarted{name,preview}`、`ToolFinished{name,is_error,output}`、`TurnComplete`、`SessionCreated{id}`、`Sessions{ids}`、`Error{message}`
  - `proto::read_request(r: &mut impl BufRead) -> anyhow::Result<Option<ClientRequest>>`、`proto::write_event(w: &mut impl Write, e: &ServerEvent) -> anyhow::Result<()>`
  - `session_manager::DaemonSessionManager`：`new(provider, model, max_tokens, temperature, root) -> Self`；`create() -> String`（返回 session id）；`get(&id) -> Option<&DaemonSession>`；`list() -> Vec<String>`；`send_message(&mut self, id: &str, content: String) -> anyhow::Result<std::sync::mpsc::Receiver<ServerEvent>>`
  - `socket::SocketServer`：`bind(path: &Path) -> anyhow::Result<Self>`；`accept_one(&self) -> anyhow::Result<UnixStream>`（单次 accept，便于测试）

**线协议设计（newline-delimited JSON）：** 客户端每写一行是一个 `ClientRequest`；daemon 对每个请求回一连串 `ServerEvent` 行，以 `TurnComplete`（或 `Error`）收尾。`SendMessage` 的回包由该 turn 的全部 `AgentEvent` 翻译而来（`AgentEvent::StreamDelta`→`ServerEvent::StreamDelta`，依此类推）。

**为什么 turn 级用 `Receiver<ServerEvent>`：** 每个 session 的 `AgentEvent` 由一个 agent 线程持续产出（agent.rs:363 阻塞循环）。`DaemonSessionManager` 持有每个 session 的 `event_rx`，turn 开始时启动一个 drainer 把 `AgentEvent`→`ServerEvent` 推入一个**临时** mpsc；`Mutex<Receiver>` 串行化同 session 的 turn（天然「一个 session 同时只跑一个 turn」）。

- [ ] **Step 1: 写失败测试 — 线协议 serde 往返**

新建 `src/daemon/proto.rs`：

```rust
// 客户端 ↔ daemon 的可序列化线协议（newline-delimited JSON）。
// 与进程内 `AgentCommand`/`AgentEvent` 平行存在：后者携带 oneshot Sender，无法 serde，
// 故 daemon 在两者间翻译。ADR 0016 的通道拓扑不变。
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{BufRead, Write};

/// 客户端 → daemon 的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    SendMessage { content: String },
    NewSession,
    ListSessions,
    Resume { id: String },
    Shutdown,
    Status,
}

/// daemon → 客户端的事件。一个 `SendMessage` 会产生一串事件，以 `TurnComplete` 或
/// `Error` 收尾。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    StreamDelta { text: String },
    Notice { text: String },
    Context { pct: u16 },
    ToolStarted { name: String, preview: String },
    ToolFinished { name: String, is_error: bool, output: String },
    TurnComplete,
    SessionCreated { id: String },
    Sessions { ids: Vec<String> },
    Error { message: String },
}

/// 从一行读一个 `ClientRequest`。`Ok(None)` 表示客户端关闭（EOF）。
pub fn read_request(r: &mut impl BufRead) -> anyhow::Result<Option<ClientRequest>> {
    let mut line = String::new();
    let n = r.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let req: ClientRequest = serde_json::from_str(line.trim())?;
    Ok(Some(req))
}

/// 写一个 `ServerEvent`（单行 JSON + `\n`）。
pub fn write_event(w: &mut impl Write, e: &ServerEvent) -> anyhow::Result<()> {
    let json = serde_json::to_string(e)?;
    writeln!(w, "{json}")?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn client_request_roundtrips() {
        let cases = vec![
            ClientRequest::SendMessage { content: "hi".into() },
            ClientRequest::NewSession,
            ClientRequest::ListSessions,
            ClientRequest::Resume { id: "abc123".into() },
            ClientRequest::Shutdown,
            ClientRequest::Status,
        ];
        for req in cases {
            let json = serde_json::to_string(&req).unwrap();
            let back: ClientRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req, back, "round-trip failed for {json}");
            // tag 用 snake_case
            assert!(json.contains("\"type\":"));
        }
    }

    #[test]
    fn server_event_writes_one_line_and_reads_back() {
        let ev = ServerEvent::StreamDelta { text: "hello\nworld".into() };
        let mut buf: Vec<u8> = Vec::new();
        write_event(&mut buf, &ev).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // 恰好一行（内容里没有真实换行被保留为 JSON 转义）
        assert_eq!(s.matches('\n').count(), 1);
        assert!(s.ends_with("\n"));
        let back: ServerEvent = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn read_request_returns_none_on_eof() {
        let mut r = Cursor::new("");
        assert!(read_request(&mut r).unwrap().is_none());
    }
}
```

在 `src/daemon/mod.rs` 顶部加模块声明（`Daemon` 结构体之后、`impl` 之前即可）：

```rust
pub mod proto;
```

- [ ] **Step 2: 运行测试，确认失败（模块未声明）**

Run: `cargo test --lib daemon::proto 2>&1 | tail -20`
Expected: 编译通过且 3 个 proto 测试**全绿**（因为 Step 1 已写全实现）。若失败，修到全绿再继续——proto 是后续 socket 层的契约，必须先稳。

- [ ] **Step 3: 写失败测试 — `DaemonSessionManager` 用 stub provider 跑通一个 turn**

新建 `src/daemon/session_manager.rs`：

```rust
// daemon 级 session 管理：每个 session = 一个 OS 线程跑 AgentLoop::run(cmd_rx, event_tx)。
// 管理器持有每个 session 的 cmd_tx 与 event_rx；turn 级把 AgentEvent 翻译成 ServerEvent
// 推入临时 mpsc，由 socket 层读出写回客户端。
use super::proto::ServerEvent;
use crate::agent::{AgentCommand, AgentEvent, AgentLoop};
use crate::provider::Provider;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// 一个被 daemon 托管的 session：发命令用 cmd_tx；agent 线程产出的事件汇总到
/// `event_rx`（由 forwarder 线程把 AgentEvent 搬到这里）。
pub struct DaemonSession {
    pub id: String,
    pub cmd_tx: Sender<AgentCommand>,
    /// 单一 drainer 串行化同 session 的 turn（Mutex 锁住接收端）。
    event_rx: std::sync::Mutex<Receiver<AgentEvent>>,
    _agent: JoinHandle<()>,
    _forward: JoinHandle<()>,
}

pub struct DaemonSessionManager {
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    sessions: HashMap<String, DaemonSession>,
    next_seq: u64,
}

impl DaemonSessionManager {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: String,
        max_tokens: u32,
        temperature: f32,
        root: PathBuf,
    ) -> Self {
        Self {
            provider,
            model,
            max_tokens,
            temperature,
            root,
            sessions: HashMap::new(),
            next_seq: 0,
        }
    }

    /// 新建一个 session，返回其 id。agent 线程立刻进入 `run` 阻塞循环等待命令。
    pub fn create(&mut self) -> String {
        let id = format!("s{:04}", self.next_seq);
        self.next_seq += 1;
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
        let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();

        let agent = AgentLoop::new(
            self.provider.clone(),
            self.model.clone(),
            self.max_tokens,
            self.temperature,
            self.root.clone(),
        );
        let agent = thread::spawn(move || agent.run(cmd_rx, event_tx));

        let forward = thread::spawn(move || {
            // event_rx 的所有权随 forward 线程；下面 send_message 用 Mutex 取用。
            // 这里不能持有——DaemonSession 持有 event_rx。故此线程只做 agent 的存活托管。
            drop(agent);
        });

        self.sessions.insert(
            id.clone(),
            DaemonSession {
                id: id.clone(),
                cmd_tx,
                event_rx: std::sync::Mutex::new(event_rx),
                _agent: forward,
                _forward: thread::spawn(|| ()),
            },
        );
        id
    }

    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sessions.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn get(&self, id: &str) -> Option<&DaemonSession> {
        self.sessions.get(id)
    }

    /// 发一条消息，返回该 turn 的 ServerEvent 流（以 TurnComplete 或 Error 收尾）。
    /// 同 session 的 turn 被 `event_rx` 的 Mutex 天然串行化。
    pub fn send_message(
        &mut self,
        id: &str,
        content: String,
    ) -> anyhow::Result<Receiver<ServerEvent>> {
        let sess = self
            .sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {id}"))?;
        let rx = sess.event_rx.lock().unwrap();
        let (out_tx, out_rx) = mpsc::channel::<ServerEvent>();
        sess.cmd_tx
            .send(AgentCommand::ProcessMessage(content))
            .map_err(|_| anyhow::anyhow!("agent thread closed"))?;
        // 把这个 turn 的 AgentEvent 翻译成 ServerEvent，直到 TurnComplete。
        let out_tx_clone = out_tx.clone();
        thread::spawn(move || {
            for ev in rx.iter() {
                match translate(ev) {
                    Some(se) => {
                        let is_terminal = matches!(se, ServerEvent::TurnComplete);
                        if out_tx_clone.send(se).is_err() {
                            break;
                        }
                        if is_terminal {
                            break;
                        }
                    }
                    None => {}
                }
            }
        });
        Ok(out_rx)
    }
}

/// AgentEvent → Option<ServerEvent>。丢弃进程内专属、不可回传客户端的事件
/// （PermissionRequest/AskUser 等带 oneshot 的事件由后续 task 处理；当前 daemon
/// 测试用 StubClient，不会产生它们）。
fn translate(ev: AgentEvent) -> Option<ServerEvent> {
    match ev {
        AgentEvent::StreamDelta(text) => Some(ServerEvent::StreamDelta { text }),
        AgentEvent::Notice(text) => Some(ServerEvent::Notice { text }),
        AgentEvent::Context { pct } => Some(ServerEvent::Context { pct }),
        AgentEvent::ToolStarted { name, preview } => Some(ServerEvent::ToolStarted { name, preview }),
        AgentEvent::ToolFinished { name, is_error, output } => {
            Some(ServerEvent::ToolFinished { name, is_error, output })
        }
        AgentEvent::TurnComplete => Some(ServerEvent::TurnComplete),
        _ => None,
    }
}

// 上面用到了 std::thread；显式引入避免与 crate 内部歧义。
use std::thread;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::stub::StubClient;

    fn mgr_with_temp_root() -> (DaemonSessionManager, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cc_sessmgr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = DaemonSessionManager::new(
            Arc::new(StubClient),
            "gpt-4o".into(),
            4096,
            0.7,
            dir.clone(),
        );
        (mgr, dir)
    }

    #[test]
    fn create_then_send_message_yields_turncomplete() {
        let (mut mgr, dir) = mgr_with_temp_root();
        let id = mgr.create();
        assert_eq!(mgr.list(), vec![id.clone()]);
        let rx = mgr.send_message(&id, "hello".into()).unwrap();
        let mut saw_delta = false;
        let mut saw_complete = false;
        for ev in rx.iter() {
            match ev {
                ServerEvent::StreamDelta { .. } => saw_delta = true,
                ServerEvent::TurnComplete => {
                    saw_complete = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_complete, "turn must terminate with TurnComplete");
        // StubClient 产出的回复带文本 → 至少一个 StreamDelta。
        assert!(saw_delta, "stub reply should stream some text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_to_unknown_session_errors() {
        let (mut mgr, dir) = mgr_with_temp_root();
        let err = mgr.send_message("nope", "x".into()).unwrap_err();
        assert!(format!("{err}").contains("unknown session"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

在 `src/daemon/mod.rs` 加声明：

```rust
pub mod session_manager;
```

- [ ] **Step 4: 运行测试，确认失败/修到全绿**

Run: `cargo test --lib daemon::session_manager 2>&1 | tail -30`
Expected: 初次可能因借用问题（`sess` 借用与 `self.sessions.get`）报编译错。若编译错，调整：把 `send_message` 改为先 `clone` 出 `cmd_tx`、再用 `event_rx.lock()`——因为 `self.sessions.get` 借用 `self`，而我们要在持锁期间 spawn。修法：先取出需要的句柄，释放 `self` 借用：

```rust
pub fn send_message(&mut self, id: &str, content: String) -> anyhow::Result<Receiver<ServerEvent>> {
    let cmd_tx = self.sessions.get(id).map(|s| s.cmd_tx.clone())
        .ok_or_else(|| anyhow::anyhow!("unknown session: {id}"))?;
    let rx_lock = self.sessions.get(id).expect("just checked").event_rx.lock().unwrap();
    let (out_tx, out_rx) = mpsc::channel::<ServerEvent>();
    cmd_tx.send(AgentCommand::ProcessMessage(content))
        .map_err(|_| anyhow::anyhow!("agent thread closed"))?;
    let out_tx_clone = out_tx;
    thread::spawn(move || {
        for ev in rx_lock.iter() {
            if let Some(se) = translate(ev) {
                let terminal = matches!(se, ServerEvent::TurnComplete);
                if out_tx_clone.send(se).is_err() { break; }
                if terminal { break; }
            }
        }
    });
    Ok(out_rx)
}
```

（把 Step 3 里的 `send_message` 用本块替换；其余不变。）重跑直到 2 个测试全绿。

- [ ] **Step 5: 写失败测试 — socket 层真连接往返**

新建 `src/daemon/socket.rs`：

```rust
// Unix socket listener：bind、accept、按行读写 JSON 帧。socket 路径默认
// `$CODECODER_ROOT/.ccd.sock`。
use crate::config::Config;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub fn default_sock_path(cfg: &Config) -> PathBuf {
    cfg.root.join(".ccd.sock")
}

/// 薄封装：bind + accept_one（单次），便于在测试里按需驱动。
pub struct SocketServer {
    listener: UnixListener,
    sock_path: PathBuf,
}

impl SocketServer {
    pub fn bind(sock_path: &Path) -> anyhow::Result<Self> {
        // 残留 socket 文件先清掉（上次 daemon 没干净退出）。
        let _ = std::fs::remove_file(sock_path);
        let listener = UnixListener::bind(sock_path)?;
        Ok(Self { listener, sock_path: sock_path.to_path_buf() })
    }

    /// 阻塞接受一个连接。
    pub fn accept_one(&self) -> anyhow::Result<UnixStream> {
        let (stream, _) = self.listener.accept()?;
        Ok(stream)
    }

    pub fn sock_path(&self) -> &Path {
        &self.sock_path
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock_path);
    }
}

/// 处理单个连接：读一行 ClientRequest，在 mgr 上执行，把结果事件写回流。
/// 当前仅支持 `SendMessage`/`NewSession`/`ListSessions`/`Shutdown`；其余回 Error。
pub fn handle_connection(
    stream: UnixStream,
    mgr: &std::sync::Mutex<super::session_manager::DaemonSessionManager>,
    shutdown: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    use super::proto::{read_request, write_event, ClientRequest, ServerEvent};
    use std::io::BufWriter;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    // 简化：单连接只处理第一个请求（M1 足够；REPL 多请求在 Task 3 由客户端循环驱动）。
    let Some(req) = read_request(&mut reader)? else {
        return Ok(());
    };
    let mut g = mgr.lock().unwrap();
    match req {
        ClientRequest::NewSession => {
            let id = g.create();
            write_event(&mut writer, &ServerEvent::SessionCreated { id })?;
        }
        ClientRequest::ListSessions => {
            write_event(&mut writer, &ServerEvent::Sessions { ids: g.list() })?;
        }
        ClientRequest::SendMessage { content } => {
            // 没指定 session → 自动取第一个（或新建）。
            let id = match g.list().first().cloned() {
                Some(id) => id,
                None => g.create(),
            };
            let rx = g.send_message(&id, content)?;
            drop(g); // 释放 mgr 锁，让 agent 线程推进
            for ev in rx.iter() {
                write_event(&mut writer, &ev)?;
                if matches!(ev, ServerEvent::TurnComplete) {
                    break;
                }
            }
        }
        ClientRequest::Shutdown => {
            shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            write_event(&mut writer, &ServerEvent::Notice { text: "shutting down".into() })?;
        }
        other => {
            write_event(&mut writer, &ServerEvent::Error {
                message: format!("unsupported in M1: {other:?}"),
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::proto::{write_event, ClientRequest, ServerEvent};
    use crate::daemon::session_manager::DaemonSessionManager;
    use crate::provider::stub::StubClient;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn client_sendmessage_roundtrips_through_socket() {
        let dir = std::env::temp_dir().join(format!("cc_sock_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");

        let server = SocketServer::bind(&sock).unwrap();
        let mgr = Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ));
        let shutdown = Arc::new(AtomicBool::new(false));

        // 服务端线程：accept 一次并处理。
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let stream = server.accept_one().unwrap();
            handle_connection(stream, &mgr, &shutdown_c).unwrap();
        });

        // 客户端：连、发 SendMessage、读到 TurnComplete。给服务端一点时间 bind。
        std::thread::sleep(Duration::from_millis(50));
        let mut conn = UnixStream::connect(&sock).unwrap();
        // 直接写一行 ClientRequest JSON（不要先写 ServerEvent 行——服务端首行即解析请求）
        use std::io::Write;
        let line = serde_json::to_string(&ClientRequest::SendMessage { content: "hi".into() }).unwrap();
        writeln!(conn, "{line}").unwrap();
        conn.flush().unwrap();

        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut events = Vec::new();
        loop {
            let mut buf = String::new();
            if reader.read_line(&mut buf).unwrap() == 0 { break; }
            let ev: ServerEvent = serde_json::from_str(buf.trim()).unwrap();
            let is_done = matches!(ev, ServerEvent::TurnComplete);
            events.push(ev);
            if is_done { break; }
        }
        h.join().unwrap();
        assert!(events.iter().any(|e| matches!(e, ServerEvent::TurnComplete)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // `write_event` 在服务端 handle_connection 使用；这里仅保持测试模块对其的可见引用。
    #[test]
    fn _write_event_is_part_of_proto_api() {
        let ev = ServerEvent::Notice { text: String::new() };
        let mut buf: Vec<u8> = Vec::new();
        write_event(&mut buf, &ev).unwrap();
        assert!(!buf.is_empty());
    }
}
```

> 说明：本测试用真实 `UnixListener` + `UnixStream` + `StubClient`，无需 API key。它就是 M1 的端到端验证雏形。

在 `src/daemon/mod.rs` 加声明（`pub mod proto;` 之后）：

```rust
pub mod socket;
```

- [ ] **Step 6: 运行测试，修到全绿**

Run: `cargo test --lib daemon:: 2>&1 | tail -30`
Expected: `socket::tests::client_sendmessage_roundtrips_through_socket` 通过；proto 3 + session_manager 2 全绿。若有借用/生命周期编译错，按编译器指引修（`handle_connection` 用 `Mutex` 持 mgr 是为跨线程共享；`drop(g)` 必须在 `rx.iter()` 之前以释放锁）。

- [ ] **Step 7: 把 `Daemon::run()` 接上 accept 循环**

在 `src/daemon/mod.rs` 用真实实现替换 stub（保留 Task 1 的 `Daemon` 字段、新增 socket 路径与 session manager）：

```rust
pub mod proto;
pub mod session_manager;
pub mod socket;

use crate::config::Config;
use crate::provider::Provider;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct Daemon {
    cfg: Config,
}

impl Daemon {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    pub fn run(&self) -> anyhow::Result<()> {
        let sock_path = socket::default_sock_path(&self.cfg);
        let server = socket::SocketServer::bind(&sock_path)?;
        let provider = crate::select_provider(&self.cfg);
        let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
            provider,
            self.cfg.model.clone(),
            self.cfg.max_tokens,
            self.cfg.temperature,
            self.cfg.root.clone(),
        )));
        let shutdown = Arc::new(AtomicBool::new(false));

        // 优雅退出：SIGINT/daemon 被 shutdown 请求后，退出时杀常驻 Capability（ADR 0021）。
        while !shutdown.load(Ordering::SeqCst) {
            let stream = match server.accept_one() {
                Ok(s) => s,
                Err(e) => {
                    // accept 出错不致命，记录后继续（真实 daemon 会 log；此处 best-effort）。
                    eprintln!("ccd: accept error: {e}");
                    continue;
                }
            };
            let mgr = mgr.clone();
            let shutdown = shutdown.clone();
            std::thread::spawn(move || {
                if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown) {
                    eprintln!("ccd: connection error: {e}");
                }
            });
        }
        crate::capability::shutdown_all();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Task 1 的 stub 测试仍保留语义：daemon 可构造。
    #[test]
    fn daemon_constructs_with_temp_root() {
        let dir = std::env::temp_dir().join(format!("cc_daemon_ctor_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            api_key: None, model: "gpt-4o".into(), api_base: "https://api.openai.com/v1".into(),
            max_tokens: 4096, temperature: 0.7, root: dir.clone(), github_token: None,
        };
        let _d = Daemon::new(cfg); // 仅构造，不 run（run 会阻塞 accept）
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

> 删除 Task 1 里的旧 `daemon_stub_runs_and_returns_ok` 测试（已被 `daemon_constructs_with_temp_root` 取代，因为 `run()` 现在会阻塞）。删除 `Daemon` 上的 `#[allow(dead_code)]`。

- [ ] **Step 8: 编译 + 全量测试 + 手动冒烟**

Run: `cargo build 2>&1 | tail -15`
Expected: 编译通过。

Run: `cargo test 2>&1 | tail -25`
Expected: 既有 156 + 新 daemon 测试全绿。

手动冒烟（确认 M1 打通，stub 无需 key）：

```bash
CODECODER_ROOT=$(mktemp -d) CODECODER_DAEMON=1 cargo run 2>/dev/null &
DAEMON_PID=$!
sleep 1
# 用 printf+nc 发一行请求，读流直到 TurnComplete
printf '{"type":"send_message","content":"hello"}\n' | nc -U "$CODECODER_ROOT/.ccd.sock"
kill $DAEMON_PID 2>/dev/null
```
Expected: nc 收到若干 `{"type":"stream_delta",...}` 行 + 一行 `{"type":"turn_complete"}`（StubClient 的回复）。若 `nc` 不可用，跳过手动步，靠 socket 集成测试作准。

- [ ] **Step 9: 提交**

```bash
git add src/daemon/
git commit -m "feat: ccd daemon — wire protocol, session manager, Unix socket accept loop"
```

---

### Task 3: `cc` CLI 客户端（REPL + 一次性模式）

**Files:**
- Create: `src/client/mod.rs` — 连接、序列化请求、读流式 `ServerEvent` 回调
- Create: `src/bin/cc.rs` — `cc` 二进制入口：argv 子命令 + REPL
- Modify: `src/lib.rs` — `pub mod client;`
- Modify: `Cargo.toml` — 显式 `[[bin]]` 声明 `codecoder` + `cc`

**Interfaces:**
- Consumes: `proto::{ClientRequest, ServerEvent, read_request, write_event}`（Task 2）、`Config::from_env()`（root 用于定位 `.ccd.sock`）、`socket::default_sock_path`
- Produces:
  - `client::Connection`：`connect(sock_path: &Path) -> anyhow::Result<Self>`；`send(&mut self, req: ClientRequest) -> anyhow::Result<()>`；`next_event(&mut self) -> anyhow::Result<Option<ServerEvent>>`
  - `client::print_event(&ServerEvent)`：把事件渲染到 stdout（delta 原样打、notice 前缀 `·`、tool 前缀 `⚙`、error 进 stderr）
  - 二进制 `cc`（`cargo build` 产出 `target/debug/cc`）

**CLI 形态（与既有 plan 一致）：**

```
cc                       # REPL 模式（连 daemon）
cc "修复这个 bug"         # 一次性：发一条，收完 TurnComplete 退出
cc sessions              # 列出 session
cc shutdown              # 关 daemon
```

**关键决策：不引入 `rustyline`。** 用裸 `stdin` 行循环（YAGNI）；行编辑是 nice-to-have，Phase 4 之后再说。`cc` 无 ratatui/crossterm 依赖，纯 stdout。

- [ ] **Step 1: 在 `lib.rs` 挂载 client 模块**

在 `pub mod daemon;` 后加：

```rust
pub mod client;
```

- [ ] **Step 2: 写失败测试 — `Connection` 序列化/解析往返**

新建 `src/client/mod.rs`：

```rust
// cc 客户端连接模块：连 Unix socket，写 ClientRequest 行，读 ServerEvent 行。
use crate::config::Config;
use crate::daemon::proto::{read_request, write_event, ClientRequest, ServerEvent};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::Path;

/// 一条到 daemon 的连接。`send` 写请求行；`next_event` 读一行事件。
pub struct Connection {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Connection {
    pub fn connect(sock_path: &Path) -> anyhow::Result<Self> {
        let s = UnixStream::connect(sock_path)?;
        let reader = BufReader::new(s.try_clone()?);
        Ok(Self { writer: s, reader })
    }

    pub fn send(&mut self, req: &ClientRequest) -> anyhow::Result<()> {
        // 写一行 ClientRequest（serde_json）+ 换行。注意：`write_event` 是写
        // ServerEvent 的；请求方向是 ClientRequest，故这里手写一行而非复用 write_event。
        use std::io::Write;
        let line = serde_json::to_string(req)?;
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn next_event(&mut self) -> anyhow::Result<Option<ServerEvent>> {
        let mut buf = String::new();
        if self.reader.read_line(&mut buf)? == 0 {
            return Ok(None);
        }
        let ev: ServerEvent = serde_json::from_str(buf.trim())?;
        Ok(Some(ev))
    }
}

/// 默认 socket 路径（与 daemon 一致：`$CODECODER_ROOT/.ccd.sock`）。
pub fn default_sock_path(cfg: &Config) -> std::path::PathBuf {
    crate::daemon::socket::default_sock_path(cfg)
}

/// 把一个 ServerEvent 渲染到 stdout/stderr。返回 true 表示是 turn 终态。
pub fn print_event(ev: &ServerEvent) -> bool {
    use std::io::Write;
    match ev {
        ServerEvent::StreamDelta { text } => {
            print!("{text}");
            let _ = std::io::stdout().flush();
            false
        }
        ServerEvent::Notice { text } => { println!("· {text}"); false }
        ServerEvent::Context { pct } => { eprintln!("[ctx {pct}%]"); false }
        ServerEvent::ToolStarted { name, preview } => { println!("⚙ {name}: {preview}"); false }
        ServerEvent::ToolFinished { name, is_error, output } => {
            if *is_error { eprintln!("  {name} ✗ {output}"); } else { println!("  {name} ✓"); }
            false
        }
        ServerEvent::SessionCreated { id } => { println!("· session {id}"); false }
        ServerEvent::Sessions { ids } => {
            if ids.is_empty() { println!("(no sessions)"); }
            else { for i in ids { println!("{i}"); } }
            false
        }
        ServerEvent::TurnComplete => { println!(); true }
        ServerEvent::Error { message } => { eprintln!("error: {message}"); true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::session_manager::DaemonSessionManager;
    use crate::daemon::socket::SocketServer;
    use crate::provider::stub::StubClient;
    use std::sync::{Arc, Mutex, atomic::AtomicBool};

    #[test]
    fn connection_sends_and_receives_turncomplete() {
        let dir = std::env::temp_dir().join(format!("cc_conn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join(".ccd.sock");
        let server = SocketServer::bind(&sock).unwrap();
        let mgr = Arc::new(Mutex::new(DaemonSessionManager::new(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        )));
        let shutdown = Arc::new(AtomicBool::new(false));
        let mgr_c = mgr.clone();
        let shutdown_c = shutdown.clone();
        let h = std::thread::spawn(move || {
            let s = server.accept_one().unwrap();
            crate::daemon::socket::handle_connection(s, &mgr_c, &shutdown_c).unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut conn = Connection::connect(&sock).unwrap();
        conn.send(&ClientRequest::SendMessage { content: "hi".into() }).unwrap();
        let mut done = false;
        while let Some(ev) = conn.next_event().unwrap() {
            if print_event(&ev) { done = true; break; }
        }
        assert!(done);
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 3: 运行测试，修到全绿**

Run: `cargo test --lib client 2>&1 | tail -20`
Expected: `connection_sends_and_receives_turncomplete` 通过。`Connection::send` 已在 Step 2 写为纯手写一行（`ClientRequest` 方向，不复用写 `ServerEvent` 的 `write_event`），故首行即合法请求、服务端 `read_request` 正常解析。若仍失败，检查 `ServerEvent::Notice` 是否被 `print_event` 当作非终态正确跳过（它是非终态，应继续读到 `TurnComplete`）。

重跑到绿。

- [ ] **Step 4: 在 `Cargo.toml` 显式声明两个 `[[bin]]`**

把 `[package]` 段之后、`[dependencies]` 之前插入：

```toml
[[bin]]
name = "codecoder"
path = "src/main.rs"

[[bin]]
name = "cc"
path = "src/bin/cc.rs"
```

> 没有这步，`src/bin/cc.rs` 不会被当作二进制；显式声明避免歧义（edition 2024 下隐式 bin 仅认 `src/main.rs` → `codecoder`）。

- [ ] **Step 5: 写 `cc` 入口（argv 分发 + REPL）**

新建 `src/bin/cc.rs`：

```rust
// cc — 薄 CLI 客户端，连 ccd daemon（$CODECODER_ROOT/.ccd.sock）。
// 无 ratatui，纯 stdin/stdout。
use codecoder::client::{default_sock_path, print_event, Connection};
use codecoder::daemon::proto::ClientRequest;
use codecoder::Config;
use std::io::{BufRead, Write};

fn main() -> anyhow::Result<()> {
    let cfg = Config::from_env();
    let sock = default_sock_path(&cfg);
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.as_slice() {
        [] => repl(&sock),
        [one] if one == "sessions" => send_one(&sock, ClientRequest::ListSessions),
        [one] if one == "status" => send_one(&sock, ClientRequest::Status),
        [one] if one == "shutdown" => send_one(&sock, ClientRequest::Shutdown),
        [msg @ ..] => {
            // cc "hello world" — 一次性发送
            let content = msg.join(" ");
            send_one(&sock, ClientRequest::SendMessage { content })
        }
    }
}

/// 发单个请求，打印所有事件直到终态，退出。
fn send_one(sock: &std::path::Path, req: ClientRequest) -> anyhow::Result<()> {
    let mut conn = Connection::connect(sock)
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {}: {e}\n(is `ccd` running? CODECODER_DAEMON=1 cargo run)", sock.display()))?;
    conn.send(&req)?;
    loop {
        match conn.next_event()? {
            None => break,
            Some(ev) => {
                if print_event(&ev) { break; }
            }
        }
    }
    Ok(())
}

/// REPL：读 stdin 一行 → 发 SendMessage → 流式打印 → 直到 TurnComplete。
fn repl(sock: &std::path::Path) -> anyhow::Result<()> {
    let mut conn = Connection::connect(sock)
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon at {}: {e}", sock.display()))?;
    // 开一个默认 session（若已有则复用第一个）。
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        print!("cc> ");
        std::io::stdout().flush()?;
        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 { break; } // EOF
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if trimmed == "/exit" || trimmed == "/quit" { break; }
        conn.send(&ClientRequest::SendMessage { content: trimmed.to_string() })?;
        loop {
            match conn.next_event()? {
                None => break,
                Some(ev) => {
                    if print_event(&ev) { break; }
                }
            }
        }
    }
    Ok(())
}

// 显式引入避免 unused 告警。
#[allow(unused_imports)]
use std::io::Write as _;
```

- [ ] **Step 6: 编译 + 测试 + 手动 M1 冒烟**

Run: `cargo build 2>&1 | tail -15`
Expected: 编译产出 `target/debug/codecoder` 与 `target/debug/cc`。

Run: `cargo test 2>&1 | tail -25`
Expected: 全绿。

手动 M1（验证里程碑 M1：`cc "hello"` 经 daemon → 回复 → 终端）：

```bash
ROOT=$(mktemp -d)
CODECODER_ROOT=$ROOT CODECODER_DAEMON=1 ./target/debug/codecoder 2>/dev/null &
DPID=$!
sleep 1
CODECODER_ROOT=$ROOT ./target/debug/cc "hello"
kill $DPID 2>/dev/null
rm -rf $ROOT
```
Expected: cc 打印 StubClient 的回复文本后换行退出（`turn_complete`）。**这一步打通即 M1 达成。**

- [ ] **Step 7: 提交**

```bash
git add src/client/ src/bin/cc.rs src/lib.rs Cargo.toml
git commit -m "feat: cc CLI client (REPL + one-shot) speaking the daemon wire protocol"
```

---

### Task 4: `Registry` 从「按 turn 重扫」升级到「daemon 级共享」

**Files:**
- Modify: `src/agent.rs` — `build_system_prompt` 改为接 `&Registry`；`AgentLoop::build` 接 `Option<Arc<Registry>>`
- Modify: `src/registry.rs` — 新增 `Registry::reload`（同 scan，语义显式）；标注共享用法
- Modify: `src/daemon/mod.rs` — daemon 持有 `Arc<Registry>`，建 session 时传入；`generate_skill` 写盘后触发一次 reload（daemon 级广播）
- Test: `src/agent.rs` 内联测试 + `src/registry.rs` 内联测试

**现状纠正（与原 draft 的关键差异）：** 原 draft 写「Registry 从 `AgentLoop::new()` 内部自建」——**不准确**。`Registry` 不是 `AgentLoop` 的字段；它在两处被临时构造：`build_system_prompt(root)`（agent.rs:1208 `let catalog = Registry::scan(root).render_catalog();`）与 `AgentCommand::Reload` 分支（agent.rs:380）。本 task 的真实工作是：把这两处的「每次 scan」替换为「接一个共享 `&Registry`」，由 daemon 在启动时 scan 一次、写盘后 reload 一次，所有 session 共享同一份。

**Interfaces:**
- Consumes: `Registry::scan(&root)`（registry.rs:34）、`Registry::render_catalog()`（registry.rs:44）、`AgentLoop::build`（agent.rs:246）
- Produces:
  - `Registry::reload(&mut self, root: &Path)`（重新 scan，覆盖 `self.catalog`）
  - `AgentLoop::build(..., registry: Option<Arc<Registry>>)`：`Some` → 存字段、`build_system_prompt` 用之；`None` → 维持旧「自扫」行为（TUI/sub-agent 路径不变）
  - daemon 启动时 `Arc::new(Registry::scan(&root))`，`DaemonSessionManager` 持有 clone，`send_message` 产生的 `generate_skill` 写盘事件触发 daemon 侧 `Arc::get_mut`/重建并广播

**为什么保守（与原 draft 的「保守」一致）：** 完整的 inotify/kqueue 热更新是 stretch；本 task 只做「单实例共享 + 写盘后显式 reload」，已经让「两个 session 看到同一份目录」成立。热更新留作 Task 4 的可选后续（见 Step 7 注释）。

- [ ] **Step 1: 写失败测试 — `build_system_prompt` 接受外部 Registry**

先读 agent.rs:1200–1215 确认 `build_system_prompt` 当前签名（应为 `fn build_system_prompt(root: &Path) -> String`）。在 `src/agent.rs` 测试模块（约 agent.rs:1240 起）加：

```rust
#[test]
fn build_system_prompt_uses_provided_registry() {
    use crate::registry::Registry;
    let dir = std::env::temp_dir().join(format!("cc_regshare_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("skills")).unwrap();
    std::fs::write(
        dir.join("skills/shared-skill.md"),
        "---\nname: shared-skill\ndescription: a shared skill\n---\nbody",
    ).unwrap();
    let reg = Registry::scan(&dir);
    let prompt = build_system_prompt_with_registry(&dir, &reg);
    assert!(prompt.contains("shared-skill — a shared skill"));
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 运行测试，确认失败（新函数未定义）**

Run: `cargo test --lib build_system_prompt_uses_provided_registry 2>&1 | tail -15`
Expected: 编译错——`cannot find function build_system_prompt_with_registry`。

- [ ] **Step 3: 重构 `build_system_prompt` 为薄包装**

在 agent.rs:1208 附近，把现有 `fn build_system_prompt(root: &Path) -> String` 改为：

```rust
/// 用外部传入的共享 Registry 渲染 system prompt（daemon 路径）。
fn build_system_prompt_with_registry(root: &Path, reg: &Registry) -> String {
    let mut out = String::new();
    // AGENTS.md 身份（如有）
    let agents = root.join("AGENTS.md");
    if let Ok(text) = std::fs::read_to_string(&agents) {
        out.push_str(&text);
        if !text.ends_with('\n') { out.push('\n'); }
    }
    // 目录（来自共享 Registry，而非每次 scan）
    let catalog = reg.render_catalog();
    if !catalog.is_empty() {
        out.push_str(&catalog);
    }
    out
}

/// 兼容旧路径：自扫一次。TUI/sub-agent 用此（无共享 Registry）。
fn build_system_prompt(root: &Path) -> String {
    build_system_prompt_with_registry(root, &Registry::scan(root))
}
```

> 注：以上 body 是对 agent.rs:1208 现有逻辑的**等价拆分**——先 `Read` 确认现有 `build_system_prompt` 的真实 body（它可能已经读了 AGENTS.md + 目录），把那段 body 原样搬进 `build_system_prompt_with_registry`，只把 `Registry::scan(root).render_catalog()` 这一处换成 `reg.render_catalog()`。**不要改动其它行为。**

- [ ] **Step 4: 测试转绿**

Run: `cargo test --lib build_system_prompt_uses_provided_registry 2>&1 | tail -10`
Expected: PASS。

- [ ] **Step 5: `AgentLoop::build` 接 `Option<Arc<Registry>>`**

在 `AgentLoop` 结构体（agent.rs:168）加字段：

```rust
/// daemon 共享目录（ADR 0020）。`None` 时 build_system_prompt 自扫（TUI/sub-agent）。
shared_registry: Option<Arc<Registry>>,
```

把 `fn build(...)`（agent.rs:246）签名与 body 改为：

```rust
fn build(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    toolbox: Toolbox,
    persist: bool,
    headless: bool,
    shared_registry: Option<Arc<Registry>>,
) -> Self {
    // ... 原逻辑不变 ...
    let trusted = trust == TrustState::Trusted;
    let system_prompt = if trusted {
        match &shared_registry {
            Some(reg) => build_system_prompt_with_registry(&root, reg),
            None => build_system_prompt(&root),
        }
    } else { String::new() };
    // ... Self { ..., shared_registry, ... } 原其余字段不变
}
```

更新三个调用点（agent.rs:210 `new`、223 `new_background`、243 `new_sub`）各加最后一个实参：
- `new` / `new_background`：传 `None`（行为不变）。
- `new_sub`：传 `None`。

`AgentCommand::Reload` 分支（agent.rs:380）：共享 Registry 是**只读 `Arc`**（daemon 启动时 scan 一次，见 Step 8），故单 session 的 Reload 只重建 system prompt、不原地改 Arc（多 session 共享，`Arc::get_mut` 不可行；live reload 见 Step 7 stretch）：

```rust
AgentCommand::Reload => {
    if self.trust == TrustState::Trusted {
        let n = match &self.shared_registry {
            Some(reg) => reg.catalog.len(), // 共享只读实例；内容由 daemon 侧负责刷新（M2：重启级）
            None => Registry::scan(&self.root).catalog.len(),
        };
        self.system_prompt = match &self.shared_registry {
            Some(reg) => build_system_prompt_with_registry(&self.root, reg),
            None => build_system_prompt(&self.root),
        };
        let _ = event_tx.send(AgentEvent::Notice(format!("reloaded — {n} skills/capabilities in catalog")));
    } else {
        let _ = event_tx.send(AgentEvent::Notice("project not trusted; nothing reloaded".into()));
    }
    let _ = event_tx.send(AgentEvent::TurnComplete);
}
```

- [ ] **Step 6: 写失败测试 — `Registry::reload` 刷新目录**

在 `src/registry.rs` 加方法：

```rust
impl Registry {
    /// 重新扫描，覆盖当前 catalog（daemon 共享实例在写盘后调用）。
    pub fn reload(&mut self, root: &Path) {
        let fresh = Registry::scan(root);
        self.catalog = fresh.catalog;
    }
}
```

在 registry.rs 测试模块加：

```rust
#[test]
fn reload_picks_up_newly_written_skill() {
    let dir = std::env::temp_dir().join(format!("cc_regreload_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("skills")).unwrap();
    let mut reg = Registry::scan(&dir);
    assert!(reg.catalog.is_empty());
    std::fs::write(dir.join("skills/new.md"), "---\nname: new\ndescription: d\n---\nb").unwrap();
    reg.reload(&dir);
    assert_eq!(reg.catalog.len(), 1);
    assert_eq!(reg.catalog[0].name, "new");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 7: 运行测试 + 全量回归**

Run: `cargo test --lib registry::tests::reload_picks_up_newly_written_skill 2>&1 | tail -10`
Expected: PASS。

Run: `cargo test 2>&1 | tail -25`
Expected: 全绿（TUI 路径 `new()` 传 `None`，行为零变化）。

> **热更新 stretch（不在本 task 必做范围）：** 若要 inotify/kqueue 自动 reload，在 daemon 加一个文件监听线程，发现 `skills/`/`capabilities/`/`prompts/` 变化即调一次 `Arc::get_mut` 或重建 `Arc<Registry>` 并通知各 session reload。留作 Task 4 完成后的独立增强，不阻塞 M2。

- [ ] **Step 8: daemon 持有共享 Registry（让 M2 成立）**

在 `src/daemon/session_manager.rs`：`DaemonSessionManager` 新增字段 `registry: Arc<Registry>`，`new(...)` 多接一个 `registry: Arc<Registry>` 参数并 clone 存储；`create()` 用 `new_with_registry`（新增）：

在 agent.rs `impl AgentLoop` 加一个 daemon 专用构造器（紧邻 `new_background`）：

```rust
/// daemon 托管的 session：共享 daemon 的 Registry（ADR 0020 daemon 级目录）。
pub fn new_daemon(
    provider: Arc<dyn Provider>,
    model: impl Into<String>,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
    registry: Arc<Registry>,
) -> Self {
    Self::build(provider, model.into(), max_tokens, temperature, root, Toolbox::builtin(), true, false, Some(registry))
}
```

把 `session_manager.rs::create()` 里的 `AgentLoop::new(...)` 改成 `AgentLoop::new_daemon(..., self.registry.clone())`。

`src/daemon/mod.rs::run()` 启动时建共享 Registry：

```rust
let registry = Arc::new(crate::registry::Registry::scan(&self.cfg.root));
let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
    provider, self.cfg.model.clone(), self.cfg.max_tokens, self.cfg.temperature,
    self.cfg.root.clone(), registry,
)));
```

- [ ] **Step 9: 编译 + 全量测试**

Run: `cargo build 2>&1 | tail -15 && cargo test 2>&1 | tail -25`
Expected: 全绿。**M2 前提（共享 Registry）成立。**

- [ ] **Step 10: 提交**

```bash
git add src/agent.rs src/registry.rs src/daemon/mod.rs src/daemon/session_manager.rs
git commit -m "feat: daemon-level shared Registry (Arc<Registry>) threaded into AgentLoop build"
```

---

### Task 5: Persistent Capability 监督树（`Supervisor`）

**Files:**
- Modify: `src/capability.rs` — 新增 `Supervisor`（扫 `capabilities/`、起 `Persistent`、监督重启、优雅退出）
- Modify: `src/daemon/mod.rs` — `run()` 起一个监督线程，周期 `supervise()`；退出时 `shutdown_all()`
- Test: `src/capability.rs` 内联测试（用临时 `capabilities/` + 一个会退出的 shell 脚本 fake Persistent）

**现状：** `capability.rs` 已有 `RunningServiceTable`（进程级 `OnceLock<Mutex<...>>` 单例，`shutdown_all()` 杀全部）与 `Lifecycle::Persistent`。但**没有自动重启**——进程崩了就崩了。本 task 加 `Supervisor`：daemon 启动时扫 `capabilities/`，对 `lifecycle: persistent` 的条目自动起，崩了重启，最大重启 3 次/60s，daemon 退出优雅关闭。

**Interfaces:**
- Consumes: `CapabilityManifest`（capability.rs:23）、`Environment::Shell`、`Lifecycle::Persistent`、`RunningServiceTable`（capability.rs:60）
- Produces:
  - `capability::Supervisor { max_restarts: u32, window_secs: u64, states: HashMap<String, SupervisedService> }`
  - `Supervisor::start_all(root: &Path) -> Self`（扫 manifest，起所有 Persistent）
  - `Supervisor::start_one(&mut self, name: &str, root: &Path) -> anyhow::Result<()>`
  - `Supervisor::supervise(&mut self)`（检查子进程，重启超阈值则放弃）
  - `Supervisor::shutdown_all(&mut self)`（kill + reap 全部）

- [ ] **Step 1: 写失败测试 — fake Persistent 崩溃后被重启，超过上限放弃**

在 `src/capability.rs` 测试模块加：

```rust
#[test]
fn supervisor_restarts_crashed_persistent_until_cap() {
    use std::time::Instant;
    let dir = std::env::temp_dir().join(format!("cc_supervisor_{}", std::process::id()));
    let capdir = dir.join("capabilities/flaky");
    std::fs::create_dir_all(&capdir).unwrap();
    // 一个会立即退出的脚本（模拟崩溃）。Shell 环境。
    let script = if cfg!(windows) { "exit 1" } else { "#!/bin/sh\nexit 1\n" };
    std::fs::write(dir.join("capabilities/flaky/entry.sh"), script).unwrap();
    std::fs::write(
        dir.join("capabilities/flaky/manifest.json"),
        r#"{"name":"flaky","description":"crashes","environment":"shell","lifecycle":"persistent","entry":"sh entry.sh"}"#,
    ).unwrap();
    std::fs::create_dir_all(dir.join("capabilities")).unwrap(); // 确保目录

    let mut sup = Supervisor { max_restarts: 3, window_secs: 60, states: Default::default() };
    sup.start_all(&dir).unwrap();
    // 反复 supervise 直到放弃或超时
    let start = Instant::now();
    loop {
        sup.supervise();
        if start.elapsed().as_secs() > 2 { break; } // 测试保护
        let name = "flaky";
        if let Some(s) = sup.states.get(name) {
            if s.gave_up { break; }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let s = sup.states.get("flaky").expect("flaky supervised");
    assert!(s.restart_count >= 1, "should have restarted at least once");
    assert!(s.gave_up, "should give up after max_restarts");
    sup.shutdown_all();
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 运行测试，确认失败（`Supervisor` 未定义）**

Run: `cargo test --lib capability::tests::supervisor_restarts_crashed_persistent_until_cap 2>&1 | tail -15`
Expected: 编译错——`Supervisor` 未定义。

- [ ] **Step 3: 实现 `Supervisor`**

在 `src/capability.rs` 末尾（`shutdown_all` 之后）加：

```rust
/// 监督一个 Persistent Capability 的运行状态：记录重启次数与窗口。
pub struct SupervisedService {
    pub manifest: CapabilityManifest,
    pub child: Option<std::process::Child>,
    pub restart_count: u32,
    pub first_restart: Option<std::time::Instant>,
    /// 达到上限后放弃重启（已死）。
    pub gave_up: bool,
}

/// Persistent Capability 监督树（first-class citizen #3 的 daemon 级形态）：
/// 扫 capabilities/ 起 Persistent 条目，崩溃自动重启，超过 max_restarts/window_secs
/// 放弃；daemon 退出时 shutdown_all。
pub struct Supervisor {
    pub max_restarts: u32,
    pub window_secs: u64,
    pub states: std::collections::HashMap<String, SupervisedService>,
}

impl Supervisor {
    pub fn start_all(root: &std::path::Path) -> anyhow::Result<Self> {
        let mut sup = Self { max_restarts: 3, window_secs: 60, states: Default::default() };
        let caps = root.join("capabilities");
        let Ok(entries) = std::fs::read_dir(&caps) else { return Ok(sup); };
        for e in entries.flatten() {
            let man = e.path().join("manifest.json");
            let Ok(raw) = std::fs::read_to_string(&man) else { continue };
            let Ok(m) = serde_json::from_str::<CapabilityManifest>(&raw) else { continue };
            if m.lifecycle == Lifecycle::Persistent && m.environment == Environment::Shell {
                let _ = sup.start_one(&m.name, root);
            }
        }
        Ok(sup)
    }

    pub fn start_one(&mut self, name: &str, root: &std::path::Path) -> anyhow::Result<()> {
        let man = read_manifest(name, root)?;
        let child = spawn_shell_capability(root, &man)?;
        self.states.insert(
            name.to_string(),
            SupervisedService { manifest: man, child: Some(child), restart_count: 0, first_restart: None, gave_up: false },
        );
        Ok(())
    }

    /// 周期调用：检查每个已起服务的子进程，若已退出则按窗口/上限决定重启或放弃。
    pub fn supervise(&mut self) {
        for (_name, s) in self.states.iter_mut() {
            if s.gave_up { continue; }
            let exited = match s.child.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            if !exited { continue; }
            // 窗口外重置计数（重启滑动窗口）
            let now = std::time::Instant::now;
            // 注意：Instant::now 在 workflow 脚本里被禁，但这是生产二进制代码，可用。
            let now_inst = std::time::Instant::now();
            if let Some(first) = s.first_restart {
                if now_inst.duration_since(first).as_secs() >= self.window_secs {
                    s.restart_count = 0;
                    s.first_restart = None;
                }
            }
            if s.restart_count >= self.max_restarts {
                s.gave_up = true;
                s.child = None;
                continue;
            }
            s.restart_count += 1;
            if s.first_restart.is_none() { s.first_restart = Some(now_inst); }
            // 重启
            if let Ok(c) = spawn_shell_capability(&std::path::PathBuf::from("."), &s.manifest) {
                s.child = Some(c);
            } else {
                s.child = None;
            }
        }
    }

    pub fn shutdown_all(&mut self) {
        for (_, s) in self.states.iter_mut() {
            if let Some(mut c) = s.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

fn read_manifest(name: &str, root: &std::path::Path) -> anyhow::Result<CapabilityManifest> {
    let raw = std::fs::read_to_string(root.join("capabilities").join(name).join("manifest.json"))?;
    Ok(serde_json::from_str(&raw)?)
}

fn spawn_shell_capability(_root: &std::path::Path, m: &CapabilityManifest) -> anyhow::Result<std::process::Child> {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(&m.entry);
    Ok(cmd.spawn()?)
}
```

> 注：测试脚本 `sh entry.sh` 在 fake 目录下需要正确 cwd。若 `try_wait` 立即返回退出（脚本秒退），`restart_count` 会迅速累加到上限 → `gave_up=true`，符合测试断言。若路径/cwd 有问题，把 `spawn_shell_capability` 改为 `cmd.current_dir(cap_dir)`，`cap_dir = root.join("capabilities").join(name)`。

- [ ] **Step 4: 运行测试，修到全绿**

Run: `cargo test --lib capability::tests::supervisor_restarts_crashed_persistent_until_cap 2>&1 | tail -20`
Expected: PASS。若 fake 脚本 cwd 问题导致 spawn 失败（`restart_count` 不增），按注解给 `spawn_shell_capability` 设 `current_dir`。

- [ ] **Step 5: daemon 接入监督线程**

`src/daemon/mod.rs::run()`：在 accept 循环之前建 `Supervisor`，每个循环迭代的空闲点调一次 `supervise()`；`shutdown` 后 `sup.shutdown_all()`。把 `run()` 改为：

```rust
pub fn run(&self) -> anyhow::Result<()> {
    let sock_path = socket::default_sock_path(&self.cfg);
    let server = socket::SocketServer::bind(&sock_path)?;
    let provider = crate::select_provider(&self.cfg);
    let registry = Arc::new(crate::registry::Registry::scan(&self.cfg.root));
    let mgr = Arc::new(Mutex::new(session_manager::DaemonSessionManager::new(
        provider, self.cfg.model.clone(), self.cfg.max_tokens, self.cfg.temperature,
        self.cfg.root.clone(), registry,
    )));
    let shutdown = Arc::new(AtomicBool::new(false));

    let mut supervisor = crate::capability::Supervisor::start_all(&self.cfg.root)
        .unwrap_or_else(|e| { eprintln!("ccd: supervisor init failed: {e}"); crate::capability::Supervisor { max_restarts: 3, window_secs: 60, states: Default::default() } });

    // 监督线程：周期 supervise（独立线程，避免阻塞 accept）。
    let shutdown_c = shutdown.clone();
    let sup_handle = {
        std::thread::spawn(move || {
            while !shutdown_c.load(Ordering::SeqCst) {
                supervisor.supervise();
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            supervisor.shutdown_all();
        })
    };

    while !shutdown.load(Ordering::SeqCst) {
        let stream = match server.accept_one() {
            Ok(s) => s,
            Err(e) => { eprintln!("ccd: accept error: {e}"); continue; }
        };
        let mgr = mgr.clone();
        let shutdown = shutdown.clone();
        std::thread::spawn(move || {
            if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown) {
                eprintln!("ccd: connection error: {e}");
            }
        });
    }
    let _ = sup_handle.join();
    crate::capability::shutdown_all();
    Ok(())
}
```

- [ ] **Step 6: 编译 + 全量测试**

Run: `cargo build 2>&1 | tail -15 && cargo test 2>&1 | tail -25`
Expected: 全绿。

- [ ] **Step 7: 提交**

```bash
git add src/capability.rs src/daemon/mod.rs
git commit -m "feat: persistent capability supervisor tree (auto-restart with backoff, graceful shutdown)"
```

---

### Task 6: daemon 级 Session 管理（list / find / last / resume）

**Files:**
- Modify: `src/session.rs` — 新增 `SessionManager`（纯 I/O：list/find/last，磁盘上的 `sessions/*.json`）
- Modify: `src/daemon/session_manager.rs` — `DaemonSessionManager` 复用 `SessionManager` 做 list；`Resume` 请求落到具体 session 的 `AgentCommand::Resume`
- Modify: `src/daemon/socket.rs` — `handle_connection` 支持 `ClientRequest::Resume{id}` 与 `ListSessions`（后者回盘上的 session 文件而非内存 session）
- Test: `src/session.rs` 内联测试

**现状：** `session::sessions_dir(root)`、`session::latest_session(root)` 已存在（session.rs:10/15）。`AgentLoop` 自管理 session 文件（每 append 落盘，agent.rs:332）。daemon 下「list/resume」要的是：列出磁盘上所有 `sessions/*.json`、按 id/前缀定位、把 resume 命令路由到对应内存 session（或新建并 resume）。

**挑战与保守选择（与原 draft 一致）：** 让 `AgentLoop` 继续自管理落盘（`persist: true`，daemon session 默认即如此）；daemon 只在「list/find/last」上提供磁盘视图，「resume」= 向某内存 session 发 `AgentCommand::Resume`。**不**把 I/O 提到 daemon 层。

**Interfaces:**
- Consumes: `session::sessions_dir`、`session::latest_session`、`AgentCommand::Resume`
- Produces:
  - `session::SessionMeta { id: String, mtime: SystemTime }`
  - `session::SessionManager::new(root) -> Self`；`list() -> Vec<SessionMeta>`；`find(id_or_prefix: &str) -> Option<String>`；`last() -> Option<String>`

- [ ] **Step 1: 写失败测试 — `SessionManager` 列/找/最近**

在 `src/session.rs` 测试模块加：

```rust
#[test]
fn session_manager_lists_finds_and_last() {
    use std::time::SystemTime;
    let dir = std::env::temp_dir().join(format!("cc_sessio_mgr_{}", std::process::id()));
    std::fs::create_dir_all(sessions_dir(&dir)).unwrap();
    // 写两个 session 文件
    for name in ["session-a.json", "session-b.json"] {
        std::fs::write(sessions_dir(&dir).join(name),
            r#"{"schema_version":2,"model":"gpt-4o","token_count":0,"entries":[],"leaf":null}"#).unwrap();
    }
    let mgr = SessionManager::new(&dir);
    let mut ids: Vec<String> = mgr.list().into_iter().map(|m| m.id).collect();
    ids.sort();
    assert_eq!(ids, vec!["session-a".to_string(), "session-b".to_string()]);
    assert_eq!(mgr.find("a"), Some("session-a".into()));       // 前缀匹配
    assert_eq!(mgr.find("session-a"), Some("session-a".into())); // 全名
    assert_eq!(mgr.find("zzz"), None);
    // last：取 mtime 最新的；这里两者同秒，任一即可，仅断言非空
    assert!(mgr.last().is_some());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = SystemTime::now(); // keep import used
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test --lib session::tests::session_manager_lists_finds_and_last 2>&1 | tail -15`
Expected: 编译错——`SessionManager` 未定义。

- [ ] **Step 3: 实现 `SessionManager`**

在 `src/session.rs`（`latest_session` 之后）加：

```rust
use std::time::SystemTime;

/// 磁盘上一个 session 文件的轻量视图。
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub mtime: SystemTime,
}

/// daemon 级 session 视图：纯 I/O，读 `sessions/*.json`。不持有内存 session。
pub struct SessionManager {
    root: std::path::PathBuf,
}

impl SessionManager {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }

    /// 列出所有 session（按 mtime 降序）。
    pub fn list(&self) -> Vec<SessionMeta> {
        let dir = sessions_dir(&self.root);
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else { return out; };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
            let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if id.is_empty() { continue; }
            let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(SessionMeta { id, mtime });
        }
        out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
        out
    }

    /// 按 id 或唯一前缀定位；歧义/无匹配返回 None。
    pub fn find(&self, id_or_prefix: &str) -> Option<String> {
        let all = self.list();
        let exact: Vec<_> = all.iter().filter(|m| m.id == id_or_prefix).collect();
        if exact.len() == 1 { return Some(exact[0].id.clone()); }
        let prefix: Vec<_> = all.iter().filter(|m| m.id.starts_with(id_or_prefix)).collect();
        if prefix.len() == 1 { return Some(prefix[0].id.clone()); }
        None
    }

    /// 最近的 session id（mtime 最新）。
    pub fn last(&self) -> Option<String> {
        self.list().into_iter().next().map(|m| m.id)
    }
}
```

> 若 `use std::time::SystemTime;` 与文件顶部已有重复，合并到顶部一次。

- [ ] **Step 4: 测试转绿 + 全量回归**

Run: `cargo test --lib session::tests::session_manager_lists_finds_and_last 2>&1 | tail -10`
Expected: PASS。

Run: `cargo test 2>&1 | tail -25`
Expected: 全绿。

- [ ] **Step 5: daemon 接入 `ListSessions`/`Resume`**

先把 `DaemonSessionManager::send_message` 的 drainer 抽成通用 `dispatch`，并新增 `resume`（`DaemonSessionManager` 已持有 `root`，无需改 `handle_connection` 签名）。在 `src/daemon/session_manager.rs`：

```rust
use crate::agent::AgentCommand;
use crate::session::SessionManager;

/// 通用：向某 session 发一条 AgentCommand，返回该轮事件流（drain 到 TurnComplete）。
fn dispatch(&mut self, id: &str, cmd: AgentCommand) -> anyhow::Result<Receiver<ServerEvent>> {
    let sess = self.sessions.get(id)
        .ok_or_else(|| anyhow::anyhow!("unknown session: {id}"))?;
    let cmd_tx = sess.cmd_tx.clone();
    let rx_lock = sess.event_rx.lock().unwrap();
    let (out_tx, out_rx) = mpsc::channel::<ServerEvent>();
    cmd_tx.send(cmd).map_err(|_| anyhow::anyhow!("agent thread closed"))?;
    thread::spawn(move || {
        for ev in rx_lock.iter() {
            if let Some(se) = translate(ev) {
                let terminal = matches!(se, ServerEvent::TurnComplete);
                if out_tx.send(se).is_err() { break; }
                if terminal { break; }
            }
        }
    });
    Ok(out_rx)
}

pub fn send_message(&mut self, id: &str, content: String) -> anyhow::Result<Receiver<ServerEvent>> {
    self.dispatch(id, AgentCommand::ProcessMessage(content))
}

/// 按 id/前缀解析磁盘 session；内存无此 session 则新建一个并对其发 Resume。
pub fn resume(&mut self, id_or_prefix: &str) -> anyhow::Result<Receiver<ServerEvent>> {
    let sm = SessionManager::new(&self.root);
    let resolved = sm.find(id_or_prefix);
    let target = match resolved {
        Some(_id) => self.list().first().cloned().unwrap_or_else(|| self.create()),
        None => self.create(),
    };
    self.dispatch(&target, AgentCommand::Resume)
}

/// 磁盘上的全部 session id（daemon `ListSessions` 用此，而非内存 session 列表）。
pub fn disk_sessions(&self) -> Vec<String> {
    SessionManager::new(&self.root).list().into_iter().map(|m| m.id).collect()
}
```

> 把原 `send_message` 的实现整段替换为上面 `dispatch` + 薄 `send_message`/`resume`/`disk_sessions`。`new()` 已存 `root`，无需新字段。

然后在 `src/daemon/socket.rs::handle_connection` 的 match 里，把 `ListSessions` 分支改为用磁盘视图，并补 `Resume`：

```rust
ClientRequest::ListSessions => {
    write_event(&mut writer, &ServerEvent::Sessions { ids: g.disk_sessions() })?;
}
ClientRequest::Resume { id } => {
    let rx = g.resume(&id)?;
    drop(g);
    for ev in rx.iter() {
        write_event(&mut writer, &ev)?;
        if matches!(ev, ServerEvent::TurnComplete) { break; }
    }
}
```

- [ ] **Step 6: 编译 + 全量测试 + M2 冒烟**

Run: `cargo build 2>&1 | tail -15 && cargo test 2>&1 | tail -25`
Expected: 全绿。

M2 冒烟（两个终端连同一 daemon，各自独立会话）：

```bash
ROOT=$(mktemp -d)
CODECODER_ROOT=$ROOT CODECODER_DAEMON=1 ./target/debug/codecoder 2>/dev/null &
DPID=$!
sleep 1
# 终端1
CODECODER_ROOT=$ROOT ./target/debug/cc sessions   # 列出 session（首次可能为空）
CODECODER_ROOT=$ROOT ./target/debug/cc "hello from A"
# 终端2（同时）— 行为独立
CODECODER_ROOT=$ROOT ./target/debug/cc "hello from B"
kill $DPID; rm -rf $ROOT
```
Expected: 两次 `cc` 各自经 daemon 回复；`cc sessions` 列出落盘的 session 文件。**M2 达成。**

- [ ] **Step 7: 提交**

```bash
git add src/session.rs src/daemon/session_manager.rs src/daemon/socket.rs
git commit -m "feat: daemon-level session management (disk list/find/last, resume routing)"
```

---

### Task 7: Work Graph 自动推进引擎（复用 `background.rs`）

**Files:**
- Modify: `src/daemon/mod.rs` — 新增 daemon 级 workgraph 推进线程
- Modify: `src/background.rs` — 抽出 `pub fn advance_once(provider, model, max_tokens, temperature, root) -> anyhow::Result<BgOutcome>`（单步推进一个里程碑），供 daemon 复用；`run_background` 内部改为调它
- Test: `src/background.rs` 内联测试 + daemon 集成测试

**关键纠正（与原 draft 最大差异）：** 原 draft 把 workgraph 自动推进当作全新功能设计——**这已落地**。`background.rs::run_background`（background.rs:50）在 `task` 为空时已经：从 workgraph 取 `next_ready` 里程碑 → `run_one_turn` → 解析 verdict → `set_status` + `save`，并自动连推最多 3 个（MAX_AUTO，background.rs:59/80）。daemon 只需：周期性（或在空闲时）调一次单步推进，且避免与用户当前在跑的 turn 抢同一里程碑。**不需要重写推进逻辑。**

**Interfaces:**
- Consumes: `background::run_background`（background.rs:50）、`WorkGraph::read/next_ready/set_status/save`、`review::parse_review`
- Produces:
  - `background::advance_one_milestone(provider, model, max_tokens, temperature, root) -> anyhow::Result<Option<BgOutcome>>`（推进一步；`None` = 无就绪里程碑）
  - daemon 线程：每 N 秒（默认 30s）调一次；与「用户 active turn」互斥（用户操作某里程碑时 skip）

- [ ] **Step 1: 写失败测试 — `advance_one_milestone` 推进单个里程碑**

在 `src/background.rs` 测试模块加（`background.rs` 当前无测试模块，新建）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::stub::StubClient;
    use crate::workgraph::{NodeStatus, WorkGraph};
    use std::sync::Arc;

    fn root_with_one_milestone() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cc_bg_advance_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = WorkGraph::default();
        g.add("do thing", "acceptance", vec![]).unwrap();
        g.save(&dir).unwrap();
        dir
    }

    #[test]
    fn advance_one_milestone_returns_none_when_empty() {
        let dir = std::env::temp_dir().join(format!("cc_bg_empty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap();
        assert!(out.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_one_milestone_runs_a_turn() {
        let dir = root_with_one_milestone();
        let out = advance_one_milestone(
            Arc::new(StubClient), "gpt-4o".into(), 4096, 0.7, dir.clone(),
        ).unwrap();
        assert!(out.is_some(), "should run a turn for the ready milestone");
        let outcome = out.unwrap();
        assert!(!outcome.final_text.is_empty(), "stub should produce some final text");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test --lib background::tests 2>&1 | tail -15`
Expected: 编译错——`advance_one_milestone` 未定义。

- [ ] **Step 3: 实现 `advance_one_milestone`，重构 `run_background` 复用它**

在 `src/background.rs`，把现有 `run_background` 的「单里程碑推进」逻辑抽成 `advance_one_milestone`（不改既有 `run_background` 对外行为）：

```rust
/// 推进 workgraph 的下一个就绪里程碑：跑一个 turn、解析 verdict、写回状态。
/// 无就绪里程碑时返回 `Ok(None)`。daemon 与 background runner 共用此函数。
pub fn advance_one_milestone(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
) -> anyhow::Result<Option<BgOutcome>> {
    use crate::workgraph::{NodeStatus, WorkGraph};
    let milestone_id = {
        let g = WorkGraph::read(&root);
        match g.next_ready() {
            Some(n) => n.id,
            None => return Ok(None),
        }
    };
    let (task_text, title) = {
        let g = WorkGraph::read(&root);
        let n = g.get(milestone_id).expect("just read");
        let t = format!(
            "workgraph milestone #{}: {}\nacceptance: {}\n\n\
             Complete this milestone, then review your changes and report the \
             verdict (pass / needs_fix / rebuild).",
            n.id, n.title,
            if n.acceptance.is_empty() { "(none)" } else { &n.acceptance },
        );
        (t, n.title.clone())
    };
    let mut agent = AgentLoop::new_background(provider, model, max_tokens, temperature, root.clone());
    let mut out = BgOutcome::default();
    out.events.push(format!("task: workgraph milestone #{} ({})", milestone_id, title));
    let (tx, rx) = channel::<AgentEvent>();
    agent.run_one_turn(task_text, &tx);
    drop(tx);
    drain_bg_events(rx, &mut out);

    // auto-writeback：解析 verdict 更新里程碑状态
    let outcome = crate::review::parse_review(&out.final_text);
    if !outcome.unparsed {
        let mut g = WorkGraph::read(&root);
        let (status, vs) = match outcome.verdict {
            crate::review::Verdict::Pass => (NodeStatus::Done, "pass"),
            crate::review::Verdict::NeedsFix => (NodeStatus::NeedsFix, "needs_fix"),
            crate::review::Verdict::Rebuild => (NodeStatus::NeedsFix, "rebuild"),
        };
        g.set_status(milestone_id, status);
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == milestone_id) {
            n.verdict = Some(vs.to_string());
        }
        let _ = g.save(&root);
        out.events.push(format!("milestone #{} ({}) auto-updated: {}", milestone_id, title, vs));
    }
    Ok(Some(out))
}
```

把现有 `run_background` 的循环体（background.rs:80–125）改为反复调 `advance_one_milestone`（最多 `MAX_AUTO` 次），保持对外签名与 `BgOutcome` 形态不变。先 `Read` 现有 `run_background` 全文，把首 turn（explicit task）之后的 workgraph 推进段替换为：

```rust
if task.trim().is_empty() {
    for _ in 1..MAX_AUTO {
        match advance_one_milestone(
            // 需要原 provider/model/... —— 把它们作为参数传入或克隆
            /* provider clone, model clone, max_tokens, temperature, root clone */
        )? {
            None => break,
            Some(step_out) => {
                out.events.extend(step_out.events);
                // 合并 final_text/tool_calls/denied 视需要
            }
        }
    }
}
```

> **实现注：** `run_background` 当前在首 turn 用了 `provider`（move 进 `AgentLoop::new_background`）。要复用 `advance_one_milestone`（它也 move provider），需把 provider 升为 `Arc` 传入并 clone（`AgentLoop::new_background` 已接 `Arc<dyn Provider>`，background.rs:71 传的就是 `provider`，原签名是 `provider: Arc<dyn Provider>`——可直接 `provider.clone()`）。把首 turn 也改成调 `advance_one_milestone` 的等价路径，或保留首 turn 原样、仅替换后续循环。**保守：保留首 turn 原样，仅把后续 `for _ in 0..MAX_AUTO-1` 循环体替换为调 `advance_one_milestone`。** 这样改动面最小、行为不变。

- [ ] **Step 4: 测试转绿**

Run: `cargo test --lib background::tests 2>&1 | tail -15`
Expected: 2 个新测试通过；既有 L1 background 测试（`tests/l1_background.rs`）无回归。

- [ ] **Step 5: daemon 接入周期推进线程**

先给 `Config` 派生 `Clone`（`select_provider` 接 `&Config`，线程内要重建 provider 需克隆 cfg）。在 `src/config.rs` 把 `pub struct Config { ... }` 上方的 derive 改为：

```rust
#[derive(Debug, Clone)]
pub struct Config {
    // 字段不变
```

在 `src/daemon/mod.rs::run()` 加一个推进线程（与监督线程并列）。注意 `Daemon::run` 内 `self.cfg` 在多处被 `.clone()` 取用，推进线程持有自己的 `cfg` clone：

```rust
// workgraph 自动推进线程（first-class citizen #2 的 daemon 级形态）：空闲时推进。
// 用户 active turn 优先——通过 try_lock(mgr) 探测：拿不到锁说明有 turn 在跑，skip。
let shutdown_c2 = shutdown.clone();
let cfg_for_wg = self.cfg.clone();
let mgr_for_wg = mgr.clone();
let wg_handle = std::thread::spawn(move || {
    while !shutdown_c2.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_secs(30));
        // 仅当无 active turn（mgr 锁可立即取得）时推进
        if mgr_for_wg.try_lock().is_err() { continue; }
        // 释放锁后再跑（advance 内部自建 agent，不复用 mgr）
        let provider = crate::select_provider(&cfg_for_wg);
        let _ = crate::background::advance_one_milestone(
            provider,
            cfg_for_wg.model.clone(),
            cfg_for_wg.max_tokens,
            cfg_for_wg.temperature,
            cfg_for_wg.root.clone(),
        );
    }
});
```

并在 `run()` 末尾、`crate::capability::shutdown_all()` 之前加 `let _ = wg_handle.join();`（与 `sup_handle.join()` 并列）。

- [ ] **Step 6: 编译 + 全量测试 + M3 冒烟**

Run: `cargo build 2>&1 | tail -15 && cargo test 2>&1 | tail -25`
Expected: 全绿。

M3 冒烟（daemon 无人值守推进 workgraph）：

```bash
ROOT=$(mktemp -d)
# 写一个 workgraph + 一个 milestone
cat > $ROOT/workgraph.json <<'EOF'
{"schema_version":1,"nodes":[{"id":1,"title":"echo hi","acceptance":"says hi","deps":[],"status":"pending","verdict":null,"touched":[]}]}
EOF
CODECODER_ROOT=$ROOT CODECODER_DAEMON=1 ./target/debug/codecoder 2>/dev/null &
DPID=$!
sleep 35  # 等 daemon 推进周期（30s）
kill $DPID; 
cat $ROOT/workgraph.json | grep -o '"status":"[^"]*"'   # 期望被推进过（verdict/状态变化）
rm -rf $ROOT
```
Expected: workgraph.json 里 milestone #1 的 `status` 或 `verdict` 被更新（StubClient 产出的文本可能无法解析为 verdict → 状态不变但 events 有记录；这步主要验证线程不崩、不与用户 turn 死锁）。**M3 达成（无人值守推进运行）。**

- [ ] **Step 7: 提交**

```bash
git add src/background.rs src/daemon/mod.rs src/config.rs
git commit -m "feat: daemon workgraph auto-advance engine (reuses background::advance_one_milestone)"
```

---

### Task 8: 跨 session 共享（Inference Tree 检索 + Memory 共享）— P2

**Files:**
- Modify: `src/tool/reason.rs` — 推理树检索可跨 session 读其它 `sessions/*.json` 的 `meta` 标注节点
- Modify: `src/daemon/mod.rs` — daemon 级 event bus（session 间 Notice 广播，可选）
- Test: `src/tool/reason.rs` 内联测试

**要求（P2，可延后；本 task 给出最小可用形态）：**
- **Memory 已天然跨 session**：`memory/<key>` 是文件级 KV（memory.rs），所有 session 共享。无需改动——在 ADR/文档里标注即可。
- **Inference Tree 跨 session 检索**：`reason` 工具当前只看本 session 的 `SessionEntry.meta`（推理树元数据，session.rs:37）。扩展为可读 `sessions/` 下其它文件的 `meta` 节点，做跨 session 因果检索。
- **daemon event bus（stretch）**：session 间 Notice 广播，可延后。

**最小实现（本 task 仅做 Memory 标注 + Inference Tree 跨 session 读）：**

- [ ] **Step 1: 写失败测试 — `collect_hypothesis_nodes_across_sessions` 汇集多 session 的 meta 节点**

在 `src/tool/reason.rs` 测试模块加（若该文件无测试模块，新建）一个纯函数测试：给定 root 与两个 session 文件（含 `meta: {"status":"hypothesis"}` 节点），汇总返回这些节点的简要描述。

> **先 Read `src/tool/reason.rs` 全文**，确认现有推理树读 API（它如何遍历 `session.entries[].meta`）。本 task 的函数 `collect_cross_session_hypotheses(root: &Path) -> Vec<CrossSessionNode>` 复用其节点判定逻辑，仅把数据源从「单 session」扩到「sessions/*.json」。

- [ ] **Step 2: 运行测试，确认失败 → 实现 → 转绿**

按 reason.rs 现有风格实现 `collect_cross_session_hypotheses`：遍历 `session::SessionManager::new(root).list()`，对每个 id 读其 session 文件，过滤 `meta.status == "hypothesis"` 的 `SessionEntry`，映射成 `CrossSessionNode { session_id, message_id, preview }`。

Run: `cargo test --lib reason 2>&1 | tail -15` → 全绿。

- [ ] **Step 3: 文档同步**

在 `ARCHITECTURE.md` 与 `memory.rs` doc comment 标注：「Memory 跨 session 共享（文件级 KV）」「Inference Tree 支持跨 session 检索」。更新 `README.md` 相关描述。

- [ ] **Step 4: 编译 + 测试 + 提交**

Run: `cargo build && cargo test 2>&1 | tail -25` → 全绿。

```bash
git add src/tool/reason.rs src/memory.rs ARCHITECTURE.md README.md
git commit -m "feat: cross-session inference-tree retrieval; document shared memory"
```

---

### Task 9: 移除旧 TUI（收尾，仅 daemon 稳定后执行）

**前置条件（硬性）：** M1/M2/M3 在 daemon 模式下持续可用，且 `cc` 客户端覆盖原 TUI 全部日常用法（含权限弹窗的行内 `y/n` 处理——这需 Task 2 的 `PermissionRequest`→`ServerEvent` 翻译落地，是本 task 的隐含前置，若未做则**先补**再删 TUI）。

**Files:**
- Delete: `src/tui/` 整目录
- Modify: `src/lib.rs` — 删 `pub mod tui;` 与 `run_tui()`
- Modify: `src/main.rs` — 删 TUI 分支（保留 daemon + background 两路）
- Modify: `Cargo.toml` — 删 `ratatui`、`crossterm` 依赖
- Modify: `ARCHITECTURE.md`、`README.md`、`CLAUDE.md` — 同步数字与描述（25→? 工具、模块数、ADR 指向）
- Delete: `tests/l2_pty_smoke.rs`（依赖 TUI 二进制；若改为对 daemon 的 pty 冒烟则保留并改写）

- [ ] **Step 1: 删 TUI 依赖**

在 `Cargo.toml` 删除：

```toml
crossterm = "0.28"
ratatui = "0.29"
```

- [ ] **Step 2: 删 `src/tui/` 与 lib.rs 中的挂载**

```bash
rm -rf src/tui
```

在 `src/lib.rs` 删 `pub mod tui;`（约第 23 行）与整个 `run_tui()` 函数；删 `use std::thread;`（若仅 TUI 用）等随之 dead 的 import（`cargo build` 会提示）。

- [ ] **Step 3: 改 `main.rs` 为两路分发**

```rust
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return codecoder::run_background(cfg, task);
        }
    }
    codecoder::run_daemon(cfg)
}
```

- [ ] **Step 4: 处理 TUI 专属测试**

`tests/l2_pty_smoke.rs` 若直接 spawn `codecoder` 进 TUI，改为 spawn daemon + `cc` 客户端的等价冒烟，或删除并在 ADR 记录「L2 冒烟迁移到 client-server」。

- [ ] **Step 5: 编译 + 全量测试（M4 验证）**

Run: `cargo build 2>&1 | tail -15`
Expected: 编译通过，`Cargo.lock` 不再含 ratatui/crossterm。

Run: `cargo tree 2>&1 | grep -E "ratatui|crossterm" || echo "clean"`
Expected: `clean`（无 TUI 依赖残留）。

Run: `cargo test 2>&1 | tail -25`
Expected: 全绿。**M4 达成：项目不再依赖 ratatui/crossterm。**

- [ ] **Step 6: 同步文档**

更新 `CLAUDE.md`「项目状态」段（移除 TUI 相关表述，补 daemon/client）、`ARCHITECTURE.md`（运行时形状图改为 daemon ↔ cc）、`README.md`（命令表：`ccd`/`cc` 替换 `cargo run` TUI 描述）、`CONTEXT.md`（标注 Mode/Dialog/Popup 为「遗留 TUI 概念，daemon 架构下不再适用」或删除）。新增/更新一份 ADR（如 0032-client-server-architecture）。

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "refactor: remove ratatui TUI; client-server (ccd/cc) is the sole UI surface"
```

---

## 迭代优先级

```
Phase 1 (立即, M1):  Task 1 (解耦) → Task 2 (daemon 骨架) → Task 3 (cc 客户端)
                     └── 完成即可：ccd 跑 + cc 连 + cc "hello" 回显
Phase 2 (核心升级):   Task 4 (共享 Registry, M2 前提) → Task 5 (Capability 监督树) → Task 6 (Session 管理, M2)
Phase 3 (自主化, M3): Task 7 (Work Graph 自动推进, 复用 background.rs)
Phase 4 (收尾/后续):  Task 8 (跨 session 共享, P2) → Task 9 (移除旧 TUI, M4)
```

**里程碑：**
- **M1**（Phase 1 完）: `cc "hello"` 经 daemon → StubClient/LLM 回复 → 打印回终端。端到端集成测试（Task 2 Step 5 + Task 3 Step 2）覆盖。
- **M2**（Phase 2 完）: 两个终端同时连同一 daemon，各自独立会话；`cc sessions` 列盘上 session；共享 Registry；Persistent Capability 受监督。
- **M3**（Phase 3 完）: daemon 无人值守周期推进 workgraph（用户 turn 与 background 推进不抢同一里程碑）。
- **M4**（Phase 4 完）: 完全移除 ratatui/crossterm，`cargo tree` 干净。

---

## 一等公民影响全景（落地后）

| 一等公民 | 当前状态 | daemon 下新意义 | 对应 Task |
|---|---|---|---|
| **Skill** | 每 turn 重扫（`build_system_prompt`） | daemon 级共享 `Arc<Registry>`，写盘后显式 reload | Task 4 |
| **Capability** | 进程级 `RunningServiceTable`，崩了不重启 | `Supervisor` 监督树：自动重启（3/60s 上限）、优雅退出 | Task 5 |
| **Work Graph** | `background.rs` 已自动连推 3 里程碑；TUI turn 后 `drive_workgraph` | daemon 周期线程 24/7 推进（复用 `advance_one_milestone`） | Task 7 |
| **Inference Tree** | 单 session 因果分析 | 跨 session 检索 `sessions/*.json` 的 meta 节点 | Task 8 |
| **Session** | `AgentLoop` 自管理落盘 | daemon 管 N 个内存 session + 磁盘 `SessionManager` 视图 | Task 6 |
| **Registry** | 每次扫（非字段） | daemon 级共享实例 | Task 4 |
| **Memory** | 文件级 KV（已跨 session） | 标注其天然共享性 | Task 8 |
| **BG Agent** | `CODECODER_BG_TASK` env one-shot | daemon 内置周期推进，一等公民 | Task 7 |
