# BG workgraph 入口接通 + Error(4) 补全 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 接通 BG workgraph 模式入口 + 补 Error(4),让 ADR 0033 退出码 0/2/3/4 全部 live 可达,`cc ledger --failed` 不再误报。

**Architecture:** Part 1——`lib.rs` 加可测纯函数 `bg_mode_from_env()`,`main.rs` 用它路由:`CODECODER_BG_TASK` 非空→显式 BG;`CODECODER_BG_WORKGRAPH=1`→空 task→workgraph 分支(下游已就绪)。Part 2——`AgentLoop` 加 `last_error: Option<String>` 字段,provider 错误时置位;BG runner 两分支(显式 / workgraph)在 `run_one_turn` 后读它,有错→`mission_state=Error`→exit 4。**用进程内字段而非新 AgentEvent 变体**,避免 daemon→client wire(socket.rs/proto.rs)ripple。

**Tech Stack:** Rust(codecoder 本体)、cargo test(hermetic 单元 + `tests/` 黑盒)、`ScriptedProvider`/test-local providers、bash smoke。

## Global Constraints

- **改 src/ 须同步 ADR/README/ARCHITECTURE/CLAUDE 文档**(Task 6),保持文档与代码一致(遵 CLAUDE.md)。
- **TDD**:每个任务先写失败测试 → 跑红 → 最小实现 → 跑绿 → commit。
- **不破坏既有测试**:`cargo test` 全绿(当前 282 passed)。
- **领域术语**遵 `CONTEXT.md`。
- **commit 规范**:conventional commits + 中文正文;提交到 `fix/bg-workgraph-entry` 分支。
- **Part 2 机制**:用 `AgentLoop::last_error` 字段(**非** spec 起初设想的 `AgentEvent::TurnError`——后者会 ripple socket.rs/proto.rs wire;字段法零 wire 改动、同效果,见 Self-Review)。
- **不动 SIGINT→exit 0 语义**、不区分 context-overflow(都归 Error(4))、不改 one-shot 显式 task 契约。

## File Structure

- Modify: `src/lib.rs` — 加 `BgMode` enum + `pub fn bg_mode_from_env()`(可测纯函数)+ 单元测试。
- Modify: `src/main.rs` — `fn main` 改用 `codecoder::bg_mode_from_env()` 路由。
- Modify: `src/agent.rs` — `AgentLoop` 加 `last_error: Option<String>` 字段 + 初始化 + `process_turn` provider 错误处置位 + `pub fn last_error()`;加 FailingProvider 单测。
- Modify: `src/background.rs` — 显式分支读 `last_error`→Error;`advance_one_milestone` 读 `last_error`→`Err`;`run_background_cfg` catch `Err`→Error(替 `?`);加 FailingProvider 单测。
- Modify: `docs/adr/0033-*.md`、`README.md`、`ARCHITECTURE.md`、`CLAUDE.md` — 文档同步。

## 约定

```bash
CC_ROOT=/Users/rong.zhu/Code/codecoder
TEST_FN="cargo test --no-fail-fast 2>&1 | tail -20"
```

---

## Part 1 — 接通 workgraph-BG 入口

### Task 1: `bg_mode_from_env()` 纯函数 + 单元测试

**Files:**
- Modify: `src/lib.rs`(加 enum + 函数 + `#[cfg(test)]` 测试)

**Interfaces:**
- Produces: `pub enum BgMode { Explicit(String), Workgraph }` + `pub fn bg_mode_from_env() -> Option<BgMode>`(Task 2 的 main.rs 消费)。

- [ ] **Step 1: 写失败测试**(在 `src/lib.rs` 的 `#[cfg(test)] mod tests` 内,`run_background_ledger_append_and_exit_code` 旁)

