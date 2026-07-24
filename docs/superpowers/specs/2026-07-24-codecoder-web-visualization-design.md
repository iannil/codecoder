# CodeCoder Web — 可视化运行状态设计文档

> 以"不碰内核"为铁律，为 CodeCoder 构建独立观察者层 Web Dashboard。

## 概述

CodeCoder Web 是一个独立二进制 `cc-web`，通过 cc 协议连接 daemon（Unix socket），以只读方式观察 agent 运行状态。它不修改 `src/daemon/`、`src/agent.rs`、`src/workgraph.rs` 等任何内核代码，只复用 `src/daemon/proto.rs` 中的已有协议类型做 import。

## 原则

1. **不改内核一行代码** — 所有增强在 `src/bin/cc-web.rs` + `src/visual/` 下实现
2. **只读观察者** — 不向 daemon 发送任何 write 意图的命令，只读已有协议（ListSessions、ExportSession）和文件（workgraph.json）
3. **渐进交付** — 4 个 phase，每个 phase 独立可运行
4. **零构建前端** — 单页 HTML + vanilla JS + D3 CDN，无 npm/webpack

## 架构

```
┌────────────────────────────────────────────────────────────┐
│  daemon (已有，不修改)                                       │
│  ┌──────────────────┐  ┌────────────────────────────────┐  │
│  │ EventBus         │  │ Unix socket 监听                │  │
│  │ (broadcast 机制)  │  │     ↑ cc 协议 (单行 JSON)      │  │
│  └──────────────────┘  └─────┬──────────────────────────┘  │
└──────────────────────────────┼─────────────────────────────┘
                               │
                    ┌──────────┴──────────┐
                    │  cc-web 二进制      │  = 新独立二进制
                    │  (src/bin/cc-web)   │
                    │                     │
                    │  ┌───────────────┐  │
                    │  │ SocketClient  │  │  复用 proto.rs codec
                    │  │ (daemon 连接)  │  │  只 import，不修改
                    │  └───────┬───────┘  │
                    │          │           │
                    │  ┌───────┴───────┐  │
                    │  │ EventRouter   │  │  事件分发 + SSE 管理
                    │  └───────┬───────┘  │
                    │          │           │
                    │  ┌───────┴───────┐  │
                    │  │ FileWatcher   │  │  notify crate 监听
                    │  └───────┬───────┘  │  workgraph.json
                    │          │           │
                    │  ┌───────┴───────┐  │
                    │  │ HTTP Server   │  │  tiny_http, 无 async
                    │  │ :9876         │  │
                    │  └───────────────┘  │
                    └──────────┬──────────┘
                               │ localhost:9876
                               │
                    ┌──────────┴──────────┐
                    │  浏览器 (单页 HTML)  │
                    │  ┌───────────────┐  │
                    │  │ 实时时间线      │  │  Phase 1
                    │  ├───────────────┤  │
                    │  │ Workgraph 图   │  │  Phase 2
                    │  ├───────────────┤  │
                    │  │ Session 回放   │  │  Phase 3
                    │  ├───────────────┤  │
                    │  │ 测试热力图      │  │  Phase 4
                    │  └───────────────┘  │
                    └─────────────────────┘
```

## 组件详述

### SocketClient

位于 `src/visual/socket_client.rs`。连接 daemon Unix socket，收发 cc 协议消息。

**接口**：
- `connect(path: &str) -> Result<Self>` — 连接 daemon socket
- `send(&self, req: ClientRequest)` — 向 daemon 发送请求
- `recv() -> ServerEvent` — 阻塞接收事件（运行在独立线程）
- `set_event_callback(&self, cb: Box<dyn Fn(ServerEvent)>)` — 注册事件回调

**重连状态机**：
```
Connected → 连接断开 → Disconnected → 重试(最多3次,间隔1s) → 退出
                 ↓ 重连成功
             Connected
```

**实现**：使用 `std::os::unix::net::UnixStream`，`read_request`/`write_event` 从 `src/daemon/proto.rs` import。

### EventRouter

位于 `src/visual/event_router.rs`。从 SocketClient 接收事件，分发给所有 SSE 连接。

