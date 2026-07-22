# BG 任务可观测性 / 账本 — 设计文档

- **日期**: 2026-07-22
- **状态**: 已批准(Approved)
- **分支**: `feat/bg-ledger`
- **作者**: Claude Code(brainstorming 产物)
- **关联**: roadmap #2、ADR 0026(Background Agent)、ADR 0030(BG 客观验收门,提供 `BgOutcome.subgoals`/`mission_state`)、`docs/superpowers/specs/2026-07-22-bg-failure-handling-guardrails-design.md`、`docs/background-agent-scheduling.md`

## 1. 目标

让长期无人值守 BG **可观测**:跨多次 BG 调用(每次都是独立进程、跑完即退)持久化记录每轮做了什么;**可查**(`cc ledger`)、**可告警**(`mission_state`→进程退出码,外部调度器据此动作)。零新基础设施。

**成功标准**:
- 每次 BG 调用追加一条 JSONL 记录到 `CODECODER_ROOT/bg_ledger.jsonl`,含 timestamp、mission_state、每 milestone 的 subgoal 结论、聚合 counts。
- `mission_state` 映射到 BG 进程退出码(0/2/3/4);外部调度器(systemd OnFailure / cron)据此告警。
- `cc ledger [--last N] [--failed] [--detail]` 直读该文件(不经 daemon)。
- 全部逻辑 hermetic 可测(ScriptedProvider + 纯函数)。

## 2. 背景

#1 刚给 `BgOutcome` 加了 `subgoals: Vec<SubgoalOutcome>` 与 `mission_state: MissionState`(ADR 0030),这正是账本的数据源,但目前**只在 BG 进程的 stdout 打印一次就丢了**——下次 BG 调用是新进程,历史无从查起;且 `lib.rs::run_background` 恒返回 `Ok(())` → BG 永远 exit 0,外部调度器无法区分"完成"与"卡住"。

## 3. 已锁定决策(brainstorming)

| 维度 | 决策 |
|---|---|
| 告警机制 | **退出码映射**(零新基础设施,复用外部调度器语义) |
| 查询接口 | **`cc ledger` 直读文件**(不经 daemon;BG 跑时 daemon 常不在场) |
| 保留/轮转 | **append-only,不内置轮转**(`--last N` + 外部 logrotate) |

## 4. 架构

三处改动,全部对齐既有结构:

```
lib.rs::run_background(cfg, task)
  → background::run_background(...)            [existing;返回带 subgoals/mission_state 的 BgOutcome]
  → ★ bg_ledger::append(root, &outcome)        [NEW;追加一条 JSONL,失败仅 stderr 警告]
  → ★ 打印账本摘要行(一行,key=value,供日志解析)
  → ★ std::process::exit(mission_exit_code(&outcome.mission_state))   [NEW;0 正常返回,非 0 显式退出]

cc.rs
  → ★ "ledger" 子命令分支:读 bg_ledger.jsonl + pretty-print   [NEW;不经 daemon]
```

**关键不变量**:账本写/读**都不经 daemon**——BG 独立于 daemon(ADR 0026),daemon 在 BG 运行时常不在场。

## 5. 组件

### 5.1 新模块 `src/bg_ledger.rs`(纯函数 + 文件 IO)

```rust
/// 一条账本记录(序列化为 JSONL 一行)。
#[derive(Serialize, Deserialize)]
pub struct LedgerRecord {
    pub ts: String,                          // SystemTime::now() → UTC ISO8601
    pub task: String,                        // "workgraph" | "<explicit task 摘要>" | "no task"
    pub mission_state: MissionState,         // 复用 bg_gate::MissionState(Clone/Serialize 需 derive)
    pub blocked_at: Option<u64>,             // 若 mission_state == BlockedAt(id)
    pub subgoals: Vec<SubgoalOutcome>,       // 复用 background::SubgoalOutcome
    pub counts: LedgerCounts,
}

#[derive(Serialize, Deserialize, Default)]
pub struct LedgerCounts {
    pub tools: usize, pub denied: usize, pub milestones: usize, pub passed: usize, pub failed: usize,
}

pub fn ledger_path(root: &Path) -> PathBuf { root.join("bg_ledger.jsonl") }

/// 追加一条记录。IO 失败仅返回 Err(调用方记 stderr 警告,不影响主流程)。
pub fn append(root: &Path, outcome: &BgOutcome, task: &str) -> anyhow::Result<()>;

/// 读最近 n 条(按文件顺序的最后 n);only_failed=true 只回 mission_state≠CompletedAllReady 的。
/// 损坏行跳过(JSONL 容错)。
pub fn read_recent(root: &Path, n: usize, only_failed: bool) -> Vec<LedgerRecord>;

/// mission_state → 进程退出码。
pub fn mission_exit_code(state: &MissionState) -> i32;
```

`MissionState` 需 `Serialize/Deserialize`(给 bg_gate::MissionState 加 derive;`Error(String)` 变体可序列化)。`SubgoalOutcome` 已 `Clone`;需 `Serialize/Deserialize`(加 derive)。

### 5.2 退出码映射

| mission_state | exit code | 含义 |
|---|---|---|
| `CompletedAllReady` / `Running`(无 milestone) | **0** | 无需关注 |
| `BlockedAt(_)` | **2** | 硬依赖断了,任务卡住 |
| `CircuitBreaker` | **3** | 连环失败熔断 |
| `Error(_)` | **4** | turn/provider 自身错 |
| SIGINT 取消(既有路径) | **0** | 操作者主动取消,非故障 |

