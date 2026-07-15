# Compaction 质量改进（第一批）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 提升 tier-2 compaction 摘要的质量与连续性（结构化模板、迭代式摘要、累积文件追踪、tool-result 截断放宽），不改动任何 ADR 0023 契约。

**Architecture:** 全部改动局限在 `src/compaction.rs`（两个新纯函数 + 一个截断常量）与 `src/agent.rs`（`Tier2Summary` 扩两字段、`summarize_span` 加 `previous` 入参与结构化 prompt、`context_working_set` 增量/文件追踪逻辑）。摘要仍是**派生**的 Context Working Set，绝不写入持久化 Session。

**Tech Stack:** Rust；`serde_json::Value`（tool args）；`std::collections::BTreeSet`（去重+稳定排序）；测试用 `#[cfg(test)]` 内联单测 + `ScriptedProvider` 风格的假 Provider。

## Global Constraints

- 不把摘要持久化为 session entry；`apply_tier2` / tier-1 `working_set` / 持久化路径**不改**。
- 摘要 LLM 调用失败时保持既有降级：`return tier1`，且**不**写 `self.tier2` 缓存。
- 文件工具与参数键固定：`read_file`（读，arg `path`）、`write_file` / `edit_file`（改，arg `path`）。
- `Notice` 事件只在**真正调用 LLM 摘要**那次发送（缓存全复用时不发）。
- 每个任务结束跑 `cargo test` 全绿再提交；提交信息尾行加 `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`。

---

### Task 1: 放宽 `render_span` 的 tool-result 截断（200 → 2000）

**Files:**
- Modify: `src/compaction.rs`（`render_span` 内 `output.chars().take(200)`）
- Test: `src/compaction.rs`（更新既有 `render_span_drops_reasoning_and_truncates_tool_results`）

**Interfaces:**
- Consumes: 无
- Produces: `render_span` 行为不变，仅截断上限从 200 → 2000 字符。

- [ ] **Step 1: 更新既有测试使其针对 2000 上限失败**

把 `src/compaction.rs` 测试模块里的 `render_span_drops_reasoning_and_truncates_tool_results` 整体替换为：

```rust
    #[test]
    fn render_span_drops_reasoning_and_truncates_tool_results() {
        let span = vec![
            msg(1, Role::Assistant, vec![
                MessageItem::Text { text: "hello".into() },
                MessageItem::Reasoning { text: "SECRET".into() },
            ]),
            msg(2, Role::Tool, vec![MessageItem::ToolResult { call_id: "c".into(), output: "x".repeat(5000), is_error: false }]),
        ];
        let s = render_span(&span);
        assert!(s.contains("hello"));
        assert!(!s.contains("SECRET"));        // reasoning omitted
        assert!(s.len() > 1000);               // keeps well past the old 200 cap
        assert!(s.len() < 2200);               // but still truncated near 2000
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test render_span_drops_reasoning_and_truncates_tool_results`
Expected: FAIL（现值 `take(200)` → `s.len()` 约 220，`> 1000` 断言不成立）

- [ ] **Step 3: 改截断常量**

在 `src/compaction.rs::render_span` 中，把 `ToolResult` 分支的：

```rust
                    let snippet: String = output.chars().take(200).collect();
```

改为：

```rust
                    let snippet: String = output.chars().take(2000).collect();
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test render_span_drops_reasoning_and_truncates_tool_results`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/compaction.rs
git commit -m "feat(compaction): widen render_span tool-result truncation 200->2000

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: 新增 `collect_file_paths`（从 span 提取读/改文件路径）

**Files:**
- Modify: `src/compaction.rs`（顶部 imports + 新函数 + 测试）

**Interfaces:**
- Consumes: `Message` / `MessageItem::ToolCall { name, args, .. }`
- Produces:
  ```rust
  pub fn collect_file_paths(
      span: &[Message],
      read: &mut std::collections::BTreeSet<String>,
      modified: &mut std::collections::BTreeSet<String>,
  )
  ```
  语义：`read_file` 的 `path` 并入 `read`；`write_file`/`edit_file` 的 `path` 并入 `modified`；其它工具与缺 `path` 的调用忽略。就地累积（不清空传入集合）。

- [ ] **Step 1: 写失败测试**

在 `src/compaction.rs` 测试模块末尾追加：

