# CodeCoder 进程管理全面审计：使用者控制面

- **日期**: 2026-07-25
- **审计范围**: 从使用者视角审视 codecoder 所有能力/模式/进程的可管理性——发现、启动、监控、控制、停止、恢复
- **审计方法**: 源码审计（`src/` 全部关键文件）+ ADR 复核 + 测试用例验证
- **状态图例**: ✅ 已支持 · ⚠️ 有缺口 · ❌ 不支持 · 🔶 间接支持（有操作方式但不直接）

---

## 1. 审计框架

每个管理项按以下 6 个维度评估：

| 维度 | 代码问题 | 使用者问题 |
|------|----------|------------|
| **发现** | 该实体的存在/状态如何暴露给用户？ | 用户怎么知道这个东西存在？ |
| **启动** | 如何创建/spawn？环境变量/API/CLI？ | 用户怎么让它跑起来？ |
| **监控** | 运行时状态如何可观测？ | 用户怎么看它现在怎么样了？ |
| **控制** | 用户能改变其行为吗？参数/配置/工具？ | 用户怎么干预它？ |
| **停止** | 优雅终止路径存在吗？信号/tool/API？ | 用户怎么让它停下来？ |
| **恢复** | 崩溃/异常后如何自愈或手动修复？ | 出问题了用户怎么修？ |

---

## 2. 运行模式切换（main.rs → 三种入口）

**代码**: `src/main.rs`（3 行 shim）、`src/lib.rs:54-75`（`bg_mode_from_env`）

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ❌ | 无 `--help`、无 CLI 参数解析。用户只能读 `README.md` 或源码才知道有 3 种入口模式 |
| **启动** | ✅ | `CODECODER_BG_TASK=<task>`（显式任务）、`CODECODER_BG_WORKGRAPH=1`（workgraph 模式）、默认 daemon。三种入口由 env 路由，无重叠 |
| **监控** | ❌ | 无启动时打印当前模式。用户无从得知当前进程是 daemon 还是 background 除非检查 env |
| **控制** | ❌ | 无运行时切换模式的能力。需重启进程、改 env |
| **停止** | 🔶 | daemon 可 SIGINT/SIGTERM 或 `cc shutdown`；background 单次执行自动退出 |
| **恢复** | ❌ | daemon crash → 用户需手动重启。无 systemd/launchd 集成（ADR 0026 标注"调度外置"） |

**缺口 1 (P1)**: 缺少 CLI 参数入口。`main.rs` 完全没有 arg 解析——`--help`、`--version`、`--daemon`、`--bg-task` 等都不存在。用户被迫通过环境变量控制，且无验证（设了冲突的 env 也不会报错，优先级靠硬编码）。

---

## 3. daemon 进程管理

**代码**: `src/daemon/mod.rs`（daemon 主循环）、`src/daemon/socket.rs`（socket server + connection handler）、`src/daemon/proto.rs`（线协议）、`src/daemon/session_manager.rs`（session 托管）、`src/daemon/bus.rs`（事件总线）

### 3.1 daemon 生命周期

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | socket 文件 `.ccd.sock` 存在即表示 daemon 在运行。`cc` 客户端连接失败会报错 |
| **启动** | ✅ | `CODECODER_DAEMON=1 cargo run` 或默认入口启动。bind socket、spawn 5 个后台线程 |
| **监控** | ⚠️ | 启动时打印有限；`BusNotice` 事件可被 `cc` 客户端收到（workgraph/supervisor 事件），但无 `cc status` 命令查看 daemon 健康状态 |
| **控制** | ❌ | 无 `cc status`、无 `cc services` 命令。daemon 运行中完全黑盒 |
| **停止** | ✅ | SIGINT/SIGTERM → shutdown flag → 自连接 socket 唤醒 accept → 循环退出 → `shutdown_all()` → `remove_file(sock)` → `exit(0)` |
| **恢复** | ❌ | daemon 进程本身无 watchdog。crash 后 socket 残留需手动清理 |

### 3.2 daemon 后台线程（5 个）