**数据结构**：
```rust
struct EventRouter {
    sse_clients: Mutex<HashMap<u64, Sender<ServerEvent>>>,
    next_id: u64,
    event_buffer: Mutex<VecDeque<ServerEvent>>, // 最近 200 条
}
```

**规则**：
- 全部 `ServerEvent` 变体都转发给 SSE 连接
- 前端侧的过滤规则：`StreamDelta` 过长时截断（前端处理）
- 连接断开时清理 sender，不阻塞其他连接
- 新连接时发送 buffer 中最近 50 条事件作为 catch-up

### HTTP Server

位于 `src/visual/http_server.rs`。使用 `tiny_http` 暴露 REST + SSE 端点。

**端点清单**：

| 端点 | 方法 | 用途 | Phase |
|------|------|------|-------|
| `/` | GET | 静态文件 index.html | 1 |
| `/api/v1/events` | GET | SSE 实时事件流 | 1 |
| `/api/v1/sessions` | GET | 返回 session 列表 | 3 |
| `/api/v1/sessions/:id` | GET | 返回单个 session 详情 | 3 |
| `/api/v1/sessions/:id/events` | GET | 返回 session 事件序列 | 3 |
| `/api/v1/workgraph` | GET | 返回当前 workgraph.json 内容 | 2 |
| `/api/v1/workgraph/stream` | GET | SSE: workgraph 变更事件 | 2 |
| `/api/v1/tests` | GET | 返回测试结果摘要 | 4 |
| `/api/v1/tests/run` | POST | 触发测试运行，SSE 推送进度 | 4 |

**SSE 响应格式**（所有 SSE 端点共享）：
```
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: keep-alive

event: <event_type>
data: <JSON payload>

```

### FileWatcher

位于 `src/visual/file_watcher.rs`。监听 `workgraph.json` 文件变化。

**实现**：
- 使用 `notify` crate 的 `RecommendedWatcher`
- 监听 `Modify` + `Create` 事件
- 300ms debounce（daemon 写文件用 temp + rename 模式）
- 读取失败时静默跳过
- 变化后重新读取并序列化，通过 EventRouter 推送给 SSE 连接

## Phase 1：实时时间线

### 文件清单

```
src/
├── bin/cc-web.rs              ← 新二进制入口
├── visual/
│   ├── mod.rs                 ← 模块声明
│   ├── socket_client.rs       ← 连接 daemon 的 cc 协议客户端
│   ├── event_router.rs        ← 事件分发 + SSE 管理
│   ├── http_server.rs         ← tiny_http 端点（含静态文件服务）
│   └── file_watcher.rs        ← workgraph.json 监听（骨架：初始化 + 空回调）
└── static/
    └── index.html             ← 单页前端
```

### Phase 1 前端

**布局**：实时时间线是默认 tab，从上到下展示事件流。

**渲染规则**：
- `StreamDelta` → 灰色文本，逐字符追加到"推理"气泡
- `ToolStarted` → 蓝色卡片，显示工具名 + preview
- `ToolFinished` → 更新已显示的工具卡片：绿色 ✅（成功）或红色 ❌（错误）
- `Reasoning` → 可折叠的灰色推理块
- `TurnComplete` → 分隔线 + 本轮统计信息
- `Context` → 更新状态栏的上下文百分比
- `BusNotice` → 橙色通知条

**状态栏**：顶部固定，显示 daemon 连接状态、上下文使用率、当前 turn 编号。

### 显式排除

| 项 | 移至 |
|----|------|
| Workgraph 图 | Phase 2 |
| Session 回放 | Phase 3 |
| 测试热力图 | Phase 4 |
| 认证/鉴权 | 不实现（仅监听 localhost） |
| 事件持久化 | 可选增强 |
| 前端构建工具 | 不引入 |
| 加密/wss | 不实现 |
| 配置管理 | 环境变量 `CC_WEB_PORT`（默认 9876） |

## Phase 2：Workgraph 可视化

### 后端增量

- `file_watcher.rs` 从骨架变为完整实现
- 新增 `GET /api/v1/workgraph` — 读取 `workgraph.json`
- 新增 `GET /api/v1/workgraph/stream` — SSE 推送变更

