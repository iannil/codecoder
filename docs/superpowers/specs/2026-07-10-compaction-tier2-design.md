# Compaction tier-2 设计（ADR 0023 的延续）

- 日期：2026-07-10
- 状态：已批准（brainstorming），待转 writing-plans
- 关联：ADR 0023（context compaction）、0004（session persistence）、0017（provider-neutral messages）

## 1. 目的

实现 ADR 0023 的 **tier-2**：当 tier-1（丢 `Reasoning` + 占位化老 `ToolResult`）之后上下文**仍**超过模型窗口阈值时，把**最旧的对话跨度**用一次 LLM 调用摘要成一条合成 `System` 消息，插在 anchor 之后、取代中间老 turn。摘要**只塑造派生的 Context Working Set，绝不破坏持久化 Session**（与 tier-1 同一原则）。

## 2. 已定决策（brainstorming）

- **LLM 摘要调用位置**：`compaction::working_set` 保持**纯函数**（只做 tier-1）。tier-2 逻辑由 `AgentLoop` 驱动（它持有 `self.provider` 与内存缓存）。
- **缓存**：摘要**只存内存**（`AgentLoop` 字段），不改 Session schema。`/resume`/重启后若仍超阈会重新摘要一次（可接受）。

## 3. 架构与数据流

### 3.1 两级触发
1. 每次 `process_turn` 迭代照旧计算 tier-1 `working_set`（纯函数，不变）。
2. 对 tier-1 结果再测 token：若仍 `should_compact(count_tokens(tier1), window)`，触发 tier-2。

### 3.2 摘要跨度（turn 内稳定，缓存友好）
- 保护两端：
  - **anchor** = 第一条 `Role::User`（原始目标），永不动。
  - **当前 turn** = 从**最后一条 `Role::User`** 到末尾，永不动。
- 摘要区 = `messages[anchor+1 .. last_user_idx)`——除首目标外的**所有更早 turn**。
- 关键性质：一个 turn 内工具只追加 `Assistant`/`Tool` 消息，`last_user_idx` 不变 → 摘要区在 turn 内不变 → 缓存命中 → **每 turn 至多一次摘要 LLM 调用**。下个 turn 新 `User` 消息到来，区间增长、缓存失效、重新摘要。

### 3.3 tier-2 后的 working set
`[anchor, System("先前对话摘要：\n<summary>"), …当前 turn 的消息]`——中间老 turn 被一条合成 `System` 摘要取代。

### 3.4 退化情形
- 摘要区为空（只有一个 turn / 首轮，`anchor+1 >= last_user_idx`）→ **tier-2 不触发**，返回 tier-1 结果。
- 摘要区里的消息**先经 tier-1 处理**（Reasoning 已丢、老 ToolResult 已占位化）再渲染给摘要器——省 token。

## 4. 组件与接口

### 4.1 `compaction.rs`（纯逻辑新增）
- 保留 `should_compact`、`working_set`（tier-1）不变。
- 新增纯函数 `summary_span(messages: &[Message]) -> Option<(usize, usize)>`：返回 `[anchor+1, last_user_idx)` 的下标区间，空区间返回 `None`。**可单测**。
- 新增纯函数 `apply_tier2(tier1: &[Message], span: (usize, usize), summary_text: &str) -> Vec<Message>`：产出 `[anchor, System(summary), …tail]`。注意 span 是相对**原始 messages** 的下标，需在 tier-1 结果上按 id 对齐（见 §7 实现注记）。**可单测**。
- 新增 `fn render_span(messages: &[Message]) -> String`：把摘要区消息渲染成 `role: text` 文本，供摘要 prompt。

> 边界一致性：`summary_span` 与 tier-1 的 anchor 定义共用「第一条 User」；tier-2 的 tail 用「最后一条 User」而非 tier-1 的 `RECENT_TAIL`，两者独立且都合法（tier-2 的保护区是「当前 turn」）。

### 4.2 `AgentLoop`（tier-2 编排 + 缓存）
- 新增字段：`tier2: Option<Tier2Summary>`，`struct Tier2Summary { covered_last_id: MessageId, text: String }`。
- 新增方法 `fn context_working_set(&mut self, event_tx: &Sender<AgentEvent>) -> Vec<Message>`：
  1. `let tier1 = compaction::working_set(&self.model, &self.session.messages, self.model_window);`
  2. 若 `!should_compact(count_tokens(tier1), window)` → 返回 `tier1`。
  3. `let Some(span) = compaction::summary_span(&self.session.messages) else { return tier1 };`
  4. `covered_last_id = messages[span.1 - 1].id`。
  5. 缓存命中（`self.tier2` 的 `covered_last_id` 相等）→ 用缓存 `text`；否则调 `self.summarize_span(span)` 得 `text`，成功则 `self.tier2 = Some(...)`，失败 → 返回 `tier1`（降级）。
  6. 返回 `compaction::apply_tier2(&tier1, span, &text)`。
