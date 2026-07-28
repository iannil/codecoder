# P1-4: Token 消耗可见性 — 设计文档

> 7×24 高度自主开发差距 P1-4：BgObserver 有事件流但无 token 消耗指标。修复方式：在 Completion 中携带 usage 数据，经 BgObserver 写入 NDJSON。

---

## 现状

- `Completion`（`src/provider/mod.rs`）只有 `message` 和 `stop_reason`，无法携带 token 消耗数据
- OpenAI response 中包含 `usage` 字段（`prompt_tokens`、`completion_tokens`），但 `from_wire_response()` 未解析
- `BgObserver::emit()` 只接受 `(kind, msg)`，固定 JSON 格式 `{ "kind": ..., "msg": ... }`
- agent turn 循环在 `complete_retrying` 之后拿到 LLM 响应但不记录 token 数据

## 设计

### 修改点

| 文件 | 变更 | 类型 |
|------|------|------|
| `src/provider/mod.rs` | `Completion` 增加 `usage` 可选字段 | 修改 |
| `src/provider/openai.rs` | `from_wire_response()` 解析 `usage` 字段 | 修改 |
| `src/bg_observer.rs` | `emit()` 支持可选 `data` 参数（额外 JSON 字段） | 修改 |
| `src/agent.rs` | 在 LLM 调用后通过 `BgObserver` 记录 token 数据 | 修改 |

### 具体变更

**Completion 结构体：**
```rust
#[derive(Debug, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug)]
pub struct Completion {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Option<Usage>,
}
```

**from_wire_response()：**
```rust
fn from_wire_response(json: &Value) -> anyhow::Result<Completion> {
    // ... 现有解析逻辑不变 ...
    let usage = json.get("usage").and_then(|u| {
        Some(Usage {
            prompt_tokens: u.get("prompt_tokens")?.as_u64()? as u32,
            completion_tokens: u.get("completion_tokens")?.as_u64()? as u32,
        })
    });
    // ... 返回时携带 usage
}
```

**BgObserver::emit()：** 增加 `data` 可选参数，NDJSON 行增加 `data` 字段。
```rust
pub fn emit_with_data(&mut self, kind: &str, msg: &str, data: Option<serde_json::Value>) {
    // ...
    let line = if let Some(d) = data {
        json!({ "kind": kind, "msg": msg, "data": d })
    } else {
        json!({ "kind": kind, "msg": msg })
    };
}
```

**agent.rs LLM 调用后记录：**
```rust
let (reply, stop_reason, usage) = match self.complete_retrying(&req, event_tx) {
    Ok(c) => (c.message, c.stop_reason, c.usage),
    // ...
};
// ... 在 tool_loop 中记录 token
if let Some(u) = &usage {
    self.bg_observer.emit("llm_call", &format!(
        "prompt_tokens={} completion_tokens={}", u.prompt_tokens, u.completion_tokens
    ));
}
```

### NDJSON 格式

```
{"kind":"llm_call","msg":"prompt_tokens=1234 completion_tokens=567"}
{"kind":"tool_started","msg":"read_file"}
{"kind":"milestone_start","msg":"#1 Core data model"}
{"kind":"llm_call","msg":"prompt_tokens=2345 completion_tokens=678"}
{"kind":"gate","msg":"#1 pass"}
```

### 验收标准

1. NDJSON 中的 `llm_call` 事件包含 `prompt_tokens` 和 `completion_tokens`
2. 现有事件格式不变（向后兼容）
3. StubClient 返回的 Completion 不含 usage（Option::None），不报错
