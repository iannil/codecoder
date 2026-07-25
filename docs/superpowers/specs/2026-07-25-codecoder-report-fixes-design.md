# 设计：修复 experiment-report 中的真实问题

> 日期: 2026-07-25
> 来源: `docs/proof/experiment-report.md` 的问题清单（经核实后的三分类）
> 状态: 已批准，待写实现计划

## 一、背景与问题三分类

`experiment-report.md` 列了一批 codecoder"缺陷"。逐条核实源码后，分为三类：

### ❌ 报告误诊（并非缺陷）

| 报告条目 | 核实结论 |
|---|---|
| #1「Review gate 未实现 / `bg_gate.rs:295` panic」 | Review 门**已实现**（`bg_gate.rs:127` → `review_runner`）。第 295 行是**单测断言**里的 panic。真实局限较小：`background.rs:389` 的 review_runner 只解析 agent **自报**的 `VERDICT:`，解析不出返回 `Inconclusive`（"review gate deferred in v1"）。是**弱**，不是**没有**。 |
| #2「复合命令 keying 太严」 | `npm install 2>&1` 因含 `>` 被判复合、按整串 keying。这是 **ADR 0036** 的刻意安全加固（堵"良性前缀→任意后缀"提权洞）。 |
| #3「BG_WORKGRAPH 不能自动建里程碑」 | 按 ADR 0033/CLAUDE.md 是**刻意设计**，空图 → exit 0。 |
| #5/#6「.ccd.env 不注入 API_KEY」 | 刻意安全设计（`config.rs:110`）：密钥/端点/trust 绝不从不可信 repo 文件注入。 |

### ✅ 真实可修（本设计范围）

| 编号 | 真实问题 | 位置 |
|---|---|---|
| B | `bg_max_auto` 默认 3 偏小 | `config.rs`（默认值 + 单测断言） |
| A | Review 门只靠 agent 自报，缺客观校验 | `background.rs::run_milestone_and_gate` 的 `review_runner` |
| C | headless BG 进展在内存累积、只在 turn 结束后浮现，运行期不可实时观察 | `background.rs::drain_bg_events` |
| cc-web | 只读面板"对用户完全没帮助"：空闲首屏空、blocked 不渲染、session 回放是裸 JSON、非活动 tab 显占位符 | `static/index.html`、`src/visual/http_server.rs` |

### ⛔ 明确不做（"重审刻意项"的结论）

- #2 ADR 0036 复合命令 keying — **保留**（放宽即重开提权洞）。
- #3 BG 空图不自动播种 — **保留**（ADR 0033）。
- #5 `.ccd.env` 不注入密钥 — **保留**（安全边界）。
- §4.2（TS 编译错误、`matchMedia` mock、页面覆盖率）— 是被生成 app 的**模型产出质量**，非 codecoder 机制缺陷 → **不在本仓库范围**。

## 二、cc-web 实测诊断（证据）

在临时项目起 daemon + cc-web，逐端点探测所得：

| 编号 | 缺陷 | 证据 | 严重度 |
|---|---|---|---|
| #1 | 空闲即"死屏"：默认落地页是 timeline，但 daemon 空闲时 SSE `/api/v1/events` 零输出（无 catch-up/心跳），首屏永远"⏳ 等待实时事件…" | `curl -N /api/v1/events` 2s 空输出 | 🔴 高 |
| #2 | 纯旁观：无写路由（`http_server.rs` 仅 GET） | — | 🔴 高（本轮不做，见下） |
| #3 | `blocked` 状态不渲染：Workgraph tab 仅 in_progress/needs_fix/done/pending 四组 | `index.html:268-273` vs `NodeStatus::Blocked`(snake_case `blocked`) | 🟡 中 |
| #4 | 测试热力图是假的：`/api/v1/tests` 只返回测试名，正文写死"Run cargo test to populate" | `index.html:409-415` | 🟡 中（本轮不做） |
| #5 | Session 回放 = 裸 `JSON.stringify` 塞 `<pre>` | `index.html:363-372` | 🟡 中 |
| #6 | 非活动 tab 仅点击时 fetch，未点过一直是"— Phase 2/3/4"占位文字 | `index.html:95-97` + `:332-340` | 🟢 低 |

> 澄清：曾疑 Workgraph 渲染有大小写契约 bug，核实后 `NodeStatus` 序列化为 **snake_case**，前端过滤正确，**不是 bug**（唯缺 `blocked` 组）。

本轮 cc-web 目标 = **"只读但真能看"**：修 #1/#3/#5/#6；**不新增任何写路由**（#2 完整交互控制台、#4 真实热力图留待后续）。

## 三、各工作线设计

### B — bg_max_auto 默认值

- `src/config.rs`：默认 `3 → 10`。熔断 `bg_circuit_k` 独立存在，连续失败仍会熔断兜底，故调大安全。
- 更新对应单测：默认断言 `c.bg_max_auto == 10`（env 覆盖测试不变）。
- 同步文档中"默认 3"的描述（README/CLAUDE.md）。

### C — headless BG 实时可观测（stderr + NDJSON）

**目标**：BG 运行期间可通过 tail 实时看到工具调用、门结论、里程碑状态、mission_state，而非只在结束后。

- 新增轻量 `BgObserver`（建议置于 `src/background.rs` 或 `src/bg_observer.rs`）：
  - 输入：一条 `AgentEvent` 或一条里程碑级结构化进展。
  - 输出①：格式化写 `stderr`（人读，复用现 `eprintln` 风格）。
  - 输出②：追加一行 JSON 到 `<root>/.ccd.bg.ndjson`（机读，每行一事件；文件可被 cc-web/工具 tail）。