```rust
#[test]
fn bg_mode_from_env_routes_correctly() {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var("CODECODER_BG_TASK");
        std::env::remove_var("CODECODER_BG_WORKGRAPH");
    }
    assert!(bg_mode_from_env().is_none(), "nothing set → None (daemon)");

    unsafe {
        std::env::set_var("CODECODER_BG_WORKGRAPH", "1");
    }
    assert!(matches!(bg_mode_from_env(), Some(BgMode::Workgraph)), "WORKGRAPH=1 → Workgraph");

    unsafe {
        std::env::set_var("CODECODER_BG_TASK", "do X");
    }
    assert!(matches!(bg_mode_from_env(), Some(BgMode::Explicit(t)) if t == "do X"), "explicit task wins over WORKGRAPH");

    unsafe {
        std::env::remove_var("CODECODER_BG_TASK");
        std::env::set_var("CODECODER_BG_WORKGRAPH", "0");
    }
    assert!(bg_mode_from_env().is_none(), "WORKGRAPH=0 (not '1') → None");

    unsafe {
        std::env::set_var("CODECODER_BG_TASK", "   ");
    }
    assert!(bg_mode_from_env().is_none(), "whitespace-only task → None");
    unsafe {
        std::env::remove_var("CODECODER_BG_TASK");
        std::env::remove_var("CODECODER_BG_WORKGRAPH");
    }
}
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test bg_mode_from_env_routes_correctly 2>&1 | tail -15`
Expected: 编译失败(`bg_mode_from_env` / `BgMode` 未定义)。

- [ ] **Step 3: 最小实现**(在 `src/lib.rs` `run_background` 上方)

```rust
/// BG 模式的 env 路由结果(ADR 0033)。空 task→workgraph 分支由 background.rs 处理。
pub enum BgMode {
    Explicit(String),
    Workgraph,
}

/// 从 env 解析 BG 模式。优先级:显式非空 task > WORKGRAPH 哨兵 > None(走 daemon)。
pub fn bg_mode_from_env() -> Option<BgMode> {
    if let Ok(task) = std::env::var("CODECODER_BG_TASK") {
        if !task.trim().is_empty() {
            return Some(BgMode::Explicit(task));
        }
    }
    if std::env::var("CODECODER_BG_WORKGRAPH").map(|v| v == "1").unwrap_or(false) {
        return Some(BgMode::Workgraph);
    }
    None
}
```

- [ ] **Step 4: 跑测试看绿**

Run: `cargo test bg_mode_from_env_routes_correctly 2>&1 | tail -8`
Expected: `test bg_mode_from_env_routes_correctly ... ok`。

- [ ] **Step 5: commit**

```bash
git add src/lib.rs
git commit -m "feat(bg): bg_mode_from_env 纯函数路由 CODECODER_BG_WORKGRAPH

可单测的 BG 模式解析:显式非空 task > WORKGRAPH=1 哨兵 > None。
为 main.rs 接通 workgraph-BG 入口铺路(空 task→workgraph 分支下游已就绪)。"
```

### Task 2: main.rs 接入 bg_mode_from_env + smoke

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `codecoder::bg_mode_from_env()`(Task 1)、`codecoder::run_background`、`codecoder::run_daemon`。
- Produces: 生产入口可达 workgraph-BG 模式(`CODECODER_BG_WORKGRAPH=1 codecoder`)。

- [ ] **Step 1: 改 main.rs**

把 `src/main.rs` 整体替换为:

```rust
// CodeCoder — 入口分发 shim。三条路径(ADR 0016/0026/0033 + client-server migration):
//   1. CODECODER_BG_TASK=<task>      → headless background runner,显式单 shot
//   2. CODECODER_BG_WORKGRAPH=1      → headless background runner,workgraph 逐里程碑模式
//   3. 其它                          → ccd daemon(client-server 架构)
fn main() -> anyhow::Result<()> {
    let cfg = codecoder::Config::from_env();
    match codecoder::bg_mode_from_env() {
        Some(codecoder::BgMode::Explicit(task)) => codecoder::run_background(cfg, task),
        Some(codecoder::BgMode::Workgraph) => codecoder::run_background(cfg, String::new()),
        None => codecoder::run_daemon(cfg),
    }
}
```

