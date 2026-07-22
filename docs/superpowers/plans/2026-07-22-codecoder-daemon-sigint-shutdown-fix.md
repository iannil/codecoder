# daemon SIGINT 响应 + cc shutdown 修复 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** daemon 响应 SIGINT/SIGTERM 优雅退出,`cc shutdown` 不再挂起(≤50ms 察觉退出)。

**Architecture:** daemon/mod.rs 的 run() 在 accept 循环前注册 SIGINT+SIGTERM→shutdown flag(复用 signal-hook);SocketServer 加 `set_nonblocking`;accept 循环改为 50ms 轮询 + shutdown flag 检查。零新依赖。

**Tech Stack:** Rust + signal-hook(既有依赖)。

## Global Constraints

- **零新依赖**;复用既有 `signal-hook` 与 `Arc<AtomicBool>`。
- **既有测试(daemon_constructs_with_temp_root、reload_loop、workgraph_publisher 等)必须仍绿**——改的是 daemon run 逻辑,测试不调 run()。
- **SIGINT + SIGTERM 都接**。
- **cc shutdown 不变**(socket.rs:173 `shutdown.store(true)` 已对)。
- **TDD** + conventional commits 中文 + 分支 `fix/daemon-sigint-shutdown`。

## File Structure

- Modify: `src/daemon/socket.rs`(加 `set_nonblocking` + 新的 `accept_one` 文档注释)。
- Modify: `src/daemon/mod.rs`(accept 循环改非阻塞轮询 + 信号注册)。
- Modify: `docs/adr/0032-*.md`(修订,补 daemon 生命周期节)。
- Modify: `ARCHITECTURE.md`(取消节补 SIGINT/SIGTERM 优雅退出)。

---

## Task 1: SocketServer 加 `set_nonblocking` + 既有测试回归

**Files:**
- Modify: `src/daemon/socket.rs`

- [ ] **Step 1: 加 set_nonblocking 方法**

在 `socket.rs` `SocketServer` impl(`pub fn sock_path` 后,约 :70)加:
```rust
    /// 把底层 listener 设为非阻塞模式(daemon 的 accept 轮询需要)。
    pub fn set_nonblocking(&self, nonblocking: bool) -> anyhow::Result<()> {
        self.listener.set_nonblocking(nonblocking)?;
        Ok(())
    }
```
把 `accept_one` 的文档注释"阻塞接受一个连接"改为"接受一个连接(阻塞或非阻塞取决于 set_nonblocking)。"

- [ ] **Step 2: 编译 + 回归测试**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' | wc -l | xargs echo "非0failed行(应0):"`
Expected: 0(accept_one 调用者不改,既有测试仍绿)。

- [ ] **Step 3: commit**

```bash
git add src/daemon/socket.rs
git commit -m "feat(socket): SocketServer 加 set_nonblocking 方法

daemon 的 accept 循环需要非阻塞模式以轮询 shutdown flag;
set_nonblocking 委托给 UnixListener::set_nonblocking。"
```

---

## Task 2: daemon run 循环改非阻塞轮询 + 信号注册

**Files:**
- Modify: `src/daemon/mod.rs`
- Modify: `src/daemon/socket.rs`(import)

**Interfaces:**
- Consumes: `SocketServer.set_nonblocking`(Task 1)。

- [ ] **Step 1: 改 daemon/mod.rs run 循环**

在 `src/daemon/mod.rs` 文件开头加 import:
```rust
use std::time::Duration;
use std::io::ErrorKind;
```

在 `run()` 中 `let server = socket::SocketServer::bind(&sock_path)?;` 之后、`let provider = ...` 之前加:
```rust
        server.set_nonblocking(true)?;
```

