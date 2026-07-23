# Client-Server Architecture

## Context

The original CodeCoder architecture ([[0016-channel-topology-and-event-model]]) was a **monolithic TUI process** where the terminal UI (`ratatui`) ran directly in the agent thread, sharing memory and blocking on the same event loop. This tight coupling had several drawbacks:

1. **No multi-client support**: Only one terminal could interact with the agent at a time.
2. **TUI dependency**: The entire codebase depended on `ratatui`/`crossterm`, even for headless usage.
3. **Session isolation**: Each TUI instance had its own agent process; no cross-session state sharing.
4. **Blocking UI**: The agent loop was hard-wired to the TUI render loop, making programmatic access difficult.

Background Agent ([[0026-background-agent-headless-runner]]) and sub-agents ([[0019-sub-agent-capability-boundary]]) already demonstrated that the agent core could run headlessly. The **daemon + client** pattern was the natural next step.

## Decision

We adopted a **client-server architecture** with a long-running `ccd` daemon and a stateless `cc` CLI client:

### Core components

- **`ccd` daemon**: A long-running process that manages the `AgentLoop`, `Registry`, sessions, and capabilities. It listens on a Unix socket at `$CODECODER_ROOT/.ccd.sock` (i.e. `Config.root.join(".ccd.sock")`, see `src/daemon/socket.rs::default_sock_path`; `src/client/mod.rs` matches).
- **`cc` client**: A lightweight CLI that connects to the daemon via stdin/stdout. It has no state — all session memory lives in the daemon.
- **Wire protocol**: JSON messages over the Unix socket for commands, events, and the 5 interactive prompts (permission/ask/confirm/plan/trust).

### Permission/ask round-trip (Task 9a)

The daemon supports **interactive prompts over the wire**:

1. Daemon sends `ServerEvent::PermissionRequest { id, key }` (or ask/confirm/plan/trust variants).
2. Client displays inline `[y/n]` prompt, reads user input from stdin.
3. Client sends response back as JSON message.
4. Daemon resumes the tool execution.

This means **all 5 dialog types work seamlessly** in the client-server architecture, with `cc` rendering them inline in the terminal (no full-screen TUI needed).

### Architecture diagram

```
Unix socket (JSON wire protocol)
  ┌──────────────┐ ───────────────────────────────▶ ┌──────────────┐
  │  cc 客户端    │                                    │  ccd daemon  │
  │  stdin/stdout │ ◀─────────────────────────────── │  AgentLoop   │
  └──────────────┘  event_rx (AgentEvent: 流式增量/    └──────────────┘
      行内 y/n         工具状态/权限请求/ask/通知)              │
   (permission/ask)  经 AgentEvent 内嵌 reply_tx oneshot ◀────┘
```

### Entry points (main.rs dispatch)

- `CODECODER_BG_TASK=<task>` → `run_background()` (headless one-shot, unchanged)
- Otherwise → `run_daemon()` (client-server mode)

The `cc` binary is invoked separately to connect to the running daemon.

## Consequences

### Positive

1. **Multi-client support**: Multiple terminals can connect to the same daemon simultaneously (M2 milestone).
2. **State sharing**: Sessions, Registry, and capabilities are shared across all clients (M2).
3. **No TUI dependency**: `ratatui`/`crossterm` completely removed; `cargo tree` is clean (M4).
4. **Programmatic access**: The socket protocol enables future tooling/IDE integrations.
5. **Isolation**: Client crashes don't kill the agent; daemon survives client disconnection.
6. **Simplified testing**: No PTY/TUI timing issues; integration tests can speak JSON directly.

### Negative

1. **Startup overhead**: Users must start `ccd` before `cc` (managed by shell aliases or systemd).
2. **Socket dependency**: Requires Unix socket support (no Windows support without named pipes).
3. **Migration pain**: Existing users must adapt to the two-process model.
4. **L4 verify events not yet on the wire**: `AgentEvent::TestSuiteLoaded`, `TestProgress`, `TestSuiteComplete`, `L4ScenarioProgress`, and `L4ExploreProgress` are dropped by `drain_agent_events` — they only fire in L4 verify scenarios and are not surfaced to `cc` clients. `SubAgentMilestone` and `Reasoning` are translated to `ServerEvent::Notice` (prefixed `↳ ` / `💭 `) rather than getting dedicated protocol variants, so styling fidelity is lost.

### Neutral

1. **Performance**: Negligible overhead; local socket is fast, and the agent remains the bottleneck.
2. **Security**: Socket access is filesystem-permissioned; no new attack surface vs. TUI.
3. **Compatibility**: All existing tools work unchanged; only the UI layer changed.

## Implementation status

- **Milestone M1** (Task 2): Daemon skeleton + `cc` client basic connectivity
- **Milestone M2** (Tasks 4-6): Multi-client support, shared Registry, session management
- **Milestone M3** (Task 7): Work Graph auto-advancement in daemon background thread
- **Milestone M4** (Task 9, this document): **TUI removal complete** — `ratatui`/`crossterm` deleted, `cargo tree` clean

## Related ADRs

- [[0016-channel-topology-and-event-model]]: Original channel topology (TUI → agent)
- [[0019-sub-agent-capability-boundary]]: Sub-agent read-only limits (reused for client isolation)
- [[0020-skills-and-capabilities-registry]]: Registry shared across clients
- [[0026-background-agent-headless-runner]]: Headless runner pattern (precedent for daemon mode)

## 修订(2026-07-22):daemon 生命周期——SIGINT/SIGTERM 优雅退出

- `cc shutdown` 命令 → 设 `shutdown` flag → 监控线程(100ms 轮询)自连接 socket → accept 退出阻塞 → `shutdown_all()` 杀常驻 Capability → 清理 socket 文件 → `exit(0)`。
- SIGINT/Ctrl+C → `signal_hook::flag::register` 设 shutdown flag。监控线程同上路径。
- SIGTERM(`kill <pid>`) → 同 SIGINT,优雅退出(此前 daemon 无 SIGINT handler,SIGINT 被已有 turn 的 CancelToken handler 吞掉;SIGTERM 默认硬杀)。
- daemon 子进程(Persistent Capability)不被信号级联(由 `shutdown_all()` 在退出时 kill)。
- `SocketServer` 的 `Drop` 在 `exit(0)` 时不执行 → 退出前手动 `remove_file(socket_path)`。
- `handle_connection` 的 `Shutdown` 分支亦自连接 socket(双保险)。