- [ ] **Step 2: 编译**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` dev;无 error/warning(确认 `run_daemon` 仍 pub 可见——若编译报 `run_daemon` 私有,加 `pub`)。

- [ ] **Step 3: smoke——workgraph 入口不误起 daemon**

```bash
cd "$CC_ROOT"
# 无 workgraph 也没有 task → 应起 daemon(socket 出现);测完关掉
CODECODER_ROOT=/tmp/cc_smoke_daemon target/debug/codecoder & sleep 1
ls /tmp/cc_smoke_daemon/.ccd.sock 2>&1 && echo "DAEMON_OK" || echo "DAEMON_FAIL"
kill -TERM %1 2>/dev/null; rm -rf /tmp/cc_smoke_daemon
# CODECODER_BG_WORKGRAPH=1 + 空 workgraph → BG 跑完即退(空图→CompletedAllReady→exit 0),不应留 socket
mkdir -p /tmp/cc_smoke_wg
CODECODER_BG_WORKGRAPH=1 CODECODER_ROOT=/tmp/cc_smoke_wg target/debug/codecoder 2>&1 | tail -3
echo "exit=$?"
ls /tmp/cc_smoke_wg/.ccd.sock 2>&1 || echo "NO_SOCKET(BG 不起 daemon)✓"
rm -rf /tmp/cc_smoke_wg
```
Expected: `DAEMON_OK`;workgraph BG 末行 `=== summary: ... ===` 且 `exit=0`、无 socket 残留。

- [ ] **Step 4: 跑全测试确认无回归**

Run: `cargo test 2>&1 | grep -E 'test result' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。

- [ ] **Step 5: commit**

```bash
git add src/main.rs
git commit -m "feat(main): 接通 CODECODER_BG_WORKGRAPH 入口走 workgraph-BG

main.rs 改用 bg_mode_from_env 路由:显式 task→单 shot,WORKGRAPH=1→空 task
走 workgraph 分支(下游 background.rs/bg_gate/lib.rs:55 全就绪)。解锁
BlockedAt(2)/CircuitBreaker(3)/CompletedAllReady 经生产入口可达。"
```

---

## Part 2 — 补 Error(4):AgentLoop.last_error 贯通

### Task 3: AgentLoop.last_error 字段 + provider 错误置位

**Files:**
- Modify: `src/agent.rs`(struct 字段 + `new`/`new_background` 初始化 + `process_turn:844` 置位 + getter + 测试)

**Interfaces:**
- Produces: `impl AgentLoop { pub fn last_error(&self) -> Option<&str>; }`(Task 4/5 消费)。

- [ ] **Step 1: 写失败测试**(在 `src/agent.rs` `#[cfg(test)]` 内,`FlakyProvider`(约 :1870)旁加一个 always-failing provider + 测试)

```rust
struct FailingProvider;
impl Provider for FailingProvider {
    fn name(&self) -> &str { "failing" }
    fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Completion> {
        Err(anyhow::anyhow!("provider down: simulated 503"))
    }
}

#[test]
fn provider_error_sets_last_error() {
    let dir = std::env::temp_dir().join(format!("cc_lasterr_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let provider = Arc::new(FailingProvider);
    let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());
    let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
    agent.run_one_turn("do something".into(), &tx);
    drop(tx);
    for _ in rx {}
    assert!(agent.last_error().is_some(), "provider error should set last_error");
    assert!(agent.last_error().unwrap().contains("503"), "got: {:?}", agent.last_error());
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test provider_error_sets_last_error 2>&1 | tail -12`
Expected: 编译失败(`last_error` 方法未定义)。

- [ ] **Step 3: 加字段 + 初始化 + getter**

