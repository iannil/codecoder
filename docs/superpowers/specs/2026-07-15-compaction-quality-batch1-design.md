# Compaction 质量改进（第一批）— 设计

**日期**: 2026-07-15
**状态**: 待评审
**关联**: ADR 0023（Compaction / Context Working Set）
**来源**: 借鉴 `archived/pi`（pi-mono）的 compaction 工程实践（见 `packages/coding-agent/docs/compaction.md`）

## 背景

CodeCoder 已实现 ADR 0023 的 tier-1 + tier-2 compaction（`src/compaction.rs` + `src/agent.rs::context_working_set`）。对照 pi 的实现，其 tier-2 摘要在**质量与连续性**上有几处成熟做法值得吸收，且都能局限在现有 compaction 模块内、不触动任何契约。

本设计只覆盖**第一批**（快赢、零契约变更）。token 化的近端保留与 split-turn（需改 tier-1 tail 语义和 `summary_span` 边界，属 ADR 0023 修订）列为后续子项目，不在本 spec 内。

## 目标

在**不改变** ADR 0023 核心立场（compaction 是派生的 Context Working Set，绝不改写持久化 Session）的前提下，提升 tier-2 摘要质量：

1. **结构化摘要模板** — 摘要从自由散文变为可预期的固定结构。
2. **迭代式摘要** — span 增长时把上一版摘要当上下文传入，只摘增量，提升连续性、省 token。
3. **累积文件追踪** — 跨轮累积 span 内读/改过的文件路径，附在摘要末尾。
4. **tool result 截断阈值 200 → 2000** — 现值过狠，常把关键输出摘没。

## 非目标

- 不把摘要持久化为 session entry（pi 的 `CompactionEntry` 做法）。CodeCoder 刻意保持 compaction 派生、非破坏——**红线，不碰**。
- 不做 token 化近端保留、不做 split-turn（第二批）。
- 不改 tier-1（`working_set`）逻辑。

## 现状锚点（实现前的真实代码）

- `src/compaction.rs::render_span` —— 渲染 span 为纯文本，tool result 截断到 **200** 字符。
- `src/agent.rs::summarize_span(&self, rendered: &str) -> Result<String>` —— 自由散文 system prompt，无"上一版摘要"入参。
- `src/agent.rs::context_working_set` —— tier2 缓存 `Tier2Summary { covered_last_id, text }`，命中条件为 `covered_last_id` 完全相等；未命中即对**整段** `[start..end]` 重新摘要，不带旧摘要。
- 文件工具与参数键：`read_file`（读，`path`）、`write_file` / `edit_file`（改，`path`）。
- `MessageItem::ToolCall { id, name, args }`；`args` 为 `serde_json::Value`。

## 设计

### 数据结构

扩展 `agent.rs` 的 `Tier2Summary`：

```rust
struct Tier2Summary {
    covered_last_id: MessageId,
    text: String,                          // 纯散文摘要，不含文件块
    read_files: std::collections::BTreeSet<String>,
    modified_files: std::collections::BTreeSet<String>,
}
```

`BTreeSet` 天然去重 + 稳定排序，便于渲染与快照测试。

### 1. 结构化摘要模板（`summarize_span`）

改签名，加入"上一版摘要"入参：

```rust
fn summarize_span(&self, rendered: &str, previous: Option<&str>) -> anyhow::Result<String>
```

system prompt 换成结构化指令，要求模型只产出以下小节的散文（**不**要求模型列文件——文件清单由我们确定性计算）：

```
## 目标
## 约束与偏好
## 进展（已完成 / 进行中 / 受阻）
## 关键决策
## 下一步
## 关键上下文
```

当 `previous` 为 `Some` 时，追加一条 user 消息块 `先前摘要：\n{previous}`，指令改为"把先前摘要与下列新增消息合并、更新为一份完整摘要"。`previous` 为 `None` 时按整段首次摘要处理。其余请求参数（max_tokens/temperature/无工具）不变。

### 2. 迭代式摘要 + 3. 文件追踪（`context_working_set`）

新流程：