| 线程 | 功能 | 周期 | 可管理性 |
|------|------|------|----------|
| **监控线程** | 轮询 shutdown flag，被设后自连接 socket 唤醒 accept | 100ms | ❌ 不可见、不可控 |
| **Supervisor 线程** | 每 1s 检查 Persistent 服务存活，标记崩溃 | 1s | ⚠️ 事件通过 bus 广播，但无 `cc services` 查看 |
| **Workgraph 推进线程** | 每 30s 尝试推进一个就绪里程碑 | 30s | ❌ 不可配置间隔、不可暂停 |
| **Registry 热重载线程** | 每 3s 重扫 skills/capabilities/prompts | 3s | ❌ 不可配置间隔 |
| **Connection 线程** | 每个客户端连接一个独立线程处理 | per-conn | ✅ 自动创建和清理（`ConnGuard` 保证清理） |

**缺口 2 (P2)**: 后台线程完全不可观测。用户无法知道 workgraph 推进线程是否在工作、Supervisor 是否发现了服务崩溃、热重载是否生效。所有线程的间隔硬编码，不可配置。

### 3.3 socket 通信

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | socket 路径 `$CODECODER_ROOT/.ccd.sock`，权限 `0o600` |
| **启动** | ✅ | daemon 启动时自动 bind |
| **监控** | ✅ | 连接成功即表示 daemon 在运行；`BusNotice` 可接收广播事件 |
| **控制** | ⚠️ | 支持的 `ClientRequest` 有限：SendMessage / NewSession / ListSessions / Resume / Shutdown / Status(存在但实现存疑) / TreeShow / TreeNav / TreeClone / PromptReply |
| **停止** | ✅ | `cc shutdown` 发 Shutdown 请求 |
| **恢复** | ❌ | socket 文件残留需手动清理（Drop 会清理，但 exit(0) 前可能来不及） |

**缺口 3 (P2)**: `ClientRequest::Status` 存在(`src/daemon/proto.rs:81`)但 socket 层处理存疑（需验证是否真正实现）。缺少关键命令如 `cc services`、`cc workgraph`、`cc threads`。

---

## 4. cc 客户端管理

**代码**: `src/client/mod.rs`（socket 连接、事件渲染、交互式提示）、`src/bin/`（二进制入口）

### 4.1 客户端命令一览

| 命令 | 状态 | 备注 |
|------|------|------|
| `cc <message>` | ✅ | 发送消息给 agent |
| `cc shutdown` | ✅ | 优雅关闭 daemon |
| `cc new` | ✅ | 新建 session |
| `cc sessions` | ✅ | 列出所有 session |
| `cc resume <id>` | ✅ | 恢复 session |
| `cc tree` | ✅ | 显示 session 会话树 |
| `cc fork <id>` | ✅ | 导航到 session 树节点 |
| `cc clone` | ✅ | 复制当前 session |
| `cc status` | ⚠️ | 声明在 proto 中但客户端实现存疑 |
| `cc services` | ❌ | 不存在 |
| `cc workgraph` | ❌ | 不存在 |
| `cc help` | ❌ | 不存在 |
| `cc version` | ❌ | 不存在 |

### 4.2 交互式提示处理

| 提示类型 | 状态 | 处理方式 |
|----------|------|----------|
| Permission 授权 | ✅ | `y/n/s/p` 行内选择（Once/Session/Project/Deny） |
| AskUser 提问 | ✅ | 输入文本回复 |
| Confirm 确认 | ✅ | `y/n` |
| PlanApproval 计划审批 | ✅ | `y/n` |
| TrustPrompt 项目信任 | ✅ | `a/o/n`（Always/Once/Never） |

**缺口 4 (P2)**: 无 `cc help`，新用户只能通过 `README.md` 了解可用命令。无 `cc version`，无法确认 daemon 版本兼容性。无 `cc services` 和 `cc workgraph` 等管理命令。

---

## 5. 子进程管理（run_command）

**代码**: `src/tool/builtin.rs:56-180`（`RunCommand` 工具 + `run_shell_cancellable` 辅助函数）

