# 设计 · CodeCoder 成熟度冲刺 90%+ 路线图

- **日期**: 2026-07-23
- **类型**: 多迭代开发路线图（spec）
- **来源**: `docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`（coedit dogfooding 评价报告）
- **关联 ADR**: 0026(Background Agent)、0029(turn steering)、0033(退出码/账本)、0034(Persistent supervisor 跨重启)、0035(workgraph 并发写保护)、0036(复合命令 keying)、0037(输出截断护栏)

---

## 1. 目标与基线

把评价报告的 5 个维度全部抬到 **≥90%** 成熟度。经确认的三条基线约束：

1. **成熟度基线 = 弱模型**。codecoder 必须在 `deepseek-v4-flash` 一类弱模型下也达到 90%——即真正逼近「无人值守」，而非依赖强模型兜底。
2. **验收 = 分层行为测试为主 + 定期 dogfooding 回归**。每迭代先补 L1（StubClient/确定性）行为测试锁机制，能仿真的补 L2；把「弱模型 headless 从零建外部项目并无人干预跑完」固化为 L3 门控作最终客观证据。
3. **本 spec 边界 = 路线图 + 首迭代实现计划**。本文件是覆盖全部迭代的总路线图；迭代 1（needs_fix 自恢复循环）随后由 writing-plans 细化为可执行实现计划，迭代 2–5 后续各自单独成计划。

### 维度现状与目标

| 维度 | 现状 | 目标 | 缺口 |
|------|------|------|------|
| 架构/设计 | 90% | ≥90% | 保持不回退 |
| 客观验收门(护栏) | 95% | ≥90% | 保持不回退 |
| 单能力正确性 | 85% | ≥90% | +5（打磨） |
| 自主执行(headless) | 60% | ≥90% | +30（大） |
| 健壮性/打磨 | 55% | ≥90% | +35（最大） |

---

## 2. 迭代排序策略

采用**杠杆优先**（余量×杠杆，最高性价比先出）。备选「维度优先」因单迭代过大、异质、跨迭代依赖交叉被否；「地基优先」的依赖洞察（截断根治部分制约自恢复价值）被吸收——把截断根治紧跟自恢复排在迭代 2，且因 ADR 0037 已能*检测*截断，迭代 1 不被迭代 2 硬阻塞。

### 维度 → 迭代映射

| 迭代 | 主攻缺口 | 直接抬升维度 | 报告依据 |
|---|---|---|---|
| **1. needs_fix 自恢复循环** | 失败不能自恢复 | 自主执行 60→~80 | §7 P0、§3 |
| **2. 截断根治 + 小步写引导** | max_tokens 4096 截断（头号杀手） | 健壮性 55→~72、自主执行 →~85 | §7 P1、§5 附、§6.5 |
| **3. acceptance 契约化** | agent 过不了自己的 gate | 健壮性 →~82、护栏保持 95 | §7 P1、§5 Issue A |
| **4. 探索兜底 + footgun 清零** | 整轮只探索、8 条 footgun | 自主执行 →90、健壮性 →90 | §6 全部、§4.2 |
| **5. dogfooding 转 L3 门控 + 正确性收尾** | headless 路径未经常态捶打 | 单能力正确性 85→90、全维度重测 | §7 P2、§4.3 |

---

## 3. 迭代详细设计

### 迭代 1 — needs_fix 自恢复循环（P0，自主执行核心）

**目标**：gate 判 needs_fix 后，runner 自动把失败原因喂回 agent、在预算内重试，而非停摆等人手动 reset pending。这是「监督式 → 无人值守」最关键一步。

**现状锚点**：`advance_one_milestone`（`src/background.rs:243`）每次只跑一个就绪里程碑一个 turn；无就绪 → `Ok(None)`。needs_fix 里程碑不会被重跑。客观 gate（`src/bg_gate.rs`）已覆盖 agent 自报 VERDICT。`src/retry.rs` 有纯分类器（transient vs 不可重试）但仅用于 provider error，未用于 gate 失败。`src/bg_ledger.rs` 可承载每里程碑重试计数。

