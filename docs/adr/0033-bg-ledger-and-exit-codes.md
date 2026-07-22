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