### 5.1 子进程 spawn / kill / 超时

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | 子进程由 agent 的 tool 调用发起，事件流有 `ToolStarted` / `ToolFinished` |
| **启动** | ✅ | `run_command { cmd: "..." }` 工具调用。`Command::new("sh").arg("-c")` spawn |
| **监控** | ⚠️ | 输出实时流式传输（`StreamDelta`），但无子进程 PID 暴露、无执行时间报告 |
| **控制** | ⚠️ | 复合命令 vs 简单命令的 permission keying 有区分；输出截断（`CODECODER_MAX_TOOL_OUTPUT`）；但**无 timeout 参数**、**无手动 kill 机制** |
| **停止** | ✅ | Ctrl+C → `CancelToken` → `child.kill()` + `child.wait()`。pipe 防死锁（独立线程 drain stdout/stderr，poll 20ms 间隔） |
| **恢复** | ❌ | 子进程僵死：poll 循环 20ms 间隔 + cancel token 是唯一终止方式。无 timeout 兜底 |

`run_shell_cancellable` 关键机制：
```rust
// 独立线程 drain stdout/stderr，避免 pipe buffer 满死锁
let out_reader = std::thread::spawn(move || { ... });
let err_reader = std::thread::spawn(move || { ... });
// 20ms poll 循环 + cancel 检查
loop {
    if ctx.is_cancelled() { child.kill(); child.wait(); break None; }
    match child.try_wait()? {
        Some(status) => break Some(status),
        None => std::thread::sleep(Duration::from_millis(20)),
    }
}
```

**缺口 5 (P3)**: 子进程无 timeout。如果一个命令永远不退出（如 `tail -f`），唯一终止方式是 Ctrl+C 取消整个 turn。工具参数中无 `timeout_secs` 字段，无法设置单个命令的超时。

### 5.2 权限控制

| 特性 | 状态 | 详情 |
|------|------|------|
| 简单命令 keying | ✅ | `run_command:git`（按命令类） |
| 复合命令 keying | ✅ | `run_command:cd X && rm`（整条命令串，ADR 0036 加固） |
| 预授权 allowlist | ✅ | `codecoder.json` 持久化，session allowlist 内存暂存 |
| headless 自动拒绝 | ✅ | 未预授权 → 自动拒绝（`ToolFinished { is_error: true }`） |

---

## 6. Capability 生命周期管理

**代码**: `src/capability.rs`（Supervisor、RunningServiceTable、Environment/Lifecycle）、`src/supervisor_state.rs`（跨重启持久化）、`src/tool/builtin.rs:352-570`（run_capability 工具）

### 6.1 OneShot Capability

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | 通过 `registry catalog` 可见（agent 可 `list_directory capabilities/`） |
| **启动** | ✅ | `run_capability` 工具调用，spawn 子进程、等退出、返回输出 |
| **监控** | ✅ | 工具返回 stdout/stderr 合并输出 |
| **控制** | ❌ | 无参数传递能力（仅 `CODECODER_CAPABILITY_ARGS` JSON 环境变量） |
| **停止** | ✅ | 自动退出；cancel token 可 kill |
| **恢复** | ❌ | 单次执行，无重试语义 |

### 6.2 OnDemand Capability

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | 同 OneShot |
| **启动** | ✅ | `run_capability` 工具调用，spawn 子进程 |
| **监控** | ❌ | 无"服务是否在运行"的查询 |
| **控制** | ❌ | 生命周期由代码自动管理（同 turn 复用，turn 结束自动 kill） |
| **停止** | ✅ | turn 结束自动 reaper 线程（5 秒后 kill） |
| **恢复** | ❌ | 同 turn 内可复用，跨 turn 重新 spawn |

**缺口 6 (P3)**: OnDemand 的 reaper 线程 5 秒硬编码（`src/tool/builtin.rs:561`），不可配置。

### 6.3 Persistent Capability（Supervisor 监督）

