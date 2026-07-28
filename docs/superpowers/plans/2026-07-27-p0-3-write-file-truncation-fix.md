# P0-3: write_file 截断修复 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `write_file` 增加 `append` 模式，使截断后模型能用 append 续写，同时增强截断 error 消息提示。

**Architecture:** 修改 `write_file` 工具支持 `append` 参数；在截断 error 中注入 append 提示。

**Tech Stack:** Rust, 同一进程内工具调用。

## 全局约束

- `write_file` 的 `append` 参数默认 false，向后兼容
- `append=true` 时文件不存在则创建，存在则追加
- 截断 error 消息包含 append 提示
- 测试覆盖 append 为 true/false 两种情况

---

### Task 1: WriteFile schema 增加 append 参数

**Files:**
- Modify: `src/tool/builtin.rs` — WriteFile::schema()

**Interfaces:**
- Consumes: 无
- Produces: WriteFile schema 包含 `append` 布尔字段

- [ ] **Step 1: 修改 WriteFile::schema()**

```rust
fn schema(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "content": { "type": "string" },
            "append": { "type": "boolean", "description": "Append to existing file instead of overwriting. Default: false." }
        },
        "required": ["path", "content"]
    })
}
```

- [ ] **Step 2: 添加测试：schema 包含 append 字段**

```rust
#[test]
fn write_file_schema_includes_append() {
    let tool = WriteFile;
    let schema = tool.schema();
    let props = schema.get("properties").unwrap();
    assert!(props.get("append").is_some(), "schema should include append property");
}
```

- [ ] **Step 3: 运行测试确认通过**

```bash
cargo test write_file_schema_includes_append -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/tool/builtin.rs
git commit -m "feat: add append parameter to write_file schema"
```

---

### Task 2: WriteFile::run() 实现 append 模式

**Files:**
- Modify: `src/tool/builtin.rs` — WriteFile::run()

**Interfaces:**
- Consumes: `append` 参数（boolean, 可选）
- Produces: WriteFile::run() 在 append=true 时使用 OpenOptions::append(true)

- [ ] **Step 1: 修改 WriteFile::run()**

原代码（第 272-286 行）：
```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
    let content = args.get("content").and_then(Value::as_str).unwrap_or_default();
    if path.is_empty() {
        return Ok(ToolOutput::err("missing required arg: path"));
    }
    let full = ctx.root.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::write(&full, content) {
        Ok(()) => Ok(ToolOutput::ok(format!("wrote {} bytes to {}", content.len(), path))),
        Err(e) => Ok(ToolOutput::err(format!("cannot write {}: {e}", full.display()))),
    }
}
```

改为：
```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
    let content = args.get("content").and_then(Value::as_str).unwrap_or_default();
    let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
    if path.is_empty() {
        return Ok(ToolOutput::err("missing required arg: path"));
    }
    let full = ctx.root.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let result = if append {
        use std::fs::OpenOptions;
        use std::io::Write;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&full)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map(|_| content.len())
    } else {
        std::fs::write(&full, content).map(|_| content.len())
    };
    match result {
        Ok(bytes) => {
            let action = if append { "appended" } else { "wrote" };
            Ok(ToolOutput::ok(format!("{action} {} bytes to {}", bytes, path)))
        }
        Err(e) => Ok(ToolOutput::err(format!("cannot write {}: {e}", full.display()))),
    }
}
```

- [ ] **Step 2: 添加测试**

```rust
#[test]
fn write_file_append_mode_appends() {
    let dir = std::env::temp_dir().join(format!("cc_append_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = ToolCtx::new(&dir);
    // 先写入初始内容
    WriteFile.run(json!({"path": "test.txt", "content": "hello "}), &mut ctx).unwrap();
    // append 追加
    let out = WriteFile.run(json!({"path": "test.txt", "content": "world", "append": true}), &mut ctx).unwrap();
    assert!(!out.is_error, "append should succeed: {}", out.content);
    let content = std::fs::read_to_string(dir.join("test.txt")).unwrap();
    assert_eq!(content, "hello world", "append should concatenate content");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_file_append_creates_new_file() {
    let dir = std::env::temp_dir().join(format!("cc_append_new_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = ToolCtx::new(&dir);
    // 文件不存在时 append 应创建
    let out = WriteFile.run(json!({"path": "new.txt", "content": "created", "append": true}), &mut ctx).unwrap();
    assert!(!out.is_error, "append to new file should succeed: {}", out.content);
    let content = std::fs::read_to_string(dir.join("new.txt")).unwrap();
    assert_eq!(content, "created");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_file_overwrite_by_default() {
    let dir = std::env::temp_dir().join(format!("cc_overwrite_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = ToolCtx::new(&dir);
    WriteFile.run(json!({"path": "test.txt", "content": "original"}), &mut ctx).unwrap();
    // 默认 append=false，应覆盖
    WriteFile.run(json!({"path": "test.txt", "content": "replaced"}), &mut ctx).unwrap();
    let content = std::fs::read_to_string(dir.join("test.txt")).unwrap();
    assert_eq!(content, "replaced", "default should overwrite");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_file_permission_requires_ask() {
    use std::path::Path;
    let tool = WriteFile;
    match tool.permission(&json!({"path": "x"}), Path::new(".")) {
        Permission::Ask { key } => assert_eq!(key, "write_file"),
        _ => panic!("expected Ask"),
    }
}
```

- [ ] **Step 3: 运行测试确认通过**

```bash
cargo test write_file -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/tool/builtin.rs
git commit -m "feat: implement append mode for write_file"
```

---

### Task 3: 增强截断 error 消息

**Files:**
- Modify: `src/agent.rs` — 第 927-930 行

**Interfaces:**
- Consumes: 无
- Produces: 截断 error 消息包含 append 提示

- [ ] **Step 1: 修改截断 error 消息**

原代码（第 927-930 行）：
```rust
let output = "tool call truncated: the response hit max_tokens before the \
     arguments finished; not executed. Retry with a shorter response or \
     split the work."
    .to_string();
```

改为：
```rust
let output = "tool call truncated: the response hit max_tokens before the \
     arguments finished. Not executed. The model's reasoning was too long, \
     leaving no room for the file content. Retry with a MUCH shorter thought \
     process, or if this was a write_file, consider whether the file was \
     partially created — you can append to it with append=true."
    .to_string();
```

- [ ] **Step 2: 添加测试**

在 `src/agent.rs` 测试模块中添加（或在集成测试中验证）：

```rust
#[test]
fn truncated_tool_call_error_mentions_append() {
    let err_msg = "tool call truncated: the response hit max_tokens before the \
     arguments finished. Not executed. The model's reasoning was too long, \
     leaving no room for the file content. Retry with a MUCH shorter thought \
     process, or if this was a write_file, consider whether the file was \
     partially created — you can append to it with append=true.";
    assert!(err_msg.contains("append=true"), "error should mention append=true");
}
```

- [ ] **Step 3: 运行测试确认通过**

```bash
cargo test truncated_tool_call_error_mentions_append -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/agent.rs
git commit -m "feat: enhance truncated tool call error with append hint"
```

---

### Task 4: 编译验证

- [ ] **Step 1: 完整编译**

```bash
cargo build 2>&1
```

- [ ] **Step 2: 运行全部测试**

```bash
cargo test 2>&1
```

- [ ] **Step 3: 提交最终版本**

```bash
git add -A
git commit -m "feat: P0-3 write_file truncation fix with append mode"
```