在 `let shutdown = Arc::new(AtomicBool::new(false));` 之后、`let bus = ...` 之前加:
```rust
        // 注册 SIGINT + SIGTERM → shutdown flag(ADR 0032 修订)。
        // 复用 signal-hook(既有依赖,同 BG cancel_on_sigint),但 handler 设的是
        // daemon 的 shutdown flag(而非 turn CancelToken),使信号触发优雅退出。
        if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown)) {
            eprintln!("ccd: SIGINT handler not registered: {e}");
        }
        if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown)) {
            eprintln!("ccd: SIGTERM handler not registered: {e}");
        }
```

把 accept 循环(约 :117-135):
```rust
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
            let turn_token_c = Arc::clone(&turn_token);
            let bus_c = Arc::clone(&bus);
            std::thread::spawn(move || {
                if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown, &turn_token_c, &bus_c) {
                    eprintln!("ccd: connection error: {e}");
                }
            });
        }
```
改为:
```rust
        // 优雅退出: SIGINT/SIGTERM → shutdown flag → 循环退出 → shutdown_all。
        // cc shutdown 设 shutdown flag 后,非阻塞 accept 轮询在 ≤50ms 内察觉。
        while !shutdown.load(Ordering::SeqCst) {
            let stream = match server.accept_one() {
                Ok(s) => s,
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                    // 非阻塞:无新连接,轮询一次 shutdown flag 后继续。
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => {
                    // accept 出错不致命,记录后继续。
                    eprintln!("ccd: accept error: {e}");
                    continue;
                }
            };
            let mgr = mgr.clone();
            let shutdown = shutdown.clone();
            let turn_token_c = Arc::clone(&turn_token);
            let bus_c = Arc::clone(&bus);
            std::thread::spawn(move || {
                if let Err(e) = socket::handle_connection(stream, &mgr, &shutdown, &turn_token_c, &bus_c) {
                    eprintln!("ccd: connection error: {e}");
                }
            });
        }
```

- [ ] **Step 2: 编译 + 测试**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' | wc -l | xargs echo "非0failed行(应0):"`
Expected: 0(既有测试不调 `run()`,应无回归)。

- [ ] **Step 3: daemon smoke(确认能启动 + shutdown)**

```bash
cd /Users/rong.zhu/Code/codecoder
LAB=/Users/rong.zhu/Code/codecoder-probe
set -a; . .ccd.env; set +a
cargo build 2>&1 | tail -1
# 启动 daemon;wait 2s;cc shutdown;测 ≤2s 退出
CODECODER_ROOT="$LAB" target/debug/codecoder > /tmp/p11_daemon.log 2>&1 & sleep 1
ls "$LAB/.ccd.sock" >/dev/null 2>&1 && echo "daemon up"
CODECODER_ROOT="$LAB" target/debug/cc shutdown 2>&1; RC=$?; echo "cc shutdown exit=$RC"
sleep 2
if ls "$LAB/.ccd.sock" 2>/dev/null; then
  echo "❌ socket still exists (daemon didn't exit)"
  kill -TERM "$(pgrep -f 'target/debug/codecoder' | head -1)" 2>/dev/null
else
  echo "✓ socket gone (daemon exited cleanly)"
fi
pgrep -fl 'target/debug/codecoder' && echo "❌ residual" || echo "✓ no residual"
```
Expected: daemon 启动 → `cc shutdown` → socket 清理(≤2s) → 无残留进程(对比 P11 修复前:cc shutdown 不退出)。

- [ ] **Step 4: SIGINT + SIGTERM smoke**

```bash
CODECODER_ROOT="$LAB" target/debug/codecoder > /tmp/p11_sig.log 2>&1 & D=$!; sleep 1
kill -INT "$D"; sleep 2
if ls "$LAB/.ccd.sock" 2>/dev/null; then echo "❌ SIGINT didn't stop daemon"; kill -TERM "$D"; else echo "✓ SIGINT → clean exit"; fi
# SIGTERM
CODECODER_ROOT="$LAB" target/debug/codecoder > /tmp/p11_sigterm.log 2>&1 & D=$!; sleep 1
kill -TERM "$D"; sleep 2
ls "$LAB/.ccd.sock" 2>/dev/null && echo "❌ SIGTERM didn't stop" && kill -KILL "$D" || echo "✓ SIGTERM → clean exit"
```
Expected: SIGINT + SIGTERM 均优雅退出(socket 清理、无残留)。