**代码**: `src/capability.rs:113-213`（Supervisor 结构）、`src/supervisor_state.rs`（持久化状态）

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ⚠️ | daemon 启动时扫描 `capabilities/` 加载。运行时事件通过 bus 广播（`capability 'xxx' exited; marked Failed`），但用户无主动查询手段 |
| **启动** | ✅ | daemon 启动自动 `start_all`；daemon 运行中可通过 `run_capability` 的 persistent 分支单独启动 |
| **监控** | ❌ | Supervisor 线程每 1s 检查子进程存活，事件发 bus。但无 `cc services` 查看当前运行状态 |
| **控制** | ❌ | 无手动 restart / stop / reset gave_up 命令。唯一重置方式：修改 manifest 文件（触发 mtime 检测自动重置） |
| **停止** | ✅ | daemon 退出时 `shutdown_all()` kill 所有子进程 |
| **恢复** | ⚠️ | 会话内：崩溃 1 次即 `gave_up`，不自动重启（ADR 0021 有意设计）。跨重启：检查 `supervisor_state.json`，超崩溃预算跳过，manifest 变更自动重置 |

Supervisor 状态持久化：

```rust
// supervisor_state.json 结构（src/supervisor_state.rs）
pub struct ServiceEntry {
    pub gave_up: bool,
    pub crash_count: u32,
    pub manifest_mtime_secs: u64,
}
```

**缺口 7 (P1)**: **无 `cc services` 命令**——用户无法查看正在运行的服务、无法手动重启 gave_up 的服务、无法重置崩溃计数。这是最影响日常使用的管理缺口之一。

**缺口 8 (P3)**: Supervisor 监督线程周期 1s 硬编码，不可配置。

### 6.4 Wasm Capability

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | catalog 可见 |
| **启动** | ✅ | `run_capability` → Wasm 分支，wasmtime 执行 |
| **监控** | ❌ | 无 wasm 运行时状态 |
| **控制** | ❌ | OnDemand 模式下 Wasm 等同于 OneShot（无复用） |
| **停止** | ✅ | 自动退出 |
| **恢复** | ❌ | 单次执行 |

---

## 7. Workgraph 编排管理

**代码**: `src/workgraph.rs`（WorkGraph 数据结构 + read/save/lock）、`src/background.rs`（advance/retry 循环）、`src/bg_gate.rs`（客观验收门）、`src/agent.rs:498-500`（交互式 `drive_workgraph`）

### 7.1 里程碑生命周期

| 维度 | 评估 | 详情 |
|------|------|------|
| **发现** | ✅ | agent 可通过 `milestone` 工具查看 workgraph |
| **启动** | ✅ | `milestone { action: "add" }` 创建里程碑；`drive_workgraph` / daemon 空闲线程自动推进 |
| **监控** | ⚠️ | 交互式：agent 通过 `milestone` 工具报告状态。daemon 模式：bus 广播 milestone 事件。headless 模式：NDJSON 文件 + stderr 输出。但无 `cc workgraph` 直接查看 |
| **控制** | ⚠️ | 交互式：`milestone` 工具可 add/set_status。headless 自恢复有 `CODECODER_BG_MAX_FIX_ATTEMPTS` 控制。但 daemon 推进线程**不可暂停/不可配置间隔** |
| **停止** | ❌ | 无"暂停 workgraph 推进"的命令。daemon 启动后 30s 固定间隔自动推进 |
| **恢复** | ⚠️ | headless 自恢复（`retry_one_milestone`）。交互式/daemon 空闲推进**只标记 needs_fix，需人工重置 pending** |

**缺口 9 (P2)**: daemon workgraph 推进线程 30s 间隔硬编码（`src/daemon/mod.rs:103`），不可配置。且无暂停/恢复机制——用户可能不希望 daemon 在忙碌时自动推进 milestone。

**缺口 10 (P3)**: 交互式模式下 milestone 验收失败后，用户只能通过 agent 对话手动重置 `pending`，无快捷命令。

### 7.2 headless workgraph 自恢复