```rust
    #[test]
    fn collect_file_paths_splits_read_and_modified_and_dedups() {
        use std::collections::BTreeSet;
        fn call(id: &str, name: &str, args: serde_json::Value) -> Message {
            msg(0, Role::Assistant, vec![MessageItem::ToolCall { id: id.into(), name: name.into(), args }])
        }
        let span = vec![
            call("c1", "read_file", json!({ "path": "a.rs" })),
            call("c2", "edit_file", json!({ "path": "b.rs", "old": "x", "new": "y" })),
            call("c3", "write_file", json!({ "path": "b.rs", "content": "z" })), // dup modified
            call("c4", "run_command", json!({ "cmd": "ls" })),                    // no path
            call("c5", "read_file", json!({})),                                  // missing path
        ];
        let mut read = BTreeSet::new();
        let mut modified = BTreeSet::new();
        collect_file_paths(&span, &mut read, &mut modified);
        assert_eq!(read.into_iter().collect::<Vec<_>>(), vec!["a.rs".to_string()]);
        assert_eq!(modified.into_iter().collect::<Vec<_>>(), vec!["b.rs".to_string()]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test collect_file_paths_splits_read_and_modified_and_dedups`
Expected: FAIL（`collect_file_paths` 未定义，编译错误）

- [ ] **Step 3: 实现函数**

在 `src/compaction.rs` 顶部 imports 补一行：

```rust
use std::collections::BTreeSet;
```

在 `render_span` 之后（`apply_tier2` 之前）插入：

```rust
/// 扫描 span 内的 ToolCall，按工具名把 `path` 参数分入读/改集合。
/// `read_file` → `read`；`write_file`/`edit_file` → `modified`。就地累积，
/// 便于跨多次 compaction 叠加历史。
pub fn collect_file_paths(span: &[Message], read: &mut BTreeSet<String>, modified: &mut BTreeSet<String>) {
    for m in span {
        for it in &m.items {
            if let MessageItem::ToolCall { name, args, .. } = it {
                let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                    continue;
                };
                match name.as_str() {
                    "read_file" => {
                        read.insert(path.to_string());
                    }
                    "write_file" | "edit_file" => {
                        modified.insert(path.to_string());
                    }
                    _ => {}
                }
            }
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test collect_file_paths_splits_read_and_modified_and_dedups`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/compaction.rs
git commit -m "feat(compaction): collect_file_paths — track read/modified files in a span

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: 新增 `render_file_blocks`（渲染读/改文件块）

**Files:**
- Modify: `src/compaction.rs`（新函数 + 测试）

**Interfaces:**
- Consumes: `std::collections::BTreeSet<String>`
- Produces:
  ```rust
  pub fn render_file_blocks(read: &BTreeSet<String>, modified: &BTreeSet<String>) -> String
  ```
  语义：两集合都空 → 返回空串；否则返回以 `\n\n` 起始、含非空块的文本。仅渲染非空块。

- [ ] **Step 1: 写失败测试**

在 `src/compaction.rs` 测试模块末尾追加：

```rust
    #[test]
    fn render_file_blocks_omits_empty_and_formats_present() {
        use std::collections::BTreeSet;
        let empty = BTreeSet::new();
        assert_eq!(render_file_blocks(&empty, &empty), "");

        let read: BTreeSet<String> = ["a.rs".to_string(), "b.rs".to_string()].into_iter().collect();
        let modified: BTreeSet<String> = ["c.rs".to_string()].into_iter().collect();
        let s = render_file_blocks(&read, &modified);
        assert!(s.starts_with("\n\n"));
        assert!(s.contains("<read-files>\na.rs\nb.rs\n</read-files>"));
        assert!(s.contains("<modified-files>\nc.rs\n</modified-files>"));

        // 只有 read 非空时不渲染 modified 块。
        let only_read = render_file_blocks(&read, &empty);
        assert!(only_read.contains("<read-files>"));
        assert!(!only_read.contains("<modified-files>"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test render_file_blocks_omits_empty_and_formats_present`
Expected: FAIL（`render_file_blocks` 未定义）

- [ ] **Step 3: 实现函数**

在 `collect_file_paths` 之后插入：

```rust
/// 把读/改文件集合渲染成附加在摘要末尾的块。两集合都空时返回空串（不占 token）。
pub fn render_file_blocks(read: &BTreeSet<String>, modified: &BTreeSet<String>) -> String {
    if read.is_empty() && modified.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n");
    if !read.is_empty() {
        s.push_str("<read-files>\n");
        for p in read {
            s.push_str(p);
            s.push('\n');
        }
        s.push_str("</read-files>\n");
    }
    if !modified.is_empty() {
        s.push_str("<modified-files>\n");
        for p in modified {
            s.push_str(p);
            s.push('\n');
        }
        s.push_str("</modified-files>\n");
    }
    s
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test render_file_blocks_omits_empty_and_formats_present`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/compaction.rs
git commit -m "feat(compaction): render_file_blocks — emit read/modified-files blocks

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `agent.rs` — 结构化摘要 + 迭代 + 文件追踪接线