在 `AgentLoop` struct 加字段(找现有 `tool_cap`、`tier2` 字段旁):
```rust
    /// 最近一次 turn 的 provider 错误(若有)。BG runner 据此置 mission_state=Error(ADR 0033)。
    last_error: Option<String>,
```
在 `AgentLoop::new` 与 `new_background`(两处构造,约 agent.rs:349 与 new_background 处)初始化:
```rust
            last_error: None,
```
加 getter(在 `set_tool_cap` 旁):
```rust
    /// 最近一次 turn 是否因 provider 错误失败。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
```

- [ ] **Step 4: provider 错误处置位**

在 `process_turn` 的 provider 错误分支(约 `src/agent.rs:844`,`let _ = event_tx.send(AgentEvent::StreamDelta(format!("error: {e}")));` 那行)**之后、`hit_tool_cap = false;` 之前**加:
```rust
                    self.last_error = Some(msg.clone());
```
(`msg` 即 `let msg = e.to_string();`,已在该分支开头定义。)

- [ ] **Step 5: 跑测试看绿**

Run: `cargo test provider_error_sets_last_error 2>&1 | tail -6`
Expected: `test provider_error_sets_last_error ... ok`。

- [ ] **Step 6: commit**

```bash
git add src/agent.rs
git commit -m "feat(agent): AgentLoop.last_error 记录 provider 错误

process_turn provider 错误分支置 self.last_error(与现有 StreamDelta 并存);
pub fn last_error() 供 BG runner 读取。用进程内字段而非新 AgentEvent 变体,
避免 daemon→client wire(socket.rs/proto.rs)ripple。"
```

### Task 4: 显式 BG 分支读 last_error → mission_state=Error

**Files:**
- Modify: `src/background.rs`(显式分支 :115-129 加判定;测试加 FailingProvider)

**Interfaces:**
- Consumes: `AgentLoop::last_error()`(Task 3)。
- Produces: 显式 task + provider 错 → `BgOutcome.mission_state = Error`→ exit 4。

- [ ] **Step 1: 写失败测试**(在 `src/background.rs` `#[cfg(test)]` 内,引用 `MissionState`)

```rust
    #[test]
    fn explicit_task_provider_error_yields_error_state() {
        use crate::provider::Provider;
        struct FailingProvider;
        impl Provider for FailingProvider {
            fn name(&self) -> &str { "failing" }
            fn complete(&self, _req: &crate::provider::CompletionRequest) -> anyhow::Result<crate::provider::Completion> {
                Err(anyhow::anyhow!("provider down: simulated 503"))
            }
        }
        let dir = std::env::temp_dir().join(format!("cc_experr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = run_background_cfg(
            Arc::new(FailingProvider), "m".into(), 256, 0.0, dir.clone(),
            "do something".into(), 3, 2, 8,
        ).unwrap();
        assert!(matches!(out.mission_state, MissionState::Error(_)), "got {:?}", out.mission_state);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test explicit_task_provider_error_yields_error_state 2>&1 | tail -10`
Expected: FAIL(`mission_state` 是 `Running`,非 `Error`)。

- [ ] **Step 3: 显式分支加判定**

在 `src/background.rs` 显式分支(`agent.run_one_turn(task, &tx); drop(tx); drain_bg_events(rx, &mut out);` 之后、`return Ok(out);` 之前,约 :127-128)加:
```rust
        if let Some(e) = agent.last_error() {
            out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
        }
```

- [ ] **Step 4: 跑测试看绿**

Run: `cargo test explicit_task_provider_error_yields_error_state 2>&1 | tail -6`
Expected: `... ok`。

- [ ] **Step 5: commit**

```bash
git add src/background.rs
git commit -m "feat(bg): 显式 task provider 错误→mission_state=Error→exit 4

run_background_cfg 显式分支读 AgentLoop.last_error(),有错置 Error。
修复 P8:provider 错误原本留 Running→exit 0(误报成功),现正确→exit 4。"
```

### Task 5: workgraph 分支 advance 传 last_error → catch → Error

**Files:**
- Modify: `src/background.rs`(`advance_one_milestone` 加 `last_error`→`Err`;`run_background_cfg` 把 `?` 改 catch)

