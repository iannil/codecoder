# 设计 · 迭代 2：截断根治（自适应 max_tokens）+ 小步写引导

- **日期**: 2026-07-24
- **类型**: 迭代实现设计（spec）
- **上游**: `docs/superpowers/specs/2026-07-23-codecoder-maturity-to-90-roadmap-design.md`（路线图 · 迭代 2）
- **关联 ADR**: 0027（截断 guard / StopReason::Length）、0037（工具输出截断——**不同的东西**，见 §1）

---

## 1. 背景与问题定位

评价报告把「大文件写截断」列为 headless 下**头号杀手**。探查实际代码后，对路线图的初始框架做一处修正：

- **模型生成截断**已由 `StopReason::Length` 检测（`src/provider/openai.rs:135` 映射 `finish_reason=="length"`）。agent 循环（`src/agent.rs:891`）在命中 Length 且有 tool_calls 时**拒绝执行**可能半序列化的 tool call，追加 `is_error` 的 "tool call truncated… retry shorter or split" 结果并 `continue` 重试——即已有 **guard**（防止写坏文件）。
- **ADR 0037 是另一件事**：它截断 **工具读取输出**（read_file/run_command 的返回）以保护上下文，与模型自身生成被截断无关。路线图「0037 检测写截断」的说法不准确。

**真实缺口**：
1. `max_tokens` 默认 4096 偏低 → 非平凡文件写经常触发 Length。
2. 现有 guard 只**提示**弱模型「重试更短/拆分」，**不提高预算** → 弱模型可能反复截断直到撞 tool cap。检测到但未**治本**。
3. `StopReason::Length` 且**无** tool_calls（截断的纯文本响应）会先命中 `tool_calls.is_empty()` 的收尾 break（`src/agent.rs:877`，位于 Length 检查之前）→ 被当作「turn 正常结束」静默收尾。
4. 无主动的小步写引导。

路线图原案的「携带已写前缀发起续写」被否决：拼接半写文件（解析部分 JSON args / 文件内容）脆弱且与现有 guard 重叠。现有 guard 丢弃半成品并干净重试更简单、更稳。

---

## 2. 决策（已确认）

- **核心机制**：提高 `max_tokens` 默认值 + 命中 `StopReason::Length` 时**自适应上调**该 turn 的有效 max_tokens（有上限），复用现有 guard，不拼接半成品。
- **数值策略**：默认 `4096 → 8192`；命中 Length 时有效 max_tokens **翻倍**（8192→16384→32768），**封顶 32768**；封顶值 env 可调（`CODECODER_MAX_TOKENS_CEILING`）。
- **小步写引导**：仅在 background/system prompt 加一行纪律；**不**产出独立 skill（YAGNI）。

---

## 3. 架构与改动点

### 改动点 1 — config（`src/config.rs`）
- `max_tokens` 默认由 4096 改为 **8192**（`config.rs:35` 的 `unwrap_or(4096)` → `unwrap_or(8192)`）。
- 新增字段 `max_tokens_ceiling: u32`，env `CODECODER_MAX_TOKENS_CEILING`，默认 **32768**，解析模式与其它 `*_ceiling`/数值字段一致（`.and_then(|v| v.parse().ok()).unwrap_or(32768)`）。

### 改动点 2 — 自适应预算（`src/agent.rs` turn 工具迭代循环）
- 在 turn 的工具迭代循环开始处引入局部 `let mut effective_max_tokens = self.max_tokens;`。
- 构造 `CompletionRequest` 时用 `effective_max_tokens` 替换 `self.max_tokens`（`src/agent.rs:836`）。
- 命中 `StopReason::Length` 时：
  1. 若 `effective_max_tokens < ceiling` → `effective_max_tokens = effective_max_tokens.saturating_mul(2).min(ceiling)`；发一条 `AgentEvent::Notice`（可观测「因截断上调预算至 N」）；**保留现有 guard**（对 tool_calls 追加 is_error 结果 neutralize 半成品）；`continue` 带更大预算重试。
  2. 若 `effective_max_tokens >= ceiling` → 维持现有行为（guard 的 is_error 结果 + continue，最终由 tool cap 收尾），不再翻倍。