**Files:**
- Modify: `src/agent.rs`（`Tier2Summary` 结构、`summarize_span`、`context_working_set`、imports、测试）

**Interfaces:**
- Consumes: `compaction::collect_file_paths`、`compaction::render_file_blocks`、`compaction::render_span`、`compaction::working_set`、`compaction::summary_span`、`compaction::apply_tier2`、`compaction::should_compact`
- Produces: `context_working_set` 输出的 System 摘要消息正文现包含结构化散文 + 可选 `<read-files>`/`<modified-files>` 块。

- [ ] **Step 1: 扩 `Tier2Summary` 与 imports**

在 `src/agent.rs` 顶部 imports 区（`use std::sync::Arc;` 附近）补：

```rust
use std::collections::BTreeSet;
```

把 `Tier2Summary` 结构（约 `src/agent.rs:94`）替换为：

```rust
struct Tier2Summary {
    covered_last_id: MessageId,
    text: String,
    read_files: BTreeSet<String>,
    modified_files: BTreeSet<String>,
}
```

- [ ] **Step 2: 替换 `summarize_span`（加 `previous` 入参 + 结构化 prompt）**

把整个 `summarize_span` 方法（约 `src/agent.rs:287-316`）替换为：

```rust
    /// One-shot LLM summary of a rendered span (ADR 0023 tier-2). Structured brief;
    /// when `previous` is set, the earlier summary is merged with the new span
    /// (iterative compaction). Returns Err on transport failure or empty output.
    fn summarize_span(&self, rendered: &str, previous: Option<&str>) -> anyhow::Result<String> {
        let system = "You are compacting an agent's conversation history into a concise, \
            structured brief. Use exactly these sections, plain prose under each, and omit a \
            section when it has no content:\n\
            ## 目标\n## 约束与偏好\n## 进展（已完成 / 进行中 / 受阻）\n## 关键决策\n## 下一步\n## 关键上下文\n\
            Preserve goals, decisions, key facts, file paths, tool outcomes, and open threads. \
            Do NOT list read/modified files — those are tracked separately. Omit chit-chat and \
            any preamble.";
        let mut messages = vec![Message::text(0, Role::System, system)];
        let mut uid = 1u64;
        if let Some(prev) = previous {
            messages.push(Message::text(
                uid,
                Role::User,
                format!("先前摘要（请与下列新增消息合并，更新为一份完整摘要）：\n{prev}"),
            ));
            uid += 1;
        }
        messages.push(Message::text(uid, Role::User, rendered.to_string()));
        let req = CompletionRequest {
            model: self.model.clone(),
            messages,
            max_tokens: 1024,
            temperature: 0.0,
            tools: vec![],
        };
        let reply = self.provider.complete(&req)?;
        let text: String = reply
            .items
            .iter()
            .filter_map(|it| match it {
                MessageItem::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            anyhow::bail!("empty summary");
        }
        Ok(text)
    }
```

- [ ] **Step 3: 替换 `context_working_set`（增量 + 文件追踪）**

把整个 `context_working_set` 方法（约 `src/agent.rs:321-358`）替换为：