**Interfaces:**
- Consumes: `AgentLoop::last_error()`(Task 3)。
- Produces: workgraph 模式 provider 错 → `mission_state=Error`→ exit 4。

- [ ] **Step 1: 写失败测试**(在 `src/background.rs` `#[cfg(test)]`)

```rust
    #[test]
    fn workgraph_provider_error_yields_error_state() {
        use crate::provider::Provider;
        struct FailingProvider;
        impl Provider for FailingProvider {
            fn name(&self) -> &str { "failing" }
            fn complete(&self, _req: &crate::provider::CompletionRequest) -> anyhow::Result<crate::provider::Completion> {
                Err(anyhow::anyhow!("provider down: simulated 503"))
            }
        }
        let dir = std::env::temp_dir().join(format!("cc_wgerr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ws(&dir, &[(1, "echo ok", vec![])]);   // 有就绪里程碑,但 provider 会错
        let out = run_background_cfg(
            Arc::new(FailingProvider), "m".into(), 256, 0.0, dir.clone(),
            "".into(), 3, 2, 8,   // 空 task → workgraph 分支
        ).unwrap();
        assert!(matches!(out.mission_state, MissionState::Error(_)), "got {:?}", out.mission_state);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test workgraph_provider_error_yields_error_state 2>&1 | tail -10`
Expected: FAIL(workgraph 分支 provider 错当前不置 Error)。

- [ ] **Step 3: advance_one_milestone 读 last_error → 返回 Err**

在 `advance_one_milestone` 内 `drain_bg_events(rx, &mut out);`(约 background.rs:265)**之后**、客观验收门之前加:
```rust
    if let Some(e) = agent.last_error() {
        return Err(anyhow::anyhow!(e.to_string()));
    }
```

- [ ] **Step 4: run_background_cfg 把 `?` 改成 catch**

把 `src/background.rs` 约自 :140 的:
```rust
        let step = match advance_one_milestone(
            provider.clone(),
            model.clone(),
            max_tokens,
            temperature,
            root.clone(),
        )? {
            Some(s) => s,
            None => {
                // 无就绪 milestone(空图或全部完成/阻塞)。
                if out.mission_state == crate::bg_gate::MissionState::Running {
                    out.mission_state = crate::bg_gate::MissionState::CompletedAllReady;
                }
                break;
            }
        };
```
改为:
```rust
        let step = match advance_one_milestone(
            provider.clone(),
            model.clone(),
            max_tokens,
            temperature,
            root.clone(),
        ) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // 无就绪 milestone(空图或全部完成/阻塞)。
                if out.mission_state == crate::bg_gate::MissionState::Running {
                    out.mission_state = crate::bg_gate::MissionState::CompletedAllReady;
                }
                break;
            }
            Err(e) => {
                out.mission_state = crate::bg_gate::MissionState::Error(e.to_string());
                break;
            }
        };
```

- [ ] **Step 5: 跑测试看绿**

Run: `cargo test workgraph_provider_error_yields_error_state 2>&1 | tail -6`
Expected: `... ok`。

- [ ] **Step 6: 跑全 BG 测试无回归**

Run: `cargo test 2>&1 | grep -E 'test result' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`(含 t1/t2/t4/t5 既有 gate 测试)。

- [ ] **Step 7: commit**

```bash
git add src/background.rs
git commit -m "feat(bg): workgraph 分支 provider 错误→Error(不再 ?逃逸成 exit 1)

advance_one_milestone 读 last_error→Err;run_background_cfg 把 advance(...)? 改成
catch→mission_state=Error→break。workgraph 模式 provider 错现正确→exit 4
(原本 ? 逃逸成 anyhow Err→exit 1)。"
```

---

## Part 3 — 文档同步 + live 复验

### Task 6: ADR 0033 + README + ARCHITECTURE/CLAUDE 同步

**Files:**
- Modify: `docs/adr/0033-bg-ledger-and-exit-codes.md`、`README.md`、`ARCHITECTURE.md`、`CLAUDE.md`