- 新增方法 `fn summarize_span(&self, span) -> anyhow::Result<String>`：构造摘要 `CompletionRequest`（system 摘要指令 + `render_span` 文本，`temperature 0`，`tools` 空，`max_tokens` 适中如 1024），`self.provider.complete(&req)?`，抽取回复 `Text` 项拼成字符串；空则 `Err`。
- `process_turn` 的调用点（agent.rs:258）由 `compaction::working_set(...)` 改为 `self.context_working_set(event_tx)`。

### 4.3 缓存失效
- 仅按 `covered_last_id` key：turn 内不变 → 命中；新 turn（新 User）→ `last_user_idx` 前移 → `covered_last_id` 变 → 未命中 → 重算。
- `AgentCommand::Clear` / `Resume` 后应清空 `self.tier2 = None`（历史整体替换，旧摘要作废）。

## 5. correlation 安全
- 被摘要取代的中间段若含 `ToolCall`，其配对 `ToolResult` 也在**同一连续区间**内被一并取代——摘要区两端都切在 `User` 边界，一个 turn 的 call/result 对不会被拆散，故不会遗留孤儿 `tool_call`（OpenAI 会拒无匹配响应的 tool_call）。

## 6. 降级与错误处理
- 摘要 `provider.complete` 报错 / 返回空文本 → **静默退回 tier-1 结果**，不缓存、不插摘要。turn 照常进行（等同 tier-2 未实现时行为）。不崩 turn。

## 7. 实现注记
- **span 下标对齐**：`summary_span` 基于原始 `messages` 的下标；`apply_tier2` 在 **tier-1 结果**上操作，而 tier-1 可能已丢弃 Reasoning-only 消息（改变了下标）。因此 `apply_tier2` 应**按 message id** 定位 anchor 与「摘要区之后的第一条」，而非裸下标。实现时以 id 边界（anchor_id、`covered_last_id`）切分 tier-1 结果：保留 `id == anchor_id`，丢弃 `anchor_id < id <= covered_last_id`，其后原样保留，并在 anchor 后插入 System 摘要。
- **每 turn 一次**：`context_working_set` 每迭代调用，但缓存按 `covered_last_id` 命中，故一个 turn 内摘要 LLM 调用至多一次。
- **阈值仍对满量**：tier-1 触发照旧对**全量** `messages` 测（防振荡，ADR 0023）；tier-2 的二次判定对 **tier-1 结果** 测（是「tier-1 后仍超吗」，不回灌进 tier-1 触发器，不振荡）。

## 8. 测试

### 8.1 纯逻辑单测（`compaction.rs`）
- `summary_span`：多 User 的历史返回 `[anchor+1, last_user_idx)`；单 User / 首轮返回 `None`；anchor 后紧跟 last_user（相邻）返回空→`None`。
- `apply_tier2`：给定 tier-1 结果 + span + 摘要文本，产出 `[anchor, System(summary), …tail]`；anchor 原文保留；中间段消失；tail 原样；按 id 对齐正确（构造含 Reasoning-only 被 tier-1 丢弃的情形，验证下标错位不影响）。
- tier-1 既有测试不回归。

### 8.2 黑盒 L1（经 ScriptedProvider，`tests/l1_compaction.rs`）
- 构造：anchor(User) + 若干更早 turn（大 ToolResult，跨多个 User 消息，带唯一标记 `OLD_TURN_MARK`）+ 当前 turn(User)。窗口调小（或大 payload）使 tier-1 后**仍**超阈。
- ScriptedProvider 脚本顺序：`turns[0]` = 摘要文本（含 `SUMMARY_MARK`）；`turns[1]` = 当前 turn 助手回复。
- 断言（recorded requests）：
  1. `requests[0]`（摘要请求）messages 含 `OLD_TURN_MARK`（摘要器读到了老 span）。
  2. `requests[1]`（真正 turn 请求）含一条 `System` 带 `SUMMARY_MARK`，且**不含** `OLD_TURN_MARK`（中间段被取代）。
  3. anchor 原始目标文本仍在 `requests[1]`。
  4. 持久化 `sessions/*.json` 仍含全量（含 `OLD_TURN_MARK`）——tier-2 是派生、非破坏（复用既有 §5.7 断言风格）。
- 降级测试：脚本让摘要调用返回空（或用一个会失败的路径）→ 断言退回 tier-1（`requests` 里没有 System 摘要，turn 仍完成）。

## 9. 交付物
1. 本设计文档。
2. 实现（经 writing-plans）：`compaction.rs`（`summary_span`/`apply_tier2`/`render_span` + 单测）；`agent.rs`（`Tier2Summary` 字段、`context_working_set`、`summarize_span`、调用点替换、Clear/Resume 清缓存）；`tests/l1_compaction.rs` 增 tier-2 黑盒 + 降级测试；文档计数与 ADR 0023 状态更新（tier-2 已实现）。

## 10. 成功判据
- tier-1 后仍超阈时，发往 provider 的真正 turn 请求中，老 turn 被一条 System 摘要取代，anchor 与当前 turn 保留。
- 每 turn 至多一次摘要 LLM 调用；turn 内缓存命中。
- 摘要失败静默降级为 tier-1，不崩 turn。
- 持久化 Session 仍全量；`compaction.rs` 既有测试零回归；默认套件保持 hermetic 全绿。
- ADR 0023 状态更新为 tier-2 已实现。
