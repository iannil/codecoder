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

## 修订(2026-07-23):needs_fix 假绿修复 + acceptance 命令门健壮化

Dogfooding(用 cc/ccd/BG_WORKGRAPH 从零建一个外部 RGA CRDT 协作编辑器项目)暴露两处缺陷:

- **`StuckNeedsFix(id)` 新终态(修"假绿 exit 0")**:一个 milestone 验收失败被置 `needs_fix` 后,`next_ready()` 只选 `Pending && deps_done`,故一个 **fresh 进程**发现唯一可动的里程碑是 `needs_fix`(无 pending-ready)时,`advance_one_milestone` 立即返回 `None` → 循环 `Ok(None)` 分支**无条件**置 `CompletedAllReady` → exit 0。结果:0 工具空跑却假报"全部完成",上层调度器/编排者误判成功。修法:`Ok(None)` 分支置态前读图,若存在 `needs_fix` 节点 → `MissionState::StuckNeedsFix(id)`(退出码 **2**,同 BlockedAt 语义:需人工/上层修复该里程碑并重置 `pending` 后再续跑),否则才 `CompletedAllReady`。回归测试 `background::tests::stuck_needs_fix_when_only_needs_fix_and_nothing_ready`。
- **acceptance 命令门跳过 prose(修"假 needs_fix / 假 pass")**:`bg_gate::extract_gate_command` 原样返回首个含命令关键字的行交 `sh -c` 执行。当 agent 用 `milestone add` 写的 acceptance 是自然语言(尤其 CJK),如 `cargo init --name coedit 创建二进制项目`,整行被执行 → `unexpected argument '创建二进制项目'` → 假 `needs_fix`;或 `cargo test 通过` → 退化成过滤 "通过" 的空测试 → exit 0 假 pass。修法:仅当匹配行**为纯 ASCII 命令**时才作命令门,否则跳过该行(继续找干净命令行),找不到则返回 `None` → 交注入式 review 门。测试 `bg_gate::tests::extract_gate_command_skips_prose_acceptance_with_command_word`。
- **配套约束**:milestone 的 acceptance 最好写**独占一行的裸命令**(`cargo test`);描述性 prose 会退到 review 门(较弱信号)。全套 227 lib + 行为测试通过,无回归。

## 修订(2026-07-24,迭代 1):needs_fix 自恢复循环收窄 StuckNeedsFix 语义

上一修订的 `StuckNeedsFix(2)` 语义是"存在 needs_fix 即卡住,需人工重置 pending"。迭代 1 给 runner 补了**有界自恢复**(详见 ADR 0026 同日修订),本 ADR 关联的退出码语义随之收窄:

- **`StuckNeedsFix(id)`(退出码 2)仅在重试预算耗尽时落**:里程碑验收 `needs_fix` 后,runner 先在预算内(`CODECODER_BG_MAX_FIX_ATTEMPTS`,默认 3,0=禁用)经 `retry_one_milestone` 把 `Milestone.last_failure`(gate 失败原因)注入修复 prompt 自动重试;`WorkGraph::next_retryable` 只选 `fix_attempts < max` 的 needs_fix 节点。既无就绪又无预算内可重试节点时才置 `StuckNeedsFix` → exit 2(此时才需人工/上层介入)。
- **重试不计入 `max_auto`**:重试的成败由 per-node `fix_attempts` 预算约束,不占推进配额、不累加 `consecutive_fail`、不走 `next_action`。`fix_attempts`/`last_failure` 持久化在 `workgraph.json`,跨进程(fresh BG 调用)尊重预算。
- **已知取舍**:启用自恢复时,`CircuitBreaker(3)` 的跨里程碑连败熔断实际被 per-node 重试预算 + `max_auto` 取代(重试路径不累加 `consecutive_fail`);未来可让耗尽预算的硬失败里程碑累加独立计数以恢复该熔断。