```rust
    fn context_working_set(&mut self, event_tx: &Sender<AgentEvent>) -> Vec<Message> {
        let tier1 = compaction::working_set(&self.model, &self.session.messages, self.model_window);
        if !compaction::should_compact(
            crate::tokenizer::count_tokens(&self.model, &tier1),
            self.model_window,
        ) {
            return tier1;
        }
        let Some((start, end)) = compaction::summary_span(&self.session.messages) else {
            return tier1;
        };
        let anchor_id = self.session.messages[start - 1].id;
        let covered_last_id = self.session.messages[end - 1].id;

        let mut read = BTreeSet::new();
        let mut modified = BTreeSet::new();
        let prose: String;

        match self.tier2.as_ref() {
            // Span unchanged → full reuse, no LLM call, no Notice.
            Some(c) if c.covered_last_id == covered_last_id => {
                read = c.read_files.clone();
                modified = c.modified_files.clone();
                prose = c.text.clone();
            }
            // Span grew → summarize only the increment, seeded by cached summary + files.
            Some(c) if c.covered_last_id < covered_last_id => {
                read = c.read_files.clone();
                modified = c.modified_files.clone();
                let inc_start = self.session.messages[start..end]
                    .iter()
                    .position(|m| m.id > c.covered_last_id)
                    .map(|p| start + p)
                    .unwrap_or(start);
                let slice = &self.session.messages[inc_start..end];
                compaction::collect_file_paths(slice, &mut read, &mut modified);
                let rendered = compaction::render_span(slice);
                let prev = c.text.clone();
                match self.summarize_span(&rendered, Some(&prev)) {
                    Ok(t) => {
                        let _ = event_tx.send(AgentEvent::Notice(
                            "compacting context (summarizing earlier turns)…".into(),
                        ));
                        prose = t;
                    }
                    Err(_) => return tier1,
                }
            }
            // No cache, or id rewound (e.g. after /resume) → summarize the whole span.
            _ => {
                let slice = &self.session.messages[start..end];
                compaction::collect_file_paths(slice, &mut read, &mut modified);
                let rendered = compaction::render_span(slice);
                match self.summarize_span(&rendered, None) {
                    Ok(t) => {
                        let _ = event_tx.send(AgentEvent::Notice(
                            "compacting context (summarizing earlier turns)…".into(),
                        ));
                        prose = t;
                    }
                    Err(_) => return tier1,
                }
            }
        }

        self.tier2 = Some(Tier2Summary {
            covered_last_id,
            text: prose.clone(),
            read_files: read.clone(),
            modified_files: modified.clone(),
        });

        let summary_text = format!("{}{}", prose, compaction::render_file_blocks(&read, &modified));
        compaction::apply_tier2(&tier1, anchor_id, covered_last_id, &summary_text)
    }
```

- [ ] **Step 4: 写集成测试（StubProvider 强制 compaction，验证文件块 + 摘要）**

在 `src/agent.rs` 测试模块（`mod tests`）末尾、最后一个 `}` 之前追加：

```rust
    /// Returns a fixed summary text for any tier-2 summarization request.
    struct SummaryProvider;
    impl Provider for SummaryProvider {
        fn name(&self) -> &str {
            "summary"
        }
        fn complete(&self, _req: &CompletionRequest) -> anyhow::Result<Message> {
            Ok(Message::text(0, Role::Assistant, "SUMMARY-PROSE"))
        }
    }

    #[test]
    fn context_working_set_summarizes_and_appends_file_blocks() {
        let dir = std::env::temp_dir().join(format!("cc_compact_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let provider = Arc::new(SummaryProvider);
        let mut agent = AgentLoop::new(provider, "m", 256, 0.0, dir.clone());
        // Force compaction regardless of real token counts.
        agent.model_window = 10;
        agent.session.messages = vec![
            Message::text(0, Role::User, "goal"), // anchor
            Message {
                id: 1,
                role: Role::Assistant,
                items: vec![MessageItem::ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    args: serde_json::json!({ "path": "foo.rs" }),
                }],
            },
            Message {
                id: 2,
                role: Role::Tool,
                items: vec![MessageItem::ToolResult {
                    call_id: "c1".into(),
                    output: "contents".into(),
                    is_error: false,
                }],
            },
            Message::text(3, Role::Assistant, "did stuff"),
            Message::text(4, Role::User, "next"), // last user → span = ids 1..=3
            Message::text(5, Role::Assistant, "ok"),
        ];
        let (tx, _rx) = std::sync::mpsc::channel();

        let out = agent.context_working_set(&tx);
        let sys = out
            .iter()
            .find(|m| m.role == Role::System)
            .and_then(|m| m.items.iter().find_map(|it| match it {
                MessageItem::Text { text } => Some(text.clone()),
                _ => None,
            }))
            .expect("a System summary message should be inserted");
        assert!(sys.contains("SUMMARY-PROSE"), "got: {sys}");
        assert!(sys.contains("<read-files>"), "got: {sys}");
        assert!(sys.contains("foo.rs"), "got: {sys}");

        // Second call with an unchanged span reuses the cache (no panic, same blocks).
        let out2 = agent.context_working_set(&tx);
        assert!(out2.iter().any(|m| m.role == Role::System
            && m.items.iter().any(|it| matches!(it, MessageItem::Text { text } if text.contains("foo.rs")))));

        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 5: 跑测试确认通过 + 全量回归**

Run: `cargo test context_working_set_summarizes_and_appends_file_blocks`
Expected: PASS

Run: `cargo test`
Expected: 全绿（原 105 通过 + 新增用例；4 个 `#[ignore]` 不计）