| 特性 | 状态 | 配置项 |
|------|------|--------|
| needs_fix 重试 | ✅ | `CODECODER_BG_MAX_FIX_ATTEMPTS`（默认 3，0=禁用） |
| 重试预算 | ✅ | 每里程碑独立 `fix_attempts` 计数，持久化到 `workgraph.json` |
| 失败原因注入 | ✅ | `last_failure` 注入修复 prompt |
| 熔断 | ✅ | `CODECODER_BG_CIRCUIT_K`（默认 2），重试路径不计 |
| 推进上限 | ✅ | `CODECODER_BG_MAX_AUTO`（默认 10） |
| 工具迭代上限 | ✅ | `CODECODER_BG_MILESTONE_TOOL_CAP`（默认 8） |

---

## 8. 配置管理

**代码**: `src/config.rs`

### 8.1 环境变量一览

| 变量 | 默认值 | 管理域 | 可管理性 |
|------|--------|--------|----------|
| `CODECODER_API_KEY` | — | 连通性 | ✅ 标准 API key |
| `CODECODER_MODEL` | `gpt-4o` | 连通性 | ✅ 运行前设 |
| `CODECODER_API_BASE` | OpenAI | 连通性 | ✅ |
| `CODECODER_MAX_TOKENS` | 8192 | LLM | ✅ |
| `CODECODER_MAX_TOKENS_CEILING` | 32768 | LLM | ✅ |
| `CODECODER_TEMPERATURE` | 0.7 | LLM | ✅ |
| `CODECODER_ROOT` | CWD | 路径 | ✅ |
| `CODECODER_DAEMON` | — | 模式 | ✅ |
| `CODECODER_BG_TASK` | — | 模式 | ✅ |
| `CODECODER_BG_WORKGRAPH` | — | 模式 | ✅ |
| `CODECODER_BG_MAX_AUTO` | 10 | Workgraph | ✅ |
| `CODECODER_BG_CIRCUIT_K` | 2 | Workgraph | ✅ |
| `CODECODER_BG_MILESTONE_TOOL_CAP` | 8 | Workgraph | ✅ |
| `CODECODER_BG_MAX_FIX_ATTEMPTS` | 3 | Workgraph | ✅ |
| `CODECODER_SUPERVISOR_CRASH_BUDGET` | 3 | Capability | ✅ |
| `CODECODER_MAX_TOOL_OUTPUT` | 256KB | 子进程 | ✅ |
| `CODECODER_NOOP_NUDGE_THRESHOLD` | 3 | Agent | ✅ |
| `CODECODER_MAX_TOKENS_CEILING` | 32768 | LLM | ✅ |
| `GITHUB_TOKEN` | — | 搜索 | ✅ |

**缺口 11 (P3)**: 所有配置仅环境变量方式，无配置文件（如 `codecoder.toml` / `codecoder.json`）。环境变量不适合表达嵌套结构（如 per-skill 权限、per-service 配置）。

**缺口 12 (P3)**: 无 `.ccd.env` 自动加载的文档化行为（源码 `src/config.rs:165` 有 `autoload_ccd_env` 函数但无用户文档告知此机制）。

---

## 9. 综合缺口汇总（按严重度排序）

### P1（关键——日常使用障碍）

| # | 缺口 | 域 | 说明 |
|---|------|-----|------|
| 1 | 无 `cc services` 命令 | Capability | 无法查看/管理正在运行的 Persistent 服务 |
| 2 | 无 CLI `--help` / `--version` | 入口 | main.rs 无 arg 解析 |

### P2（重要——可观测性/控制性不足）

| # | 缺口 | 域 | 说明 |
|---|------|-----|------|
| 3 | daemon 后台线程不可观测 | daemon | 5 个线程均无状态暴露 |
| 4 | 无 `cc status` 完整实现 | daemon | proto 声明了 Status 但客户端实现存疑 |
| 5 | 无 `cc workgraph` 命令 | Workgraph | 无法直接查看/控制 workgraph 状态 |
| 6 | workgraph 推进间隔不可配置 | Workgraph | daemon 线程 30s 硬编码 |
| 7 | 无 `cc help` | 客户端 | 新用户无从发现命令 |
| 8 | daemon workgraph 推进不可暂停 | Workgraph | 无暂停/恢复机制 |