### 前端

**渲染模式**（可切换）：
1. **树形布局**（默认）：按 deps 拓扑排序，从上到下
2. **力导向图**：D3 force simulation

**节点状态颜色**：
| 状态 | 颜色 | 效果 |
|------|------|------|
| `pending` | 灰色 | 静态 |
| `in_progress` | 蓝色 | 脉冲动画 |
| `completed` | 绿色 | 静态 ✅ |
| `needs_fix` | 红色 | 闪烁 |
| `skipped` | 橙色 | 半透明 |

**交互**：
- 点击 → 详情面板（title、acceptance、verdict、touched、last_failure）
- 悬停 → tooltip（状态 + fix_attempts）
- 力导向模式下可拖拽

## Phase 3：Session 回放

### 后端增量

- 新增 `GET /api/v1/sessions` — 调用 `ListSessions` 协议
- 新增 `GET /api/v1/sessions/:id` — 调用 `ExportSession` 协议
- 新增 `GET /api/v1/sessions/:id/events` — 解析消息序列

### 前端

**Session 列表**：卡片式布局，每张卡片显示 ID、时间、消息数、工具调用数、时长、摘要。

**回放视图**：
- 控制栏：⏮ ⏸ ⏭ + 速度选择（0.5x/1x/2x/4x）
- 进度条：可拖拽跳转
- 消息折叠策略：
  - `StreamDelta` 合并为"Assistant 思考"条目（截断前 200 字）
  - `ToolStarted` + `ToolFinished` 合并为工具调用卡片
  - 长 output 默认折叠，点击展开
  - 显示相对时间戳（"+2s"）

## Phase 4：测试热力图

### 后端增量

- 新增 `GET /api/v1/tests` — 运行 `cargo test --no-run` 列出测试 + 解析上次结果
- 新增 `POST /api/v1/tests/run` — 异步运行 `cargo test`，SSE 推送进度

### 前端

**热力图矩阵**：
- 行 = 测试用例，列 = 模块
- 颜色：深绿(快速通过) → 浅绿(通过) → 黄色(慢) → 橙色(跳过) → 红色(失败)
- 底部行：模块通过率
- 交互：悬停 tooltip、点击跳转源码、运行按钮

## 依赖增量

```
cc-web (新二进制)
├── tiny_http = "0.12"     — HTTP server
├── notify = "6"           — 文件监听
└── serde / serde_json     — 已有（Cargo.toml 已有）
```

## 启动方式

```bash
# 1. 先启动 daemon
CODECODER_DAEMON=1 cargo run

# 2. 再启动 cc-web
cargo run --bin cc-web

# 可选参数
cargo run --bin cc-web -- --port 9877 --daemon-socket /tmp/codecoder.sock
```

## 错误处理

| 场景 | 行为 |
|------|------|
| daemon 未运行 | 启动时报错退出，提示启动 daemon |
| daemon 运行时断开 | 自动重试最多 3 次，前端显示 "⚠ 连接断开" |
| HTTP 端口被占用 | 报错退出，提示 `--port` 换端口 |
| SSE 客户端断开 | 静默移除，不影响其他连接 |
| workgraph.json 读取失败 | 返回 404/空图 |
| 单个 SSE 事件序列化失败 | 跳过该事件 |

## 测试策略

| 层 | 内容 | 方式 |
|----|------|------|
| 单元测试 | SocketClient 状态机、EventRouter 分发 | mock daemon socket（内存 channel） |
| 集成测试 | 启动测试 daemon → 连接 → 收事件 | `tests/visual/` 下，类似已有 L1 |
| 前端测试 | 暂不覆盖 | 手动验证 |

## 里程碑

| Phase | 估计文件数 | 估计代码行 | 可交付 |
|-------|-----------|-----------|--------|
| 1 | 6 文件 | ~400 Rust + ~200 JS/HTML | 实时时间线 |
| 2 | +2 文件 | ~150 Rust + ~150 JS | Workgraph 图 |
| 3 | +1 文件 | ~100 Rust + ~200 JS | Session 回放 |
| 4 | +1 文件 | ~150 Rust + ~150 JS | 测试热力图 |