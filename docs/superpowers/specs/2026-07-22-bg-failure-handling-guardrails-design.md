# BG 失败处理 / 护栏 — 设计文档

- **日期**: 2026-07-22
- **状态**: 已批准(Approved)
- **分支**: `feat/bg-failure-guardrails`
- **作者**: Claude Code(brainstorming 产物)
- **关联**: `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`(审计发现)、ADR 0026(Background Agent)、`src/background.rs`、`src/workgraph.rs`、`src/tool/reason.rs`、`docs/background-agent-scheduling.md`

## 1. 目标

让 Background Agent 在长期无人值守下**失败安全**:单 turn 失败不固着、不回退代码当成功;失败**客观验收**、干净写回 workgraph + reason;按策略 continue/stop,不连环 flail。

**成功标准**:
- BG turn 结束后,有一个**客观验收门**(不靠 LLM 自评)判定 milestone 的 acceptance pass/fail。
- 门 fail 或 turn 触顶 → milestone 标 `needs_fix` + 加 reason 因果节点 + `BgOutcome.subgoals` 记录,**绝不**标 done。
- continue-vs-stop 策略生效(独立失败可继续、硬依赖失败停、连续 2 fail 熔断)。
- 全部新增逻辑有 hermetic 测试覆盖(ScriptedProvider,不烧 LLM、不依赖 Docker)。

## 2. 背景(审计实证)

`2026-07-21` 能力审计暴露:
- BG turn 遇失败测试**固着**:17 个工具全花在修一个 parser 测试,未及其余请求事项。
- **回退代码**:flailing 中把 mdslides crate 从 8/9 测试回退到 0 测试,且若靠 agent 自报会判成"完成"。
- milestone 卡 `in_progress`,workgraph 无法推进。

**根因**:`run_background` 已有 MAX_AUTO=3 milestone 自动推进 + 让 agent **自报** verdict(pass/needs_fix/rebuild)(`resolve_bg_task`),但**自报不是客观验收**——agent 可把回退后的代码自评为 pass。缺一个 turn 后的**客观验收门**与失败写回策略。

## 3. 已锁定的决策(brainstorming)

| 维度 | 决策 |
|---|---|
| 主轴 | 长期无人值守 BG(用户选定) |
| #1 支柱 | 失败处理/护栏(用户选定) |
| 护栏逻辑归属 | **方案 B:BG runner(background.rs)拥有任务策略**,agent loop 保持通用(用户选定) |

## 4. 架构

复用 `run_background` 既有的 MAX_AUTO=3 milestone 循环与 `resolve_bg_task`,在其上**加一层客观验收门 + 失败写回 + 任务策略**。不新建进程/线程,不改 agent loop,不改外部调度。

```
run_background (existing MAX_AUTO=3 milestone loop)
  for each milestone M (up to MAX_AUTO or circuit-break):
    1. resolve_bg_task(M) → task text            [existing]
    2. M.status = in_progress                      [existing]
    3. run ONE turn (AgentLoop::run_background_turn) [existing]
       └─ captures: tool_calls, denied, tool_cap_hit [tool_cap_hit: NEW signal]
    4. ★ POST-TURN ACCEPTANCE GATE ★               [NEW]
         (a) command gate: acceptance has command pattern → run_shell_cancellable → exit 0?
         (b) review gate: else review(M.touched) → Verdict == pass?
         (c) inconclusive fallback: gate errored / no pattern / acceptance empty
       objective gate VERDICT OVERRIDES agent self-report.
    5. ★ FAILURE WRITE-BACK ★ (on NeedsFix / Inconclusive)   [NEW]
         M.status = needs_fix; reason.add causal node; BgOutcome.subgoals.push
    6. ★ CONTINUE-VS-STOP POLICY ★                          [NEW]
         Pass & budget & ready → next M
         Fail & has dependents → stop (BlockedAt)
         Fail & independent & consecutive < K → next independent
         consecutive fails ≥ K (K=2) → stop (CircuitBreaker)
         no ready → stop (CompletedAllReady / BlockedAt)
  write BgOutcome { subgoals, mission_state, ... }
```

## 5. 组件

### 5.1 验收门(`bg_gate` 模块,纯函数 + 一个 shell 调用)

新模块 `src/bg_gate.rs`(或 `background.rs` 内子节),提供:

