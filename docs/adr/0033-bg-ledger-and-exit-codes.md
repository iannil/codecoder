# ADR 0033 — BG 任务账本与退出码告警

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0026(Background Agent)、ADR 0030(BG 客观验收门,提供 `BgOutcome.subgoals`/`mission_state`)、roadmap #2、`docs/background-agent-scheduling.md`

## 背景

#1(ADR 0030)给 BG 加了客观验收门与 `mission_state`,但每轮 BG 的结果**只在 stdout 打印一次就丢**——下次 BG 调用是全新进程,历史无从查起;且 `run_background` 恒返回 `Ok(())` → BG 永远 exit 0,外部调度器(systemd timer / cron)无法区分"完成"与"卡住/熔断/出错"。无人值守下"没人看"是核心痛点。

## 决策

1. **JSONL 账本**:每次 BG 调用追加一条记录到 `CODECODER_ROOT/bg_ledger.jsonl`,含 `ts`(epoch 秒)、`task`、`mission_state`、`blocked_at`、`subgoals`、`counts`(tools/denied/milestones/passed/failed)。append-only,不内置轮转(外部 logrotate,与 sessions/、logs/ 一致)。
2. **退出码告警**:`mission_state` → 进程退出码:`CompletedAllReady`/`Running`→0、`BlockedAt`→2、`CircuitBreaker`→3、`Error`→4(SIGINT 取消→0,操作者主动)。非 0 时 `std::process::exit(code)`。systemd `OnFailure=` / cron 邮件据此触发——**完全复用既有调度器语义,零新基础设施**。
3. **`cc ledger` 查询**:直读 `bg_ledger.jsonl`(**不经 daemon**——BG 独立于 daemon,运行时 daemon 常不在场)。支持 `--last N` / `--failed`(仅需关注)/ `--detail`。

## 后果

- **正面**:跨 BG 调用的可观测性(账本)+ 零基础设施告警(退出码)+ 独立于 daemon 的查询。数据源复用 #1 的 `BgOutcome`,无新观测面。
- **代价/约束**:写账本失败仅 stderr 警告(观测绝不拖垮任务);坏行容错(JSONL 逐行解析);退出码语义需操作者在调度配置里对接(OnFailure 等)。
- **不做**:主动通知(webhook/email/notifier trait 留后续);内置轮转;daemon 参与账本;跨 root 聚合。

## 修订(2026-07-22):workgraph-BG 入口 + Error(4) 可达

上限压测(P8,见 `docs/superpowers/audits/2026-07-22-codecoder-capability-matrix.md`)发现:经 `CODECODER_BG_TASK` 入口,BG 恒走显式 task 单 turn 分支 → mission_state 恒 `Running` → 永远 exit 0;退出码 2/3(workgraph 分支产生)不可达,4(Error)从不构造。修订:

- **入口**: 新增 `CODECODER_BG_WORKGRAPH=1`(显式 task 缺省时)→ `run_background` 传空 task → workgraph 逐里程碑分支(`main.rs::bg_mode_from_env` 路由)。`CODECODER_BG_TASK=<非空>` 仍走显式单 shot。`lib.rs` 早已把空 task 标 `"workgraph"`(原设计意图),下游 background.rs/bg_gate 全就绪,只差入口路由。
- **退出码全可达**: 经此入口,`BlockedAt(2)`/`CircuitBreaker(3)`/`CompletedAllReady(0)` 由 `bg_gate::next_action` 产出;`Error(4)` 由 `AgentLoop.last_error`(provider 错误,进程内字段——**刻意不用新 `AgentEvent` 变体**,以免 ripple daemon→client wire 的 socket.rs/proto.rs)在显式 + workgraph 两分支置位。0/2/3/4 现全部 live 可观测。
- **`cc ledger --failed`**(= 非 CompletedAllReady)语义随 `CompletedAllReady` 可达而回归正确(此前经显式 task 入口,每条 BG 都是 Running → 被全量误报为需关注)。