**改动点**：
1. **失败原因捕获**：gate 判 `needs_fix`/`rebuild` 时结构化提取失败证据（命令门 → stderr/退出码尾部；review 门 → verdict reason），存入 `BgOutcome` 与账本，而非丢弃。
2. **重试预算（持久化）**：`bg_ledger.rs` 每里程碑记 `fix_attempts` / `last_failure`；新增 config `CODECODER_BG_MAX_FIX_ATTEMPTS`（默认 3）。跨进程读取、尊重已耗预算（呼应 ADR 0034 持久化精神）。
3. **自恢复注入**：needs_fix 且未超预算时，在 `next_ready` 之外新增「可重试 needs_fix」选取路径——构造带失败原因的修复 prompt（复用 `retry.rs` 分类器思路判断失败是否值得重试：命令门失败=值得；provider 非重试类=不值得），跑新 turn，退避 backoff。
4. **退出码语义**：仅当预算耗尽仍 needs_fix → 保持 `StuckNeedsFix(id)` exit 2（Bug B 修复不回退）；重试中途成功 → 正常推进。区分「真卡死」vs「重试中」。

**L1 验收测试**：
- `needs_fix_auto_retries_until_budget`：StubClient 头 2 turn 假绿被 gate 拦、第 3 turn 真过 → 里程碑 Done，无人工介入。
- `needs_fix_gives_up_after_max_attempts`：恒失败 → exit 2 且 attempts=cap。
- `failure_reason_injected_into_retry_prompt`：修复 prompt 含上一轮失败证据。

**ADR**：修订 0026/0033（自恢复循环与退出码语义）；视复杂度决定是否单立新 ADR「needs_fix 自恢复循环」。

---

### 迭代 2 — 截断根治 + 小步写引导（P1，健壮性头号杀手）

**目标**：消灭「大文件写截断」这一 §5 头号杀手。ADR 0037 已能*检测*截断，本迭代要*预防*并*自恢复*。

**改动点**：
1. **max_tokens 自适应/调高**：默认从 4096 提到更合理值（如 8192，`src/config.rs:35`），并在检测到 `finish_reason=length` 时对该 turn 自适应上调重试（有上限）。
2. **截断自恢复**：write_file/edit_file 输出被 0037 判定截断时，自动发起「续写」turn（携带已写前缀 + 明确续写指令），而非留半截文件。
3. **小步写引导**：background/system prompt 注入「大文件分块写」纪律（§6.5、§7 P1）；可选产出 `skills/small-step-writing.md` 供 agent 激活。
4. **与迭代 1 叠加**：截断触发的 needs_fix 现在能被迭代 1 的自恢复循环消费——组合覆盖 headless 下最大两处摩擦。

**L1/L2 验收测试**：
- `length_finish_reason_triggers_continuation`。
- `adaptive_max_tokens_bumps_within_cap`。
- `truncated_write_auto_continues_to_complete_file`：StubClient 模拟两段式输出拼出完整文件。

**ADR**：修订 0037（截断从检测扩到预防+续写）。

---

### 迭代 3 — acceptance 契约化（P1，护栏闭环）

**目标**：堵住「agent 用自己的工具写的 acceptance，却过不了自己的 gate」闭环缺口（§5 Issue A）。前次修复已让 prose 行*跳过*命令门退到 review，本迭代把它*正向契约化*。

**现状锚点**：`bg_gate::extract_gate_command`（`src/bg_gate.rs:29`）已「仅纯 ASCII 命令行才作命令门，prose 跳过」。

**改动点**：
1. **`milestone add/update` 写入时校验/引导**：检测 acceptance 是否为「独占一行裸命令」；混入 prose 时提示 agent 拆分为 `command:` 字段 + 自然语言说明两段（或 tool 侧自动分离），而非事后靠 `extract_gate_command` 猜。
2. **结构化 acceptance**：workgraph 节点 `acceptance` 支持可选结构化 `{ command, prose }`；命令门吃 `command`，review 门吃 `prose`。向后兼容旧纯字符串。
3. **弱信号显式化**：无命令时明确记 `gate=review(weak)`，让编排者/账本可见「这是弱信号验收」。