- [ ] **Step 5: commit**

```bash
git add src/daemon/mod.rs
git commit -m "feat(daemon): SIGINT/SIGTERM 优雅退出 + cc shutdown 不再挂起

daemon run 循环:accept 前注册 SIGINT+SIGTERM→shutdown flag(signal-hook,
既有依赖);accept 改为非阻塞(set_nonblocking)+50ms 轮询 shutdown flag。
cc shutdown 设 flag 后 ≤50ms 察觉退出(修复前阻塞 accept 永不读 flag)。
SIGTERM 接后 kill <pid> 也优雅退出。"
```

---

## Task 3: ADR 0032 修订 + ARCHITECTURE 同步

**Files:**
- Modify: `docs/adr/0032-*.md`、`ARCHITECTURE.md`

- [ ] **Step 1: ADR 0032 补 daemon 生命周期节**

在 `docs/adr/0032-*.md` 末尾加:
```markdown
## 修订(2026-07-22):daemon 生命周期——SIGINT/SIGTERM 优雅退出

- `cc shutdown` 命令 → 设 `shutdown` flag → 非阻塞 accept 轮询(50ms)察觉 → accept 循环退出 → `shutdown_all()` 杀常驻 Capability → 线程 join → 主进程退出。
- SIGINT/Ctrl+C → `signal_hook::flag::register` 设 shutdown flag,同 cc shutdown 路径。
- SIGTERM(`kill <pid>)` → 同 SIGINT,优雅退出(此前 daemon 无 SIGINT handler,SIGINT 被已有 turn 的 CancelToken 吞掉;SIGTERM 是默认硬杀)。
- daemon 子进程(Persistent Capability)不被信号级联(由 `shutdown_all()` 在退出时 kill)。
```

- [ ] **Step 2: ARCHITECTURE 取消节补一句**

`ARCHITECTURE.md` 取消节(既有 `- **取消**是协作式:...` 行后)加:
```
- **daemon 整体** SIGINT/SIGTERM → shutdown flag → 50ms 轮询 exit → `shutdown_all` 杀常驻 Capability(ADR 0032 修订)。
```

- [ ] **Step 3: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' | wc -l | xargs echo "非0failed行(应0):"`
Expected: 0。
```bash
git add docs/adr/0032-client-server-architecture.md ARCHITECTURE.md
git commit -m "docs: ADR 0032 修订 daemon 生命周期 SIGINT/SIGTERM 优雅退出"
```

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- socket.rs set_nonblocking(spec §4.1)→ Task 1 ✓
- daemon/mod.rs 信号注册 + 非阻塞 accept 轮询(spec §4.2)→ Task 2 ✓
- SIGINT+SIGTERM 都接(spec §2)→ Task 2 注册两个 signal ✓
- cc shutdown 不变(spec §3)→ socket.rs 的 ClientRequest::Shutdown 未改 ✓
- 既有测试回归(spec §5)→ Task 1/2 cargo test 0-failed ✓
- live 复验(spec §5)→ Task 2 Step 3/4 smoke ✓
- ADR 0032 修订 + ARCHITECTURE(spec §6)→ Task 3 ✓
- 零新依赖(spec §2)→ signal-hook 已是既有依赖 ✓

**2. Placeholder scan:** 无 TBD/TODO;daemon/mod.rs 给出完整改前/改后代码;smoke 命令完整可运行 ✓

**3. Type consistency:** `SocketServer::set_nonblocking(bool) -> anyhow::Result<()>`(Task 1)与 `server.set_nonblocking(true)?`(Task 2)一致;`ErrorKind::WouldBlock`(Task 2)与 `std::io::ErrorKind` 一致;`signal_hook::consts::SIGINT`/`SIGTERM`(Task 2)与既有 `signal_hook::flag::register` 签名一致 ✓