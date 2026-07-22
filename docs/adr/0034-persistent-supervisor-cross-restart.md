# ADR 0034 — Persistent Capability 跨重启韧性

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0021(Capability 环境与生命周期——不自动重启)、ADR 0022(自撰安全回路)、roadmap #3、`docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`

## 背景

`Supervisor`(daemon 启动时扫描 `capabilities/` 起 Persistent+Shell 服务并监督健康)的判定状态——`gave_up`/Failed——**进程内内存**,daemon 重启即丢。重启后 `start_all` **盲目重 spawn 全部声明服务**,包括上次已 `gave_up` 的;若该服务仍崩溃,每次重启都 crash-loop,且"Failed, agent 决定"的信号丢失。审计核验「Persistent 无跨重启注册表」属实。

## 决策

持久化 `Supervisor` 的**判定状态**到 `<root>/supervisor_state.json`(每服务:`gave_up` / `crash_count` / `manifest_mtime_secs`),daemon 重启后 `start_all` 据此:

1. **manifest 变更自动重置**:记录的 mtime ≠ 当前 manifest mtime → 清 `gave_up`/`crash_count`(agent 重新生成 capability → 值得重试)。
2. **崩溃预算**:超 `CODECODER_SUPERVISOR_CRASH_BUDGET`(默认 3)或 `gave_up` 的服务在 start_all 被**跳过**(不 spawn),事件可见。
3. **会话内不变(ADR 0021)**:崩溃仍 1 次即 `gave_up`、不自动重启。预算只管"重启后是否再 spawn",不改会话内语义。

`RunningServiceTable` 的 live handles(PID/容器)跨重启无意义,**不持久化**;只持久化判定状态。state 文件缺失/损坏 → 默认空(不阻塞启动);save 失败仅警告。

## 两层 "gave_up" 区分

| 层 | 语义 | 行为 |
|---|---|---|
| 会话内 gave_up(ADR 0021) | 本次 daemon 内崩过 → 不再重启 | 1 次崩溃 → gave_up(本会话) |
| 跨重启永久跳过(本 ADR) | 历史崩溃累计 ≥ 预算 → 重启后不 spawn | start_all 跳过,直到 manifest 变更 |

## 后果

- **正面**:跨重启记忆抑制 crash-loop;失败信号持久可见;manifest 变更驱动自动重试(贴合自我进化)。
- **不改 ADR 0021**:会话内仍不自动重启(预算只管"重启后是否再 spawn")。
- **约束**:budget=0 = 永不永久跳过(每次重启都试,仅 gave_up 生效);不持久化 live handles(PID 跨重启无效);无 `cc services --reset` 命令(manifest 变更即重置)。