- [ ] **Step 6: 提交**

```bash
git add src/agent.rs
git commit -m "feat(compaction): structured tier-2 summary, iterative merge, file tracking

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: 文档同步

**Files:**
- Modify: `docs/adr/0023-*.md`（追加"增强说明"小节）
- Modify: `CLAUDE.md`（compaction 段落补一句）
- Read/确认: `ARCHITECTURE.md`（如含 compaction 摘要格式描述则同步；数字无变化）

**Interfaces:**
- Consumes: 无
- Produces: 文档与代码一致。

- [ ] **Step 1: 定位 ADR 0023 文件名**

Run: `ls docs/adr | grep 0023`
Expected: 输出形如 `0023-compaction-context-working-set.md`（记下确切文件名）

- [ ] **Step 2: 在 ADR 0023 末尾追加增强说明**

在该文件末尾追加：

```markdown
## 增强说明（2026-07-15，第一批，非契约变更）

借鉴 pi-mono 的 compaction 实践，在不改变"派生、非破坏"核心立场下增强 tier-2：

- **结构化摘要模板**：`summarize_span` 产出固定小节（目标 / 约束与偏好 / 进展 / 关键决策 / 下一步 / 关键上下文）的散文。
- **迭代式摘要**：span 增长时只摘增量切片，并把上一版摘要作为 `previous` 传入合并，提升连续性、省 token。
- **累积文件追踪**：`collect_file_paths` 跨轮累积 span 内 `read_file`/`write_file`/`edit_file` 的路径，由 `render_file_blocks` 附在摘要末尾的 `<read-files>`/`<modified-files>` 块。
- **tool-result 截断** 由 200 放宽到 2000 字符。

摘要仍不写入持久化 Session；缓存 `Tier2Summary` 为进程内、`/resume` 后重算。
```

- [ ] **Step 3: 更新 CLAUDE.md 的 compaction 段落**

在 CLAUDE.md 中 `Compaction 已全量实现` 那段（`> **Compaction 已全量实现**…`）末尾，追加一句：

```markdown
tier-2 摘要采用结构化模板、迭代式合并(span 增长只摘增量并带入上一版摘要),并累积追踪 read/modified 文件路径附于摘要末尾(见 docs/adr/0023 增强说明)。
```

- [ ] **Step 4: 确认 ARCHITECTURE.md**

Run: `grep -niE "compaction|摘要|working set" ARCHITECTURE.md`
Expected: 若命中描述 tier-2 摘要格式的句子，补一句与 CLAUDE.md 一致的说明；若仅泛述机制则无需改动。

- [ ] **Step 5: 提交**

```bash
git add docs/adr CLAUDE.md ARCHITECTURE.md
git commit -m "docs: record compaction batch-1 enhancements (ADR 0023 addendum)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage：**
- 结构化摘要模板 → Task 4 Step 2 ✓
- 迭代式摘要 → Task 4 Step 3（`Some(c) if c.covered_last_id < covered_last_id` 分支 + `previous`）✓
- 累积文件追踪 → Task 2（`collect_file_paths`）+ Task 4（种子集克隆 + 并入）+ Task 3（渲染）✓
- 截断 200→2000 → Task 1 ✓
- 非目标（不持久化、不动 tier-1/apply_tier2）→ 各任务均未触碰持久化与 `apply_tier2` ✓
- 测试（纯函数单测 + StubProvider 集成）→ Task 1/2/3 单测 + Task 4 集成测试 ✓
- 文档同步 → Task 5 ✓

**2. Placeholder scan：** 无 TBD/TODO；每个代码步骤均给出完整代码与确切命令。✓

**3. Type consistency：**
- `collect_file_paths(span, &mut BTreeSet, &mut BTreeSet)` 在 Task 2 定义、Task 4 两处调用签名一致 ✓
- `render_file_blocks(&BTreeSet, &BTreeSet) -> String` 在 Task 3 定义、Task 4 调用一致 ✓
- `summarize_span(&self, &str, Option<&str>)` 在 Task 4 Step 2 定义、Step 3 两处调用（`Some(&prev)` / `None`）一致 ✓
- `Tier2Summary { covered_last_id, text, read_files, modified_files }` 字段在 Task 4 Step 1 定义、Step 3 构造一致 ✓
- 既有 `covered_last_id` / `anchor_id` / `apply_tier2` 签名不变 ✓
```