- **边界修正（缺口 3）**：把 Length 判定提到 `tool_calls.is_empty()` 收尾 break 之前。Length 且未达 ceiling → 无论有无 tool_calls 都 bump + 重试（empty 情形可附一句简短 nudge 或直接重试）；仅当**达 ceiling 的 empty** 情形才走原 break 收尾，避免截断纯文本被静默当完成。
- `effective_max_tokens` **每个 turn 重置**回 `self.max_tokens`（不跨 turn 累积）。
- 不改 `Provider` trait、不改 `CompletionRequest` 结构、`ceiling` 通过构造把 `Config.max_tokens_ceiling` 传入 `AgentLoop`（新增字段，随 `self.max_tokens` 一并注入）。

### 改动点 3 — 小步写 system-prompt 引导
- 在 background/system prompt（agent 身份注入路径）加一行纪律：写大文件时优先分块 append（多次 `edit_file`/`write_file` 追加）而非单次巨量 `write_file`，以免被 max_tokens 截断。始终生效、不依赖 `use_skill`。

### 与迭代 1 的组合
自适应 bump 到 ceiling 仍失败 → 该 turn 产物不完整 → 里程碑客观门大概率判 needs_fix → **迭代 1 自恢复循环接手重试**（带失败原因注入）。bump 治单 turn 内截断，自恢复治「整里程碑仍未完成」，天然衔接，无需额外接线。

---

## 4. 测试策略（TDD，全 hermetic）

- **L1 config**：`max_tokens` 默认 8192；`CODECODER_MAX_TOKENS_CEILING` 默认 32768 + override（env test 保持 remove/set/remove 结构，防泄漏）。
- **L1 agent**（复用现有 `TruncatedToolCall` 测试模式，新增一个记录每次 `req.max_tokens` 的有状态 Provider）：
  - `length_stop_bumps_effective_max_tokens_on_retry`：首次返回 Length，断言重试请求的 `max_tokens` 由 8192 翻倍为 16384。
  - `effective_max_tokens_caps_at_ceiling`：连续 Length，断言有效 max_tokens 不超过 ceiling（32768）。
  - `length_with_empty_tool_calls_retries_not_silently_done`：Length + 无 tool_calls，断言不静默收尾而是 bump 重试（达 ceiling 后才收尾）。
  - `bump_resets_per_turn`：一个 turn bump 后，下一 turn 从 `self.max_tokens` 起。
  - 保留并确认现有 `truncated_tool_call_is_not_executed_and_loop_recovers` 仍绿（guard 不回退）。

---

## 5. 文档同步

- README env 表：更新 `CODECODER_MAX_TOKENS` 默认为 8192；新增 `CODECODER_MAX_TOKENS_CEILING`（默认 32768，自适应上调封顶）。
- ARCHITECTURE：补「自适应 max_tokens 预算」描述。
- ADR：将截断处理从「detect + guard」扩到「adaptive cure」——修订 ADR 0027（截断 guard）追加自适应 bump 说明，或新立一条「自适应 max_tokens 预算」ADR（plan 阶段定夺；倾向修订 0027 以保持同一主题的连续性）。

---

## 6. 依赖与风险

- **唯一实质风险**：部分模型/provider 对 `max_tokens` 有硬上限，bump 超限会被 provider 拒。缓解：ceiling 默认 32768 保守；env 可下调；bump 后的请求若被 provider 拒，走现有 `complete_retrying` 错误路径，不崩（最坏退化为一次失败 turn，再由迭代 1 自恢复）。
- 无并发/状态迁移风险（纯循环内局部变量 + 两个 config 值 + 一行 prompt）。

---

## 7. 收尾定义（DoD）

- §4 全部 L1 测试绿；现有截断 guard 测试不回退；全仓 `cargo test` 绿；文档数字一致。
- 维度预期抬升（与路线图一致）：健壮性 55→~72、自主执行 →~85。

---

## 8. 下一步

本 spec 经用户复核后，进入 writing-plans 细化迭代 2 为 TDD 分解、文件级改动的实现计划。