- [ ] **Step 1: ADR 0033 补入口 + 可达性**

在 `docs/adr/0033-bg-ledger-and-exit-codes.md` 末尾加一节:
```markdown
## 修订(2026-07-22):workgraph-BG 入口 + Error(4) 可达

- **入口**: `CODECODER_BG_WORKGRAPH=1`(显式 task 缺省时)→ `run_background` 传空 task → workgraph 逐里程碑分支。`CODECODER_BG_TASK=<非空>` 仍走显式单 shot。
- **退出码全可达**: 经此入口,`BlockedAt(2)`/`CircuitBreaker(3)`/`CompletedAllReady(0)` 由 `bg_gate::next_action` 产出;`Error(4)` 由 `AgentLoop.last_error`(provider 错误)在两分支置位。0/2/3/4 现全部 live 可观测(此前经 `CODECODER_BG_TASK` 仅 0 可达)。
- **`cc ledger --failed`**(= 非 CompletedAllReady)语义随 `CompletedAllReady` 可达而回归正确。
```

- [ ] **Step 2: README env 表加 WORKGRAPH + 退出码表脚注**

在 `README.md` 环境变量表的 `CODECODER_BG_TASK` 行后加:
```markdown
| `CODECODER_BG_WORKGRAPH` | — | 设置为 `1` 时以 headless workgraph 模式跑(逐里程碑推进,无显式 task;产出 mission_state→退出码 0/2/3/4,见 ADR 0033) |
```
在 BG 账本退出码表的 `Error(_)` 行备注栏补:`(provider 错误经 AgentLoop.last_error 置位,ADR 0033 修订)`。

- [ ] **Step 3: ARCHITECTURE.md / CLAUDE.md 同步**

`ARCHITECTURE.md` 模块地图 `main.rs` 行改描述为:`入口分发:BG_TASK→显式 BG、BG_WORKGRAPH=1→workgraph BG、否则→daemon`;Background Agent 段补一句 workgraph 模式经 `CODECODER_BG_WORKGRAPH` 触发。`CLAUDE.md` 的 Background Agent 段同步 `CODECODER_BG_WORKGRAPH` 说明。

- [ ] **Step 4: 跑全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add docs/adr/0033-bg-ledger-and-exit-codes.md README.md ARCHITECTURE.md CLAUDE.md
git commit -m "docs: ADR 0033 修订 + 同步 CODECODER_BG_WORKGRAPH 入口与退出码可达性