**L1 验收测试**：
- `milestone_add_splits_prose_and_command`。
- `structured_acceptance_routes_command_to_cmd_gate`。
- `cjk_prose_acceptance_marks_weak_gate`。

**ADR**：视改动面决定修订 0030（客观验收门）或补新 ADR。

---

### 迭代 4 — 探索兜底 + footgun 清零（自主执行 + 健壮性收尾，冲 90%）

**目标**：消灭「整轮只探索不动手」（§4.2）与 §6 八条 footgun，把两维度推过 90%。

**改动点**：
1. **no-op turn 兜底**：连续 N 个 turn 无「写/编辑/命令」类工具调用（纯探索）→ 注入 steering 提示「本 turn 必须产出改动或显式声明阻塞」，再不动手则升级告警（呼应 ADR 0029 turn steering）。
2. **`.ccd.env` 自动加载**（§6.7）：启动时若存在则自动加载，去掉「裸跑无 mode env 阻塞」坑。
3. **权限 key 工效**（§6.1/6.2）：`codecoder.json` allowlist 加载不再强依赖 root trust（或提供更醒目引导）；复合命令 keying 保留 ADR 0036 行为但文档化降级路径。
4. **并发写护栏**（§6.8）：单 daemon 并发发消息的文件版本竞争——ADR 0035 已有 workgraph 并发保护，本迭代补「编排者并发」检测/拒绝或串行化提示。

**L1 验收测试**：
- `noop_exploration_turns_trigger_steering`。
- `ccd_env_autoloaded_on_start`。
- `allowlist_loads_without_explicit_trust`（或引导路径测试）。

**ADR**：修订 0029（no-op steering）、0035/0036（并发与 keying 文档化）。

---

### 迭代 5 — dogfooding 转 L3 门控 + 单能力正确性收尾（P2，全维度重测）

**目标**：把「用 codecoder 建外部项目」固化为定期回归；收尾单能力正确性到 90%；重测全维度确认达标。

**改动点**：
1. **L3 门控用例**：新增 `#[ignore]` 的 `tests/` 用例——弱模型 headless 从零建一个小型外部项目（如缩微版 coedit）并断言无人干预跑完（exit 0、单测全绿），纳入 `docs/testing/behavioral-validation.md`。
2. **单能力正确性收尾**：修迭代 1–4 中暴露的 generate_skill/reason/edit_file 边界瑕疵（+5 主要是打磨）。
3. **重测与报告**：跑一次完整 dogfooding，更新维度表，产出「达标复核」附录到评价报告同目录。

**L2/L3 验收测试**：
- `external_project_build_completes_unattended`（门控）。
- 收尾项各自回归测试。

---

## 4. 横切关注

- **测试策略**：每迭代先 L1（StubClient 确定性、锁机制），能仿真的补 L2；L3 仅迭代 5 引入，定期/手动触发。全程遵守 TDD（红→绿→重构）。
- **文档同步**：按 CLAUDE.md 要求，每迭代同步 `ARCHITECTURE.md`/`README.md` 相关数字，触及决策补/修 ADR。
- **依赖与风险**：迭代 1→2 单向叠加（截断自恢复喂给自恢复循环）；迭代 3/4 相互独立可并行；迭代 5 依赖 1–4 完成。**最大风险** = 弱模型下自恢复循环陷入「反复假修」，用持久化 attempt 预算 + 退避 + `StuckNeedsFix` exit 2 兜死。
- **收尾定义（Definition of Done）**：五迭代全绿 + 迭代 5 的 L3 dogfooding 门控通过 + 维度表全部 ≥90% 且有测试/证据支撑。

---

## 5. 下一步

本 spec 经用户复核后，进入 writing-plans，把**迭代 1（needs_fix 自恢复循环）**细化为可执行实现计划（TDD 分解、文件级改动、里程碑依赖）。迭代 2–5 在各自启动时单独 brainstorm→plan。