`lib.rs::run_background` 末尾:计算 `code = mission_exit_code(&outcome.mission_state)`;若 `code == 0` 正常 `Ok(())`,否则 `std::process::exit(code)`。保守默认:未知 state → 0(不误报)。

### 5.3 `cc ledger` 查询(cc.rs 加分支)

```
cc ledger                 # 最近 10 次(单行摘要)
cc ledger --last 50       # 最近 N 次
cc ledger --failed        # 仅需关注(mission_state≠CompletedAllReady)
cc ledger --detail        # 最近一次的完整 subgoals 明细
```
单行摘要:`<ts>  <mission_state>(#<blocked_at>)  <milestones>m(<passed>✓ <failed>✗)  <tools>t  <denied>d`。
实现:cc.rs `match args` 加 `["ledger", flags...]` → 调 `bg_ledger::read_recent(CODECODER_ROOT, n, only_failed)` → pretty-print。**不连 daemon**(与 `cc sessions` 经 daemon 不同;ledger 是纯文件读)。`CODECODER_ROOT` 从 `Config::from_env()` 取。

## 6. 数据流

```
外部 timer 触发 CODECODER_BG_TASK=... codecoder
 → lib.rs::run_background
 → background::run_background → BgOutcome{ subgoals, mission_state, tool_calls, denied, ... }
 → bg_ledger::append(root, &outcome, task_label)   # 写 bg_ledger.jsonl 一行
 → 打印 "ledger: state=X milestones=M tools=T denied=D"   # 供 grep
 → exit(mission_exit_code(&outcome.mission_state))   # 0/2/3/4
 → 外部调度器按退出码动作(OnFailure / cron 邮件)

操作者查账:
 cc ledger --failed    # 直读 bg_ledger.jsonl,只看需关注的
```

## 7. 错误处理

- **写账本失败**(IO 错)→ `append` 返 Err;`run_background` 记 stderr `"bg ledger append failed: …"`,**继续退出流程**(账本是观测,绝不拖垮任务)。
- **读账本**:文件不存在 → `cc ledger` 打印 `(no bg_ledger.jsonl yet)`;损坏行 → 逐行解析,坏行跳过并在末尾打印 `(<K> malformed line(s) skipped)`。
- **`mission_exit_code`** 对未知/未来 state → 0(保守不误报)。
- **退出码非 0 时**:仍先把 stdout/stderr 摘要打全,再 `exit(code)`(调度器能从日志看到细节)。

## 8. 测试(hermetic)

复用既有 ScriptedProvider + Workspace testkit + tempdir。

- **T1 append→read 往返**:`append(root, &outcome, "workgraph")` 写一条,`read_recent(root, 10, false)` 读回,字段匹配(ts 非空、mission_state、subgoals、counts)。
- **T2 最近 N 顺序**:写 3 条,`read_recent(root, 2, false)` 返回最后 2 条(时间顺序)。
- **T3 only_failed 过滤**:写一条 CompletedAllReady + 一条 BlockedAt,`read_recent(root, 10, true)` 只回 BlockedAt。
- **T4 mission_exit_code 全枚举**:CompletedAllReady→0、BlockedAt→2、CircuitBreaker→3、Error→4、Running→0。
- **T5 损坏行容错**:文件含 1 合法 + 1 坏行,`read_recent` 返回 1 条 + 不 panic。
- **T6 集成**:lib.rs run_background(空 task + 一个 fail milestone + ScriptedProvider)→ `bg_ledger.jsonl` 多一行,mission_state=BlockedAt;`mission_exit_code`=2。(退出码本身不便在进程内断言,断言函数返回值即可。)
- 纯函数 + testkit;不烧 LLM、不依赖 daemon/Docker。

## 9. 不做(YAGNI)

- ❌ 主动通知(webhook/email/notifier)——退出码已覆盖"可告警";notifier trait 留后续。
- ❌ 内置轮转/压缩——外部 logrotate(与 sessions/、logs/ 一致)。
- ❌ daemon 参与账本——BG 独立于 daemon。
- ❌ 账本写入影响主流程——观测不能拖垮任务。
- ❌ 跨 root 聚合——一个 root 一个账本文件。
- ❌ 实时流式推送——`cc ledger` 轮询足够。

## 10. 交付物

1. `src/bg_ledger.rs`:`LedgerRecord`/`LedgerCounts` + `append`/`read_recent`/`mission_exit_code`/`ledger_path`。
2. `src/bg_gate.rs`:`MissionState` 加 `Serialize/Deserialize` derive。
3. `src/background.rs`:`SubgoalOutcome` 加 `Serialize/Deserialize` derive。
4. `src/lib.rs`:`run_background` 末尾接 `append` + `exit(mission_exit_code(...))`。
5. `src/bin/cc.rs`:`ledger` 子命令分支。
6. 测试 T1-T6。
7. 文档:ADR 0033(BG 账本与退出码告警;0030=gate,0031=middleware-no,0032=client-server 均已占)+ `ARCHITECTURE.md`(模块图 bg_ledger + bg_ledger.jsonl 文件)+ `README.md`(cc ledger 命令 + 退出码表)+ `docs/background-agent-scheduling.md`(退出码告警用法)。
