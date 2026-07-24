# ADR 0038 — 自适应 max_tokens 预算（截断根治）

- **状态**: Accepted
- **日期**: 2026-07-24
- **关联**: ADR 0027（截断 guard / `StopReason::Length`）、迭代 2 spec（coedit dogfooding 评估 §6.5/§7.2,`docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`）

## 背景

max_tokens 默认 4096 偏低,大文件写常触发 `StopReason::Length`。既有 guard 只 neutralize 半成品 tool call 并提示模型「拆分」,不提高预算 → 弱模型反复截断至撞 tool cap;且 Length + 空 `tool_calls` 会先命中收尾 break,被静默当作 turn 正常完成。

## 决策

1. max_tokens 默认 4096 → 8192（`CODECODER_MAX_TOKENS`,`src/config.rs`）。
2. turn 内局部 `effective_max_tokens`（每 turn 从 `self.max_tokens` 重置）;命中 Length 且未达封顶 → `saturating_mul(2).min(ceiling)` 后 `continue` 重试;发 `AgentEvent::Notice` 可观测。
3. 封顶 `CODECODER_MAX_TOKENS_CEILING`（默认 32768）,在 `AgentLoop::build` 内由 `Config::from_env()` 注入 → 所有构造点（cc/daemon/sub-agent/verify）统一遵守;测试经 `set_max_tokens_ceiling` 确定性覆盖。
4. Length 判定提到 `tool_calls.is_empty()` 收尾之前,修静默收尾;保留既有 guard（半序列化的 tool call 置 `is_error` 的 `ToolResult`,**绝不执行**）。
5. 不拼接半写文件（脆弱）;达封顶仍失败交里程碑客观门 → 迭代 1 needs_fix 自恢复循环。

## 后果

- 正面:大文件一次写成概率大增;弱模型无需自觉拆分;截断的纯文本响应不再静默收尾。
- 代价:单 turn 最坏多几次翻倍重试;部分 provider 对 max_tokens 有硬上限,超限请求走 `complete_retrying` 错误路径（不崩）。
- 补充（非本迭代）:按 token 预估文件大小预设 max_tokens;per-tool 预算。
