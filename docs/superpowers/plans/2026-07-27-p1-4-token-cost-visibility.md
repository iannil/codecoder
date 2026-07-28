# P1-4: Token 消耗可见性 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** 在 BG 运行的 NDJSON 事件流中增加每轮 LLM 调用的 token 消耗数据（prompt_tokens / completion_tokens）。

**Architecture:** 在 `Completion` 结构体中增加 `usage` 字段；OpenAI response 解析时提取 `usage`；BgObserver 增加带 data 的 emit；agent 循环记录 LLM 调用 token。

**Tech Stack:** Rust, serde_json, 4 个文件。

## 全局约束

- `Usage` 结构体放在 `src/provider/mod.rs` 中
- `Completion::usage` 为 `Option<Usage>`（StubClient 返回 None）
- `BgObserver::emit_with_data()` 是新增方法，不修改已有 `emit()` 签名
- agent 侧的 LLM 调用记录只发生在 background 模式（非交互模式）
- NDJSON 行增加 `data` 字段，不破坏现有解析逻辑

---

### Task 1: Completion 增加 usage 字段

**Files:**
- Modify: `src/provider/mod.rs`

**Interfaces:**
- Produces: `Usage { prompt_tokens: u32, completion_tokens: u32 }`, `Completion::usage: Option<Usage>`

- [ ] **Step 1: 在 mod.rs 中增加 Usage 结构体 + Completion.usage 字段**

```rust
/// Token usage for a single LLM completion call.
#[derive(Debug, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// Completion 增加 usage 字段
#[derive(Debug)]
pub struct Completion {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Option<Usage>,
}
```

- [ ] **Step 2: 更新 StubClient 返回 Completion 时带 usage=None**

```rust
// stub.rs 中的 complete() 方法
Ok(Completion { message, stop_reason, usage: None })
```

- [ ] **Step 3: 更新 FallbackProvider 透传 usage**

```rust
fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
    match self.primary.complete(req) {
        Ok(c) => Ok(c),
        Err(e) => self.fallback.complete(req)
    }
}
```

- [ ] **Step 4: 编译确认**

```bash
cargo build 2>&1 | tail -3
```

- [ ] **Step 5: 提交**

```bash
git add src/provider/mod.rs src/provider/stub.rs
git commit -m "feat: add Usage struct and Completion.usage field"
```

---

### Task 2: OpenAI response 解析 usage

**Files:**
- Modify: `src/provider/openai.rs` — `from_wire_response()`

- [ ] **Step 1: 在 from_wire_response() 中解析 usage**

```rust
fn from_wire_response(json: &Value) -> anyhow::Result<Completion> {
    // ... 现有解析逻辑不变 ...
    
    let usage = json.get("usage").and_then(|u| {
        Some(crate::provider::Usage {
            prompt_tokens: u.get("prompt_tokens")?.as_u64()? as u32,
            completion_tokens: u.get("completion_tokens")?.as_u64()? as u32,
        })
    });

    let message = Message { id: 0, role: Role::Assistant, items };
    Ok(Completion { message, stop_reason, usage })
}
```

- [ ] **Step 2: 添加测试**

```rust
#[test]
fn from_wire_response_parses_usage() {
    let resp = json!({
        "choices": [{
            "finish_reason": "stop",
            "message": { "content": "hello" }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60 }
    });
    let c = from_wire_response(&resp).unwrap();
    let u = c.usage.expect("usage should be parsed");
    assert_eq!(u.prompt_tokens, 50);
    assert_eq!(u.completion_tokens, 10);
}

#[test]
fn from_wire_response_missing_usage_is_none() {
    let resp = json!({
        "choices": [{
            "finish_reason": "stop",
            "message": { "content": "hello" }
        }]
    });
    let c = from_wire_response(&resp).unwrap();
    assert!(c.usage.is_none(), "no usage in response -> None");
}
```

- [ ] **Step 3: 编译 + 测试**

```bash
cargo build && cargo test openai -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/provider/openai.rs
git commit -m "feat: parse usage from OpenAI response"
```

---

### Task 3: BgObserver 支持额外 data 参数

**Files:**
- Modify: `src/bg_observer.rs`

- [ ] **Step 1: 新增 emit_with_data() 方法**

