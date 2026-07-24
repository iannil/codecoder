# ADR 0030 — BG 客观验收门 + 失败写回 + 任务策略

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0026(Background Agent)、ADR 0021(Capability 验收)、`docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`(审计 #1)、`docs/superpowers/specs/2026-07-22-bg-failure-handling-guardrails-design.md`

## 背景

2026-07-21 能力审计暴露 BG 的核心自主性瓶颈:headless turn 遇失败测试会**固着**(17 工具全修一个测试)、**回退代码**(mdslides crate 从 8/9 测试掉到 0),而 `advance_one_milestone` 仅凭 agent **自报**的 `VERDICT:` 行写回状态——回退后的代码仍可能被自评为 pass。无人值守下这等于"把损坏当成功提交"。

## 决策

在 `run_background`/`advance_one_milestone` 既有的 milestone 循环上加一层**客观验收门**,verdict **覆盖** agent 自报:

1. **命令门优先**:milestone.acceptance 含可识别命令模式(`cargo test`/`pytest`/`make`/`rustc`…)时,BG runner 直接 `run_shell_cancellable` 跑它,按 **exit 0** 客观判定(尊重 CancelToken)。零 LLM、确定性。
2. **review 门兜底(v1)**:无命令模式时,复用 agent 自产 `VERDICT:` 文本(`parse_review`)。真正的 review 子代理门为后续增强。
3. **Inconclusive 兜底**:acceptance 空或门自身出错 → Inconclusive,**绝不谎报 pass**。

门 fail/inconclusive → milestone 标 `needs_fix` + 写一个 reason 因果节点(根因不丢)+ 记 `SubgoalOutcome`。`run_background` 用纯函数 `next_action` 决定 continue/stop:

- Pass & 预算 & 就绪 → 推进下一个。
- 失败 & 有阻塞下游 & 无独立就绪 → `BlockedAt`。
- 连续 K(默认 2)个 fail → `CircuitBreaker`(防连环 flail)。
- 无更多就绪 → `CompletedAllReady`。

caps 经 `CODECODER_BG_MAX_AUTO`(3)/`_CIRCUIT_K`(2)/`_MILESTONE_TOOL_CAP`(8)配置;单 milestone 工具预算收紧(防固着耗尽全局 12)。

## 后果

- **正面**:回退/固着被客观门抓住并记录,绝不当作 done;失败有因果节点可追溯;任务在硬依赖断裂或连环失败时干净停止而非 flail。审计 #1 根治。
- **负面/代价**:无命令模式 acceptance 的 milestone 走 v1 review 兜底(仍是 LLM 判断,非完全客观)——真正 review 子代理门待后续;门命令需在 BG 运行环境可用(缺二进制 → Inconclusive)。
- **不变**:调度仍外置(ADR 0026);agent loop 保持通用(失败策略全在 background.rs + bg_gate.rs);交互式 session 不受影响。

## 修订（2026-07-24，迭代 3：acceptance 契约化）

- Milestone 新增可选结构化 `command`（客观验收命令，`workgraph.rs` 的 `Milestone.command: Option<String>`）。`bg_gate::gate_command(m)` = 显式 `command` 优先、旧数据 `extract_gate_command(acceptance)` 兜底；`gate_kind(m) -> GateKind{Command|Review|None}` 为单一路由事实源，`evaluate` 与 `run_milestone_and_gate` 共用。
- `SubgoalOutcome.gate_kind` 随 `bg_ledger.jsonl` 序列化 → 编排者可过滤「弱信号（Review/None）通过」的里程碑；旧记录 `#[serde(default)]` 落 `None`。
- `milestone add` 接受 `command`；缺强门（既无显式 `command` 也无可提取命令）且 acceptance 非空时提示建议传 command（只提示，不拒绝）。
- 行为对无 `command` 的既有里程碑等价（退回原 extract 启发式）。
