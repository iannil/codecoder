# P0-3: write_file 截断修复 — 设计文档

> 7×24 高度自主开发差距 P0-3：当 LLM 响应在 max_tokens 处被截断时，write_file 的 `content` 参数不完整 → 文件写不完整。通过 append 模式 + 截断提示优化让模型能续写。

---

## 现状

agent.rs 已有两层截断保护（迭代 2 / ADR 0038）：
1. **StopReason::Length 检测** — 截断的 tool call 不执行，标记 `is_error` 返回给模型
2. **自适应 max_tokens 翻倍** — 命中 Length 后有效预算翻倍（上限 `max_tokens_ceiling`，默认 32768）
3. **小步写纪律** — system prompt 中注入 `SMALL_STEP_WRITE_GUIDANCE` 提醒模型分批写

但实验仍发生多次截断，根因：模型重试时用相同手法写同一文件，再次被截断，循环 3-4 次后放弃。

## 设计

### 核心思路

给 `write_file` 增加 `append` 模式：当截断发生时，前半部分已被写入磁盘，模型可以在下一轮用 append 模式续写后半部分。

### 修改点

#### 1. `write_file` 工具增加 `append` 参数

```rust
// src/tool/builtin.rs — WriteFile::schema()
"properties": {
    "path": { "type": "string" },
    "content": { "type": "string" },
    "append": { "type": "boolean", "description": "Append to existing file instead of overwriting. Default: false." }
}
```

`run()` 中：`append` 为 true 时用 `std::fs::OpenOptions::new().append(true)` 打开写入。

#### 2. 截断 error 消息增强

```rust
// src/agent.rs — 第 927-930 行
let output = format!(
    "tool call truncated: the response hit max_tokens before the arguments finished. \
     Not executed. The model's reasoning was too long, leaving no room for the write. \
     Retry with a MUCH shorter thought process, or if this was a write_file, consider \
     whether the file was partially created — you can append to it with append=true."
);
```

#### 3. `run_command 2>&1` 的权限通配（P0-4 的附带修复）

不在本 P0-3 范围内，后续处理。

### 修改点汇总

| 文件 | 变更 | 类型 |
|------|------|------|
| `src/tool/builtin.rs` | `WriteFile::schema()` 增加 `append` 参数 | 修改 |
| `src/tool/builtin.rs` | `WriteFile::run()` 处理 `append=true` 分支 | 修改 |
| `src/agent.rs` | 截断 error 消息增加 append 提示 | 修改 |

### 验收标准

1. `write_file(path="x.txt", content="hello", append=true)` 在文件已存在时追加而非覆盖
2. `write_file(path="x.txt", content="hello", append=true)` 在文件不存在时创建新文件（同普通写入）
3. 截断 error 消息包含 append 提示
4. 向后兼容：不传 `append` 参数时行为不变（覆盖写入）

### 边界情况

- append 到空文件 → 同普通写入
- append 到新文件（不存在）→ 创建后写入，同普通写入
- 并发 append → Rust 的 `OpenOptions::append(true)` 在 OS 级别是原子追加，安全
