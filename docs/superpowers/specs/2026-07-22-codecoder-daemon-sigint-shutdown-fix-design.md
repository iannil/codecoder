# daemon SIGINT 响应 + cc shutdown 修复 — 设计文档

- **日期**: 2026-07-22
- **状态**: 待用户审阅(Pending user review)
- **作者**: Claude Code(brainstorming 产物)
- **起因**: `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` P11 发现——daemon 不响应 SIGINT(SIGINT 被既有 `cancel_on_sigint` token handler 吞掉,未触发优雅退出);`cc shutdown` 虽设了 `shutdown flag`(socket.rs:173),但 accept 循环被 `listener.accept()` 阻塞、永不重检查 → 不退出。
- **关联**: `src/daemon/mod.rs`(run 循环)、`src/daemon/socket.rs`(accept_one 阻塞)、`src/agent.rs`(cancel_on_sigint 模式)、`src/bin/cc.rs`(shutdown 命令)。

## 1. 背景与目标

P11 实测:daemon 整体 SIGINT → 5s 不退出、socket 仍占、无 panic;`cc shutdown` 打印 "shutting down" 但**进程不退**(本次 session 多次复现)。根因:

1. `signal_hook::flag::register` 在 BG turn(advance/turn) 的 `cancel_on_sigint` 中被调用,handler 设的是 `CancelToken`(Arc\<AtomicBool\>),**不是** daemon 的 `shutdown flag`(另一个 Arc\<AtomicBool\>)。daemon 从未为自己注册 SIGINT handler。
2. `listener.accept()` 是**阻塞的**(socket.rs:65);`cc shutdown` 设 `shutdown flag` 后,accept 循环在等待新连接,从不检查 flag。所以 shutdown flag 虽被设,但**永不读**。

**目标**: SIGINT 与 SIGTERM → daemon 优雅退出(shutdown flag → accept 循环退出 → `shutdown_all()` 杀常驻 Capability → 线程 join);`cc shutdown` 同样触发优雅退出(≤50ms 察觉)。

## 2. 已锁定决策

| 维度 | 决策 |
|---|---|
| 信号 | **SIGINT + SIGTERM 都接**(`kill <pid>` 默认 SIGTERM;接了则 `kill` 也优雅) |
| accept | **非阻塞 + 轮询**(`listener.set_nonblocking(true)`,WouldBlock→shutdown check→sleep 50ms) |
| 零新依赖 | 复用既有 `signal-hook` 与 `Arc\<AtomicBool\>` |
| ADR | **修订 ADR 0032**(client-server 架构,补 SIGINT/SIGTERM/cc shutdown 行为) |

**为何非阻塞而非 signalfd / 分离 accept 线程**:signalfd 需额外 syscall + 处理,分离线程需 join 逻辑。非阻塞 + 50ms 轮询**最简单**(零新依赖、零新线程),且 shutdown 察觉延迟 ≤50ms,对 daemon 足够。

## 3. 架构

```
daemon/mod.rs run():
  // 1. 既有 setup(supervisor/workgraph/reload 线程)不变
  // 2. 新增:shutdown flag 上注册 SIGINT+SIGTERM handler
  signal_hook::flag::register(SIGINT, Arc::clone(&shutdown));
  signal_hook::flag::register(SIGTERM, Arc::clone(&shutdown));

  // 3. socket 设为非阻塞
  server.set_nonblocking(true);

  // 4. accept 循环改为非阻塞轮询
  while !shutdown.load(SeqCst) {
      match server.accept_one() {
          Ok(stream) => { handle_connection(stream); }
          Err(ref e) if e.kind() == WouldBlock => {
              std::thread::sleep(Duration::from_millis(50));
          }
          Err(e) => { eprintln!("accept error: {e}"); }
      }
  }
  // 5. 退出清理(既有)
  crate::capability::shutdown_all();
```

**cc shutdown 不变**:`shutdown.store(true)`(socket.rs:173)已对;改后非阻塞轮询在 ≤50ms 内察觉 → 退出。

## 4. 实现(daemon/mod.rs + socket.rs)

### 4.1 socket.rs:加 `set_nonblocking`

```rust
impl SocketServer {
    pub fn set_nonblocking(&self, nonblocking: bool) -> anyhow::Result<()> {
        self.listener.set_nonblocking(nonblocking)?;
        Ok(())
    }
    pub fn accept_one(&self) -> anyhow::Result<UnixStream> {
        // 调用方应在非阻塞模式下配合轮询循环(daemon/mod.rs run)。
        let (stream, _) = self.listener.accept()?;
        Ok(stream)
    }
}
```

### 4.2 daemon/mod.rs:改 run 循环

`run()` 中 `server.accept_one()` 前插信号注册 + 设非阻塞;accept 循环改轮询;`socket.rs` 的 import `WouldBlock`。

## 5. 测试

- **单元(daemon/mod.rs `#[cfg(test)]`)**:`set_nonblocking` + 非阻塞 accept 在测试中(已有测试用 `accept_one` 阻塞模式;非阻塞模式下 WouldBlock→sleep→轮询,不会死循环)。
- **live 复验**(codecoder-probe lab):
  - `cc shutdown` → daemon 在 ≤2s 内退出、socket 清理(对比 P11:等待 >5s 不退)。
  - `kill -INT <daemon_pid>` → daemon 优雅退出、`shutdown_all` 杀常驻 Capability。
  - `kill -TERM <daemon_pid>` → 同 SIGINT,优雅退出。

## 6. ADR / 文档

- **修订 ADR 0032**:补 daemon 生命周期节——SIGINT/SIGTERM → 优雅退出;cc shutdown → 设 shutdown flag → 非阻塞 accept 轮询察觉。
- **ARCHITECTURE.md**:取消节补"daemon 整体 SIGINT/SIGTERM → shutdown_all 优雅退出"。

## 7. 不在本范围内(YAGNI)

- **daemon 子进程/SIGTERM 级联**:daemon 杀 Persistent Capability 子进程已在 `shutdown_all()`;不做信号级联(子进程收到 SIGTERM 被 kill 即退)。
- **`cc shutdown` 超时等待**:daemon 优雅退出 ≤50ms(非阻塞轮询) + 线程 join(毫秒级);不做超时回退 SIGKILL(除非线程卡死;v1 不解决)。
- **重写 accept 循环为 signalfd/epoll**:v1 非阻塞+轮询足够简单;极端负载下 50ms 延迟可忍。