# BG workgraph 入口接通 + Error(4) 补全 — 设计文档

- **日期**: 2026-07-22
- **状态**: 待用户审阅(Pending user review)
- **作者**: Claude Code(brainstorming 产物)
- **起因**: `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` P8 发现——经 `CODECODER_BG_TASK` 接口,BG 退出码 2/3/4 不可达(只 0 可观测)。
- **关联**: ADR 0033(BG 账本与退出码)、ADR 0030(BG 客观验收门)、`src/main.rs`、`src/background.rs`、`src/lib.rs`、`src/agent.rs`、`src/bg_gate.rs`、`src/bg_ledger.rs`。

## 1. 背景与目标

上限压测(P8)坐实:BG one-shot runner 经 `CODECODER_BG_TASK` 入口**永远走显式 task 单 turn 分支**(`background.rs:115-129`),mission_state 留默认 `Running` → exit 0。导致:

- **退出码 2(BlockedAt)/ 3(CircuitBreaker)/ CompletedAllReady 不可达**——它们只在 `run_background_cfg` 的 workgraph 分支(`background.rs:131-195`)产生,该分支要求 task 为空;但 `main.rs:6-10` 要求 `CODECODER_BG_TASK` 非空才进 BG(空则走 daemon)。
- **退出码 4(Error)从不构造**——`MissionState::Error` 全代码库唯一构造点是 `background.rs:453` 的 `#[test]`;provider 错误在 `process_turn` 内被推成 `AgentEvent::StreamDelta("error: …")`(`agent.rs:844`),`drain_bg_events` 无从区分"失败"与"成功带文本" → mission_state 留 `Running` → exit 0。
- **`cc ledger --failed`(= 非 CompletedAllReady)误报全部** BG run。

**目标**: 接通 workgraph-BG 入口 + 补 Error(4),让 ADR 0033 的退出码 0/2/3/4 **全部 live 可达**,`cc ledger --failed` 语义回归正确。**不改 one-shot 显式 task 契约、不动 SIGINT→exit 0 语义。**

## 2. 已锁定决策

| 维度 | 决策 |
|---|---|
| 范围 | **P8 单独成 spec**(P9/P5/P10/P11 各自独立 spec) |
| workgraph 触发信号 | **B2 专用 env `CODECODER_BG_WORKGRAPH=1`**(非哨兵值,无 magic 碰撞) |
| Error(4) | **一并补**(显式 + workgraph 两分支,provider 错误→`Error`→exit 4) |
| ADR | **修订 ADR 0033**(兑现既有契约 + 补 env),不新建 ADR |

## 3. 架构

```
main.rs(入口路由)
  ├─ CODECODER_BG_TASK 非空           → run_background(cfg, task)      [显式 task,同今]
  ├─ CODECODER_BG_WORKGRAPH=1(且无显式 task)→ run_background(cfg, "")  [workgraph 模式,新]
  └─ 否则                              → run_daemon(cfg)               [同今]

background.rs::run_background_cfg
  ├─ 显式 task 分支(:115)  → run_one_turn → drain_bg_events
  │     drain 捕 TurnError → mission_state=Error           [新:Error(4)]
  └─ workgraph 分支(:131)  → advance_one_milestone 循环
        advance 内 turn 出 TurnError → 返回失败信号
        run_background_cfg catch → mission_state=Error(不 ?逃逸)  [新:Error(4)]
        正常 next_action → BlockedAt(2)/CircuitBreaker(3)/CompletedAllReady  [现已有,现可达]

agent.rs::process_turn
  provider 错误处(:844) → 并发发 StreamDelta("error:…")(人看)+ TurnError(msg)(机器判)[新事件]

lib.rs::run_background
  mission_exit_code(mission_state) → 0/2/3/4 → process::exit          [现已有,现全可达]
```

**关键不变量**:workgraph-BG 的全部下游(background.rs workgraph 分支、bg_gate、bg_ledger、lib.rs label `task_label="workgraph"`)均已就绪——`lib.rs:55` 已特判空 task 为 `"workgraph"` 标签,证明空 task=workgraph 是原设计意图。**唯一缺的是 main.rs 入口路由 + Error 信号**。

## 4. Part 1 — 接通 workgraph-BG 入口

### 4.1 入口路由(可测纯函数)

把 main.rs 的 env 判定抽成可单测的纯函数。黑盒测试(`tests/`)只编译 `src/lib.rs` 公共 API,故该函数**必须放 `lib.rs` 且 `pub`**(`fn main` 不能被单测):