记录 workgraph-BG 入口(CODECODER_BG_WORKGRAPH=1)与 Error(4)经
AgentLoop.last_error 可达;README env 表 + ARCHITECTURE/CLAUDE 同步。"
```

### Task 7: live 复验(codecoder-probe lab)

**Files:** 无源码改动;在 `codecoder-probe/` lab 复跑 P8 场景验证 0/2/3/4 可达。

- [ ] **Step 1: 重编译 + 正常完成(CompletedAllReady → 0)**

```bash
cd "$CC_ROOT" && cargo build 2>&1 | tail -2
LAB=/Users/rong.zhu/Code/codecoder-probe
set -a; . .ccd.env; set +a
# workgraph 模式 + 空 workgraph → 无就绪 → CompletedAllReady → exit 0
rm -f "$LAB/workgraph.json" "$LAB/bg_ledger.jsonl"
CODECODER_BG_WORKGRAPH=1 CODECODER_ROOT="$LAB" target/debug/codecoder >/tmp/wg_ok.log 2>&1; echo "exit=$?"
tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=0`;ledger 末行 `state`=`"CompletedAllReady"`(不再是 `Running`)。

- [ ] **Step 2: BlockedAt(2) 可达**

```bash
cat > "$LAB/workgraph.json" <<'EOF'
{"milestones":[{"id":1,"title":"ghost","status":"Done","acceptance":"","deps":[]},
{"id":2,"title":"real","status":"Ready","acceptance":"","deps":[999]}]}
EOF
CODECODER_BG_WORKGRAPH=1 CODECODER_ROOT="$LAB" target/debug/codecoder >/tmp/wg_blk.log 2>&1; echo "exit=$?"
tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=2`;`state`=`BlockedAt(...)`。

- [ ] **Step 3: Error(4) 可达(坏 base)**

```bash
rm -f "$LAB/workgraph.json"  # 给一个就绪里程碑让 turn 真跑起来再撞 provider 错
CODECODER_BG_WORKGRAPH=1 CODECODER_API_BASE="https://invalid.invalid/v" CODECODER_ROOT="$LAB" target/debug/codecoder >/tmp/wg_err.log 2>&1; echo "exit=$?"
tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=4`;`state`=`Error(...)`(对比 P8 修复前同样场景是 exit 0 / Running)。

- [ ] **Step 4: `cc ledger --failed` 不再误报**

```bash
set -a; . .ccd.env; set +a
CODECODER_ROOT="$LAB" target/debug/cc ledger --failed 2>&1 | tail -5
```
Expected: 仅显示真正非 CompletedAllReady 的行(Step2/3 的 BlockedAt/Error),不再把 CompletedAllReady(Step1)算进去。

- [ ] **Step 5: 记录结论 + commit lab 证据(可选,lab 不进真实仓 git)**

把 4 步的 exit/state 汇总一句写入 `codecoder-probe/matrix.md` 末尾(append-only,lab scratch)。无需真实仓 commit。

---

## Self-Review(plan vs spec)

**0. 机制偏离说明(重要):** spec §5 设想新增 `AgentEvent::TurnError` 变体。实现中发现该变体会 ripple 到 daemon→client wire(`socket.rs` AgentEvent→ServerEvent 转换无 catch-all;`proto.rs ServerEvent` 需新变体 + ser/de;`client/mod.rs` 渲染)。改用 **`AgentLoop.last_error: Option<String>` 进程内字段**:BG runner 两分支在 `run_one_turn` 后直接读它,零 wire 改动、同效果(provider 错→`Error(4)`)。交互 daemon/cc 路径不变(仍用既有 `StreamDelta("error: …")` 显示)。**目标完全一致,机制更简。** 已在 Task 3 commit message 与 Global Constraints 标注。

**1. Spec coverage:**
- Part 1 workgraph 入口(spec §4)→ Task 1(bg_mode_from_env)+ Task 2(main.rs 接入)✓
- Part 2 Error(4)(spec §5)→ Task 3(last_error 字段)+ Task 4(显式分支)+ Task 5(workgraph 分支 catch)✓
- SIGINT 不动(spec §5.4)→ 无任何 SIGINT 改动 ✓
- 不区分 overflow(spec §5.5)→ last_error 对任何 provider Err 一视同仁置 Error ✓
- 测试(spec §6)→ 每 Task TDD + Task 7 live 复验 ✓
- ADR/文档(spec §7)→ Task 6 ✓
- 范围外(spec §8:不做 P9/P5/P10/P11、不做混合模式)→ 计划未涉 ✓

**2. Placeholder scan:** 无 TBD/TODO;每个 code step 含完整代码;测试用既有 `ws()`/`run_background_cfg`/`AgentLoop::new`/`FailingProvider` 模式(参照 background.rs:366/413 与 agent.rs:1870 FlakyProvider)✓

**3. Type consistency:** `BgMode::{Explicit(String), Workgraph}`(Task 1)与 main.rs(Task 2)`match` 一致;`AgentLoop::last_error() -> Option<&str>`(Task 3)与 background.rs(Task 4/5)`agent.last_error()` 调用一致;`MissionState::Error(String)`(Task 4/5)与 bg_gate.rs:104 一致;`advance_one_milestone -> anyhow::Result<Option<BgOutcome>>` 的 catch 改写(Task 5)类型自洽(`Err` arm `break` 发散)✓