### P3（改进——边界情况/体验优化）

| # | 缺口 | 域 | 说明 |
|---|------|-----|------|
| 9 | run_command 无 timeout | 子进程 | 命令永不退出时只能 Ctrl+C 全取消 |
| 10 | OnDemand reaper 5s 硬编码 | Capability | 不可配置 |
| 11 | Supervisor supervise 周期 1s 硬编码 | Capability | 不可配置 |
| 12 | 无配置文件格式 | 配置 | 全 env，无 `codecoder.toml` |
| 13 | `.ccd.env` 自动加载未文档化 | 配置 | 用户不知道有这个机制 |
| 14 | 交互式 milestone 无快捷重置命令 | Workgraph | needs_fix 后需对话重置 |
| 15 | daemon crash 后 socket 残留 | daemon | 重启需手动清理 |

---

## 10. 管理能力总矩阵

```
                    发现     启动     监控     控制     停止     恢复
                    ──────────────────────────────────────────────
运行模式切换         ❌      ✅       ❌      ❌      🔶       ❌
daemon 生命周期       ✅      ✅      ⚠️      ❌      ✅       ❌
daemon 后台线程       ❌      ✅       ❌      ❌      ✅       ❌
socket 通信          ✅      ✅      ✅      ⚠️      ✅       ❌
cc 客户端命令        ⚠️      ✅      ⚠️      ⚠️      ✅       ❌
子进程(run_command)  ✅      ✅      ⚠️      ⚠️      ✅       ❌
OneShot Capability   ✅      ✅      ✅      ❌      ✅       ❌
OnDemand Capability  ✅      ✅      ❌      ❌      ✅       ❌
Persistent Cap.      ⚠️      ✅      ❌      ❌      ✅       ⚠️
Wasm Capability      ✅      ✅      ❌      ❌      ✅       ❌
Workgraph 里程碑     ✅      ✅      ⚠️      ⚠️      ❌       ⚠️
Workgraph 自恢复     N/A     N/A     ✅      ✅      N/A      ✅
配置管理             ⚠️      ✅      ❌      ✅      N/A      N/A
```

**关键发现**: 管理能力在"启动"和"停止"维度覆盖较好，但在"监控"和"控制"维度严重不足。"恢复"维度几乎是盲区——唯一有自愈能力的是 headless workgraph 自恢复和 Persistent 跨重启韧性。

---

## 11. 修复建议优先级

### 第一批（P1 修复）

1. **`cc services` 命令** — 列出运行中 Persistent 服务、状态（Running/Failed/gave_up）、崩溃次数、地址
   - 涉及：`src/daemon/proto.rs`（新 ClientRequest）、`src/daemon/socket.rs`（路由）、`src/client/mod.rs`（渲染）
   - 需要：Supervisor 状态暴露（目前 states 是私有的）

2. **`--help` CLI 入口** — 使用 `clap` 或手写 arg 解析替代纯 env 路由
   - 涉及：`src/main.rs`、`src/lib.rs`
   - 最少改动：在 main.rs 中检查 `--help` / `-h` / `--version` 并打印可用模式

### 第二批（P2 修复）

3. **`cc status` 完整实现** — daemon 运行时间、活跃 session 数、后台线程状态、Supervisor 摘要
4. **`cc workgraph` 命令** — 查看 workgraph 状态（里程碑总数、各状态计数、最后推进时间）
5. **daemon 线程可观测** — 每个后台线程暴露心跳/状态到 bus，`cc status` 可读
6. **workgraph 推进间隔可配置** — 新增 `CODECODER_WG_TICK_SECS` 环境变量

### 第三批（P3 修复）

7. **run_command timeout 参数** — `run_command { cmd: "...", timeout_secs: 30 }`
8. **OnDemand reaper 延迟可配置** — 新增 `CODECODER_ONDEMAND_REAPER_SECS`
9. **Supervisor 监督间隔可配置** — 新增 `CODECODER_SUPERVISOR_TICK_SECS`
10. **`.ccd.env` 文档化** — 更新 README.md 或新增文档