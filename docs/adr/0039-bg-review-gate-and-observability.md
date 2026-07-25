# ADR 0039 — BG Review 门(独立评审)与 headless 可观测性

- **状态**: Accepted
- **日期**: 2026-07-25
- **关联**: ADR 0026(headless runner)、ADR 0030(BG 客观验收门)、spec `docs/superpowers/specs/2026-07-25-codecoder-report-fixes-design.md`

## 背景

BG 的 Review-kind 里程碑此前只解析 agent 自报的 `VERDICT:`(`background.rs` review_runner),主观且易被乐观自评通过;且 headless 运行期事件只在 turn 结束后浮现,无法实时观察。(注:本 ADR 编号取下一空闲号——0037/0038 已占用,故为 0039;规划稿中曾称 0037。)

## 决策

1. **独立评审门**:Review-kind 里程碑改由独立只读评审子 agent(`AgentLoop::run_review`,复用 `review.rs` 漂移评分)客观判定,覆盖 agent 自报 VERDICT;取消时降级回自报解析。
2. **实时可观测**:`BgObserver`(`src/bg_observer.rs`)把 BG 事件同时写 stderr 与 `<root>/.ccd.bg.ndjson`(每行一 JSON,一次 truncate 开轮 + 逐事件 append 累积整条流);milestone turn 改为工作线程运行、主线程 live-drain 事件。
3. **默认预算**:`bg_max_auto` 默认 3→10(熔断 `bg_circuit_k` 仍兜底)。

## 后果

- 正面:验收更客观;运行可 tail 观察;更大项目默认可跑完。
- 代价:每个 Review 里程碑多一次 LLM 子调用(Command 门不受影响);多一线程与一 NDJSON 产物(已 gitignore)。
- 不做:cc-web 写操作、真实测试热力图、BG 空图自动播种(仍守 ADR 0033/0036)。