- 接入点：`drain_bg_events`（现为 turn 后一次性 drain）改为**边收边发**——从 `rx` 每收到一个事件即经 observer 输出，再累积进 `BgOutcome`。语义不变（`BgOutcome` 内容一致），只多实时旁路。
- 里程碑循环（`run_background_cfg` / `run_milestone_and_gate`）在关键节点（里程碑开始、门结论、状态写回、mission_state 终态）也经 observer 发一条，保证 tail 能看到进度骨架。
- 数据流：`AgentLoop → event_tx → rx →（BgObserver: stderr + .ccd.bg.ndjson）→ BgOutcome`。
- NDJSON 文件在每次 BG 运行开始时截断（truncate）或追加分隔——实现时选"截断新起"，避免跨运行混淆。

### A — review gate 复用独立评审

**目标**：`gate_kind == Review` 的里程碑走**独立**只读评审客观判定，覆盖 agent 自报。

- `src/agent.rs`：把现有评审用法（`spawn_sub_agent_text(review::review_task(target))` + `review::parse_review`，见 `agent.rs:1139`）封成公开薄封装：
  ```
  pub fn run_review(&mut self, target: <path 或当前改动描述>, tx: &Sender<AgentEvent>) -> ReviewOutcome
  ```
- `src/background.rs::run_milestone_and_gate`：`review_runner` 闭包改为——**新建一个 background `AgentLoop` 跑 `run_review`**（target = 里程碑 acceptance 描述 + root 当前改动），把返回的 `Verdict` 映射为 `GateVerdict`：
  - `Verdict::Pass → GateVerdict::Pass`
  - `Verdict::NeedsFix | Rebuild → GateVerdict::NeedsFix(原因)`
  - 评审不可用/被取消/子调用报错 → **降级回**现有自报解析逻辑（保底不回归）。
- `bg_gate::evaluate` 的注入式 `review_runner` 签名**不变** → 现有纯策略单测不受影响。
- 成本：每个 Review 里程碑多一次 LLM 子调用；Command 门里程碑不受影响（客观命令，零额外成本）。

### cc-web — 只读但真能看

均为 `static/index.html` 前端改动；`http_server.rs` 至多为 session 详情返回结构微调；**不新增 POST/写路由**。

- **#1 空闲有内容 + 落地页改 Workgraph**：默认激活 tab 从 `timeline` 改为 `workgraph`；页面加载即调 `loadWorkgraph()`（不等点击）。timeline 保留现有空闲 hint。
- **#3 blocked 组**：`renderWorkgraph` 增加 `blocked` 分组（如紫色 `#a371f7`），置于 needs_fix 之后，避免 blocked 节点消失。
- **#5 Session 回放可读**：`loadSessionDetail` 从 `JSON.stringify` 裸 dump 改为按消息渲染（role 标签 + 文本 + 工具调用摘要）。若 `http_server.rs` 的 `/api/v1/sessions/{id}` 返回结构不便渲染，做最小整形。
- **#6 tab 预加载**：首屏预取落地页（workgraph）数据；其余 tab 保持点击加载（避免无谓请求）。

## 四、测试策略

- **B**：改默认断言 `bg_max_auto == 10`。
- **C**：`BgObserver` hermetic 单测——给定事件序列 → 断言 `.ccd.bg.ndjson` 行内容与 stderr 格式；无需 LLM。
- **A**：`run_review` 用 `StubClient` 单测（stub 返回可解析评审文本 → 断言 `GateVerdict` 映射与降级路径）；`bg_gate::evaluate` 现有注入测试不动。
- **cc-web**：`http_server.rs` 现有 SSE/路由测试保持通过；前端改动以**实跑冒烟**验证（起 daemon+cc-web，curl 校验落地页数据、seed 一个 blocked 节点校验渲染）。无前端测试框架，故前端以手测为准。
- 全量 `cargo test` 必须通过（现 336 通过 + 3 ignore）。

## 五、文档更新

- 修正 `docs/proof/experiment-report.md`：#1 由"未实现/panic"改为"仅自报、本轮已升级为独立评审"；标注 #2/#3/#5 为 working-as-designed。
- `README.md` / `CLAUDE.md`：`bg_max_auto` 默认 10、C 的可观测性（`.ccd.bg.ndjson` + stderr）说明。
- `ARCHITECTURE.md`：cc-web 只读增强能力。
- 新增一条 **ADR**：BG Review 门从"agent 自报"升级为"独立评审客观判定，自报降级兜底"；C 的实时可观测旁路可并入同 ADR。

## 六、风险与安全边界

- **A 成本/延迟**：每个 Review 里程碑多一次子调用；Command 门不受影响。降级路径保证评审失败不阻断。
- **C 文件写入**：`.ccd.bg.ndjson` 在 root 下，与 `.ccd.sock`/`supervisor_state.json` 同级；需确保 gitignore 覆盖（临时产物）。
- **B**：熔断 `bg_circuit_k` 仍是失败兜底，调大 max_auto 不影响失败保护。
- **cc-web 只读**：本轮严格不引入写路由，杜绝从 web 触发副作用的攻击面（#2 留待后续单独设计权限模型）。
- **不碰安全边界**：ADR 0036 keying、`.ccd.env` 密钥过滤、BG 空图不播种——全部保留原样。