1. `tier1 = working_set(...)`；若不再超阈值 → 返回 `tier1`（同现状）。
2. `summary_span` 得 `(start, end)`，否则返回 `tier1`（同现状）。
3. `anchor_id = messages[start-1].id`；`covered_last_id = messages[end-1].id`。
4. 依缓存决定**增量**与**种子文件集**：
   - 缓存存在且 `cache.covered_last_id == covered_last_id` → **完全复用**：`prose = cache.text`，文件集 = 缓存的两个集合（不再调 LLM）。
   - 缓存存在且 `cache.covered_last_id < covered_last_id`（span 增长）→ **增量**：增量切片 = `[i..end]`，其中 `i` 为 `[start,end)` 内首个 `messages[i].id > cache.covered_last_id` 的下标（id 随下标单调，边界连续）；`previous = Some(&cache.text)`；文件集种子 = 缓存两个集合的克隆。
   - 其余（无缓存 / `cache.covered_last_id > covered_last_id`，如 `/resume` 后 id 错位）→ **首次**：切片 = `[start..end]`，`previous = None`，文件集种子为空。
5. `compaction::collect_file_paths(slice, &mut read, &mut modified)` 把增量/整段切片里的文件路径并入种子集。
6. `prose`：复用分支直接用缓存；否则 `self.summarize_span(&render_span(slice), previous)`，失败 → 降级返回 `tier1`（同现状），并**不**污染缓存。
7. 写回 `self.tier2 = Some(Tier2Summary { covered_last_id, text: prose, read_files, modified_files })`。
8. `summary_text = prose + compaction::render_file_blocks(&read, &modified)`。
9. `compaction::apply_tier2(&tier1, anchor_id, covered_last_id, &summary_text)`（`apply_tier2` 不变）。

`Notice` 事件的发送时机不变（仅在真正调用 LLM 摘要那次发）。

### compaction.rs 新增纯函数

```rust
/// 扫描 span 内的 ToolCall，按工具名把 `path` 参数并入读/改集合。
/// read_file → read；write_file/edit_file → modified。
pub fn collect_file_paths(
    span: &[Message],
    read: &mut BTreeSet<String>,
    modified: &mut BTreeSet<String>,
);

/// 渲染文件块；两个集合都空时返回空串（不占 token）。非空时形如：
/// \n\n<read-files>\n a.rs\n b.rs\n</read-files>\n<modified-files>\n c.rs\n</modified-files>
pub fn render_file_blocks(read: &BTreeSet<String>, modified: &BTreeSet<String>) -> String;
```

### 4. 截断阈值（`render_span`）

`output.chars().take(200)` → `take(2000)`。仅此一处常量级改动。

## 测试

**`src/compaction.rs`（纯函数，单测）**

- `collect_file_paths`：混合 `read_file`/`write_file`/`edit_file` 及重复路径 → 读/改集合正确、去重。
- `collect_file_paths`：非文件工具（如 `run_command`）与缺 `path` 的调用 → 不产生条目。
- `render_file_blocks`：两集合空 → 空串；仅 read 非空 → 只出 `<read-files>`；两者非空 → 两块都在、路径有序。
- 更新既有 `render_span_drops_reasoning_and_truncates_tool_results`：把输入 tool result 放大到 5000 字符，断言输出保留约 2000 而非 200（原断言 `len < 400` 会失效，改为 `> 1000 && < 2100`）。

**`src/agent.rs`（StubClient 驱动，集成味单测）**

- 迭代复用：连续两次 `context_working_set`，第二次在 span 未增长时**不**再触发摘要（`StubClient` 计数或缓存命中断言）；span 增长时以 `previous=Some` 再摘一次。若 StubClient 不便计数，则至少断言输出含文件块且摘要文本稳定。

## 影响文件

- `src/compaction.rs` — 新增 `collect_file_paths`、`render_file_blocks`；`render_span` 截断常量 200→2000；扩测。
- `src/agent.rs` — `Tier2Summary` 增两字段；`summarize_span` 加 `previous` 入参与结构化 prompt；`context_working_set` 增量/文件追踪逻辑。
- `docs/adr/0023-*.md` — 追加"增强说明"小节（结构化模板 + 迭代摘要 + 文件追踪，均为非契约增强）。
- `CLAUDE.md` / `ARCHITECTURE.md` — 若其中的 compaction 描述需要同步，补一句结构化摘要与文件追踪；数字无变化。

## 风险与回退

- 全部为加法或阈值调整，`apply_tier2` / tier-1 / 持久化均不动。摘要 LLM 调用失败仍走既有 `return tier1` 降级路径。
- 迭代增量切片若边界计算错误，最坏是"重复摘要一小段"或"漏摘一小段"，不影响 anchor/tail 保护与 tool_call 配对正确性；单测覆盖边界。
```