```rust
pub enum GateVerdict { Pass, NeedsFix(String), Inconclusive(String) }

/// 从 acceptance 文本提取可执行命令(若有)。
/// 识别: `cargo test`, `cargo build`, `cargo check`, `pytest`, `npm test`, `make <t>` 等。
/// 返回 (command, rest_acceptance)。
pub fn extract_gate_command(acceptance: &str) -> Option<String>;

/// 跑命令门(经 run_shell_cancellable,尊重 CancelToken)。exit 0 → Pass;否则 NeedsFix(stdout+stderr 摘要)。
pub fn run_command_gate(cmd: &str, root: &Path, cancel: &CancelToken) -> GateVerdict;

/// review 门:对 touched 文件跑 review 工具,按 Verdict==pass 判。
pub fn run_review_gate(touched: &[String], ...) -> GateVerdict;

/// 顶层调度:命令门 → review 门 → Inconclusive。
pub fn evaluate(milestone: &Milestone, touched: &[String], turn_ctx: &TurnCtx, cancel: &CancelToken) -> GateVerdict;
```

**关键安全性质**:门的 GateVerdict **覆盖** agent 在 turn 内的自报 verdict。agent 说 pass 但 `cargo test` exit≠0 → 最终 NeedsFix。

### 5.2 失败写回

门判定后:
- `Pass` → `M.status = done`(workgraph 既有动作)。
- `NeedsFix(reason)` / `Inconclusive(reason)` → `M.status = needs_fix`;调用 reason 工具的 add 语义,写一个因果节点到 `causal_tree.json`:
  - `question = "milestone #<id> 验收失败: <title>"`
  - `margin/leverage/terminal` 元数据:terminal=`boundary`(客观门不通过=边界),leverage 按 fail 类型。
  - body 含 gate_reason + tool_cap_hit + 触顶/ denied 摘要。
- 不论 pass/fail,push 一条 `SubgoalOutcome` 到 `BgOutcome.subgoals`。

### 5.3 continue-vs-stop 策略(纯函数)

```rust
pub enum MissionState {
    Running,                 // 还在 MAX_AUTO 循环内推进
    CompletedAllReady,       // 无更多就绪 milestone
    BlockedAt(u64),          // 某有依赖者的 milestone 失败,下游全 blocked
    CircuitBreaker,          // 连续 K 个 fail
    Error(String),           // turn/provider 自身错误(不动 workgraph)
}

/// 给定本次 milestone 结果 + 历史 consecutive_fail + workgraph 依赖,决定下一步。
pub fn next_action(verdict, milestone, consecutive_fail, graph) -> Action;
// Action: AdvanceTo(next_id) | Stop(MissionState)
```

默认参数(可 config,见 5.5):
- `MAX_AUTO = 3`(既有,每次 BG 调用最多推进 milestone 数)。
- `K = 2`(连续 fail 熔断阈值)。
- 独立失败可继续;硬依赖失败 → `BlockedAt`。

### 5.4 BgOutcome 扩展

```rust
#[derive(Debug, Default)]
pub struct SubgoalOutcome {
    pub milestone_id: u64,
    pub verdict: SubgoalVerdict,          // Pass | NeedsFix | Inconclusive
    pub gate_reason: String,
    pub tool_cap_hit: bool,
    pub touched_files: Vec<String>,
}

pub struct BgOutcome {
    pub final_text: String,               // [existing]
    pub tool_calls: Vec<String>,          // [existing]
    pub denied: Vec<String>,              // [existing]
    pub events: Vec<String>,              // [existing]
    pub subgoals: Vec<SubgoalOutcome>,    // [NEW]
    pub mission_state: MissionState,      // [NEW]
}
```

`main.rs`/`run_background` 末尾打印 `mission_state` + `subgoals` 摘要到 stderr(供外部调度器/日志解析)。

### 5.5 配置(env,`config.rs`)

| 变量 | 默认 | 说明 |
|---|---|---|
| `CODECODER_BG_MAX_AUTO` | 3 | 每次 BG 调用最多推进 milestone 数(覆盖既有 MAX_AUTO) |
| `CODECODER_BG_CIRCUIT_K` | 2 | 连续 fail 熔断阈值 |
| `CODECODER_BG_MILESTONE_TOOL_CAP` | 8 | 单 milestone turn 的工具迭代上限(< 全局 12);agent loop 对 BG turn 用此值 |

## 6. 数据流(一次 BG 调用)

```
CODECODER_BG_TASK="" (or "workgraph") + external timer fire
 → resolve_bg_task → milestone M (next_ready); M=in_progress
 → run_background_turn(task, tool_cap=BG_MILESTONE_TOOL_CAP)
     captures tool_calls, denied, tool_cap_hit(via AgentEvent::Notice "12-tool-iteration cap")
 → gate = bg_gate::evaluate(M, touched, turn_ctx, cancel)
       命令门? run_shell_cancellable(cmd) → exit0? Pass : NeedsFix
       else review(touched) → pass? Pass : NeedsFix
       兜底 → Inconclusive
 → match gate:
     Pass → M=done; subgoals.push(Pass); consecutive_fail=0
     NeedsFix/Inconclusive → M=needs_fix; reason.add node; subgoals.push(...); consecutive_fail+=1
 → next_action(gate, M, consecutive_fail, graph):
     Pass & ready & budget → next M (loop)
     Fail & M 有 dependents → Stop(BlockedAt(M.id))
     Fail & M 独立 & consecutive<K → next independent ready (loop)
     consecutive≥K → Stop(CircuitBreaker)
     no ready → Stop(CompletedAllReady / BlockedAt)
 → BgOutcome { subgoals, mission_state, ... } → stderr 摘要
```