```rust
// src/lib.rs(pub,供 main.rs 与 tests 调用)
pub enum BgMode { Explicit(String), Workgraph }

/// 从 env 解析 BG 模式。优先级:显式 task > workgraph 哨兵 > None(走 daemon)。
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

`fn main` 改为:`match codecoder::bg_mode_from_env() { Some(BgMode::Explicit(t)) => codecoder::run_background(cfg, t), Some(BgMode::Workgraph) => codecoder::run_background(cfg, String::new()), None => codecoder::run_daemon(cfg) }`。

### 4.2 解锁项

- BlockedAt(2)、CircuitBreaker(3)、CompletedAllReady 经 workgraph 分支 **live 可达**。
- `cc ledger --failed`(= 非 CompletedAllReady)语义回归:正常完成不再被误报。

### 4.3 命令

```bash
CODECODER_BG_WORKGRAPH=1 codecoder          # workgraph-BG 模式(推进就绪里程碑)
CODECODER_BG_TASK="做 X" codecoder          # 显式 task 单 shot(同今)
codecoder                                   # daemon(同今)
```

## 5. Part 2 — 补 Error(4):turn 失败信号

### 5.1 新增 AgentEvent::TurnError

```rust
// src/agent.rs AgentEvent 枚举
/// 一个 turn 因 provider/transport 错误失败(非取消、非正常结束)。
/// BG runner 据此置 mission_state=Error → exit 4(ADR 0033)。
TurnError(String),
```

在 `process_turn` provider 错误处(`agent.rs:844`)与现有 `StreamDelta` **并发发出**:

```rust
let _ = event_tx.send(AgentEvent::StreamDelta(format!("error: {e}")));  // 保留:人看
let _ = event_tx.send(AgentEvent::TurnError(msg.clone()));              // 新:机器判
```

### 5.2 显式 task 分支:drain 捕获

```rust
// src/background.rs drain_bg_events
AgentEvent::TurnError(m) => {
    out.mission_state = crate::bg_gate::MissionState::Error(m.clone());
    out.events.push(format!("turn error: {m}"));
}
```

### 5.3 workgraph 分支:advance 传播 + catch

`advance_one_milestone` 内部 turn 若出 TurnError → 返回失败信号(让 advance 在其 drain 里检测 TurnError 并返回 `Err`);`run_background_cfg` 把现在的 `advance_one_milestone(...)?`(`background.rs:140`,会 propagate 成 anyhow Err → exit 1)**改成 catch**:

```rust
let step = match advance_one_milestone(...) {
    Ok(Some(s)) => s,
    Ok(None) => { /* 无就绪,CompletedAllReady */ ... }
    Err(e) => { out.mission_state = MissionState::Error(e.to_string()); break; }  // 新:catch→Error,不 ?逃逸
};
```

### 5.4 SIGINT 不动

取消仍走 `hit_tool_cap=false; break` → mission_state=`Running` → exit 0(操作者主动,非故障)。**不改。**

### 5.5 不区分 context-overflow

context-overflow 与普通 provider 错都归 `Error(4)`;overflow 的人类可读 Notice(`agent.rs:840-842`)保留。不为 overflow 单独建 mission_state。

## 6. 测试(TDD,hermetic)

- **`bg_mode_from_env()` 单元**:`WORKGRAPH=1`→Workgraph;`TASK=x`→Explicit;`TASK=x + WORKGRAPH=1`→Explicit(显式优先);都无→None;`WORKGRAPH=0`/它值→None。
- **`drain_bg_events` 单元**:喂 `TurnError("boom")` → 断言 `out.mission_state == Error("boom")`。
- **`run_background_cfg` + `ScriptedProvider`**(pub(crate),hermetic):
  - 显式 task + provider error → `Error` → `mission_exit_code==4`。
  - workgraph(空 task)+ 故意失败里程碑(`CIRCUIT_K=1`)→ `CircuitBreaker` → exit 3。
  - workgraph + deps 断 → `BlockedAt(_)` → exit 2。
  - workgraph + 全 Done/无就绪 → `CompletedAllReady` → exit 0。
- **集成 `tests/l1_background.rs`**:BG workgraph 模式经空 task 触发,黑盒断言产出 BlockedAt/CircuitBreaker(修复前不可达)。
- **live 复验**(`codecoder-probe/` lab,非单测):重跑 P8 的 p8_ok/p8_err + 构造 BlockedAt/CircuitBreaker 场景,确认 2/3/4 现可达、`--failed` 不再误报。

## 7. ADR / 文档同步

- **ADR 0033**:补 `CODECODER_BG_WORKGRAPH=1` 入口;更新退出码可达性(0/2/3/4 现全 live 可达);记录 `AgentEvent::TurnError` 为 Error(4) 来源。
- **README** env 表:加 `CODECODER_BG_WORKGRAPH`;ADR 0033 节退出码表加可观测性脚注。
- **ARCHITECTURE.md / CLAUDE.md**:main.rs 模块地图补 workgraph-BG 入口分支;Background Agent 段补 workgraph 模式说明。
- 不新建 ADR(兑现 0033/0030 既有契约 + 补 env,修订 0033 即可)。

## 8. 不在本范围内(YAGNI)

- **P9 workgraph 并发保护 / P5 复合命令 keying / P10 长度截断 guard / P11 daemon SIGINT**——各自独立 spec。
- **混合模式**(显式 task 跑完再推进 workgraph,brainstorm 方案 C)——不做,保 one-shot 契约。
- **不区分 context-overflow 与普通 provider 错**——都归 Error(4)。
- **不动 SIGINT→exit 0** / 不新增 mission_state / 不改 Persistent 自动重启(ADR 0021 守)。
- **不实现 BG 内置调度器/多 runner 资源上限**(ADR 0026 延后项)。