```rust
impl BgObserver {
    /// Emit one event with optional structured data (extra JSON fields).
    pub fn emit_with_data(&mut self, kind: &str, msg: &str, data: Option<serde_json::Value>) {
        eprintln!("[bg] {kind}: {msg}");
        if let Some(f) = self.ndjson.as_mut() {
            let line = if let Some(d) = data {
                let mut obj = serde_json::json!({ "kind": kind, "msg": msg });
                if let Some(obj_map) = obj.as_object_mut() {
                    if let Some(data_obj) = d.as_object() {
                        for (k, v) in data_obj {
                            obj_map.insert(k.clone(), v.clone());
                        }
                    }
                }
                obj.to_string()
            } else {
                serde_json::json!({ "kind": kind, "msg": msg }).to_string()
            };
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}
```

- [ ] **Step 2: 添加测试**

```rust
#[test]
fn emit_with_data_includes_extra_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut obs = BgObserver::new(dir.path());
    obs.emit_with_data("llm_call", "done", Some(json!({"prompt_tokens": 100, "completion_tokens": 50})));
    let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
    let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(v["kind"], "llm_call");
    assert_eq!(v["prompt_tokens"], 100);
    assert_eq!(v["completion_tokens"], 50);
}

#[test]
fn emit_with_data_no_data_is_identical_to_emit() {
    let dir = tempfile::tempdir().unwrap();
    let mut obs = BgObserver::new(dir.path());
    obs.emit_with_data("tool_started", "run_command", None);
    let body = std::fs::read_to_string(dir.path().join(".ccd.bg.ndjson")).unwrap();
    let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(v["kind"], "tool_started");
    assert_eq!(v["msg"], "run_command");
    assert!(v.get("prompt_tokens").is_none(), "no extra fields when data=None");
}
```

- [ ] **Step 3: 测试**

```bash
cargo test bg_observer -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add src/bg_observer.rs
git commit -m "feat: add emit_with_data to BgObserver for structured token data"
```

---

### Task 4: agent 循环记录 LLM 调用 token

**Files:**
- Modify: `src/agent.rs`

- [ ] **Step 1: 在 process_turn 的 LLM 调用处记录 token 到 BgObserver**

在 `process_turn` 中，`complete_retrying` 返回后（第 882 行），获取 usage 并记录。

需要给 AgentLoop 添加一个 `BgObserver` 引用，或者通过 event_tx 发送 token 数据。简单方案：在 background 模式下，通过 event_tx 发送一个包含 usage 的 Notice 事件，由 bg_observer 的 drain 逻辑捕获。

更直接的方案：在 `process_turn` 中检测 background 模式，直接通过 BgObserver 记录。

由于 `AgentLoop` 目前没有 BgObserver 引用，最简单的方案是在 `process_turn` 的 LLM 调用处发送一个新的 AgentEvent 变体：

```rust
// AgentEvent 增加
TokenUsage { prompt_tokens: u32, completion_tokens: u32 },
```

然后在 `drain_bg_events`（`background.rs`）中处理这个事件，调用 `obs.emit_with_data()`。

```rust
AgentEvent::TokenUsage { prompt_tokens, completion_tokens } => {
    obs.emit_with_data("llm_call", "tokens", Some(json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
    })));
}
```

**修改点：**
1. `src/agent.rs` — AgentEvent 增加 `TokenUsage` 变体
2. `src/agent.rs` — process_turn 中 `complete_retrying` 后发送 `TokenUsage` 事件
3. `src/background.rs` — drain_bg_events 中处理 `TokenUsage` 事件

- [ ] **Step 2: 测试验证**

```bash
cargo build && cargo test background -- --nocapture
```

- [ ] **Step 3: 提交**

```bash
git add src/agent.rs src/background.rs
git commit -m "feat: record LLM token usage in background NDJSON events"
```

---

### Task 5: 编译验证

- [ ] **Step 1: 完整编译**

```bash
cargo build 2>&1
```

- [ ] **Step 2: 全量测试**

```bash
cargo test 2>&1
```

- [ ] **Step 3: 提交最终版本**

```bash
git add -A && git commit -m "feat: P1-4 token cost visibility in background NDJSON events"
```