## 7. 错误处理

- 门命令跑不起来(缺二进制 / timeout)→ `Inconclusive("gate command unavailable: …")`,**不谎报 pass**。
- review 工具出错 → `Inconclusive("review gate errored: …")`。
- turn 自身出错(provider 故障 / panic 恢复)→ `mission_state=Error(msg)`,**不动 workgraph 状态**(留 in_progress 待下次外部触发重试)+ reason 节点。
- acceptance 为空 → 跳过命令/review 门,用**弱信号**:turn 未触顶且 `denied` 为空 → `Pass(weak)`(标注 weak);否则 `NeedsFix(weak)`。弱信号在 subgoals.gate_reason 显式标注。
- 取消(SIGINT):门执行中的 `run_shell_cancellable` 尊重 CancelToken(既有机制),取消 → mission_state=Error("cancelled"),与 ADR 0026 一致。

## 8. 测试计划(hermetic,ScriptedProvider)

复用 `tests/` 既有黑盒分层(L1 默认)+ `src/background.rs` 内 `#[cfg(test)]` 单元测试。门用 `echo`/`false` 保持 hermetic(不依赖 cargo/Docker)。

- **T1 命令门 pass**:acceptance="`echo ok`",scripted turn 写文件 → 门跑 `echo ok` exit 0 → `Pass`;`M.status==done`;`subgoals[0].verdict==Pass`。
- **T2 命令门 fail**:acceptance="`false`",scripted turn 写文件 → 门 exit 1 → `NeedsFix`;`M.status==needs_fix`;causal_tree 多一节点;`subgoals[0].verdict==NeedsFix`;`gate_reason` 含 exit 信息。
- **T3 触顶 + review 门**:scripted turn 发 12-tool-cap Notice,无命令门,review(脚本注入)返 needs_fix → `NeedsFix` + `tool_cap_hit==true`。
- **T4a 阻塞**:M1 fail 且 M2 deps=[M1] → `mission_state==BlockedAt(1)`;M2 未被尝试。
- **T4b 独立可继续**:M1 fail 无 dependents,M2 独立 ready → 尝试 M2。
- **T5 熔断**:连续 2 个 fail → `mission_state==CircuitBreaker`。
- **T6 兜底**:acceptance 空 + review 出错 → `Inconclusive`;`gate_reason` 标注。
- **T7 覆盖自报**:agent 自报文本含 "pass",但命令门 fail → 最终 `NeedsFix`(证明客观门覆盖自评)。
- **T8 取消**:门执行中 SIGINT → `mission_state==Error("cancelled")`,无残留子进程。

纯函数(`extract_gate_command`、`next_action`)单测覆盖边界(空 acceptance、循环依赖、K 边界)。

## 9. 不在本范围内(YAGNI)

- **turn 内固着检测器**:验收门 + 有界 milestone scope 已足 v1;agent loop 不增"识别验证步"语义。
- **失败回滚 / 事务性 turn**:门抓住+记录;git 级 revert 属后续。
- **自动换策略重试**:下次外部 timer 触发自然重试 / 人介入;不在 BG 内做策略切换。
- **改交互式 agent loop**:仅 BG runner 策略。
- **改外部调度器**:调度仍外置(systemd/cron/launchd);本项只让每次 BG 调用 failure-safe。
- **margin×杠杆内核排序**:属 roadmap #4(reason 深化),本项仅在失败节点复用既有元数据。

## 10. 交付物

1. `src/bg_gate.rs`(或 background.rs 子节):验收门纯函数 + 命令/review 门。
2. `src/background.rs`:`BgOutcome` 扩 `subgoals`+`mission_state`;`run_background` milestone 循环接入门 + 失败写回 + continue-vs-stop。
3. `src/config.rs`:`CODECODER_BG_MAX_AUTO` / `CODECODER_BG_CIRCUIT_K` / `CODECODER_BG_MILESTONE_TOOL_CAP`。
4. `causal_tree.json` 失败节点写入(复用 reason 既有结构)。
5. 测试:T1–T8 + 纯函数边界单测。
6. (文档)`docs/adr/` 新增或扩展 ADR(0026 增补"客观验收门 + 失败写回 + 任务策略"),`ARCHITECTURE.md`/`README.md` 同步。
