# Spec: length 截断时的工具调用保护(路线图 #1)

对应 [[0027-pi-comparison-and-borrowing-roadmap]] Wave 0 #1。借鉴 pi 的
`failToolCallsFromTruncatedMessage`(`archived/pi/packages/agent/src/agent-loop.ts`)。

## 问题

`Provider::complete` 返回 `Message`,**不携带 stop reason**;`from_wire_response`
(`src/provider/openai.rs:125`)直接丢弃 `choices[0].finish_reason`。当 OpenAI 在
`max_tokens` 处把一次**含 tool_call 的响应截断**时,`function.arguments` 是一段被腰斩
的 JSON。当前解析 `serde_json::from_str(s).ok().unwrap_or(Value::Null)`
(`openai.rs:150`)有两种坏结局:

1. 解析失败 → `args = Value::Null`,工具拿到空参数照跑;
2. 恰好解析成一个**合法但不完整**的对象(如 `write_file` 的 `content` 被截半)→
   工具用错误参数执行,可能写坏文件、跑错命令。

而 loop(`src/agent.rs:479`)对 `tool_calls` 无条件逐个 `dispatch_tool`。这是一个静默
正确性 bug。

## 设计

**1. provider 层surface stop reason.** 新增(`src/provider/mod.rs`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason { Stop, Length, ToolCalls, Other }

pub struct Completion { pub message: Message, pub stop_reason: StopReason }
```

`Provider::complete` 返回类型由 `anyhow::Result<Message>` 改为
`anyhow::Result<Completion>`。改动面很小——只有 2 个实现(`openai`、`stub`)与 2 个调用点
(`process_turn` 与 `summarize_span`)。

- `from_wire_response` 读 `choices[0].finish_reason`,映射:`"length"→Length`、
  `"tool_calls"→ToolCalls`、`"stop"→Stop`、其余 `→Other`。
- `StubClient` 返回 `StopReason::Stop`(有 tool_call 时 `ToolCalls`),保持既有测试语义。
- `summarize_span`(`agent.rs:317`)只取 `.message`,忽略 stop reason。

**2. loop 里加截断保护.** 在 `process_turn`(`agent.rs` 收集 `tool_calls` 之后、
`for … dispatch_tool` 之前)插入:

```
if stop_reason == StopReason::Length && !tool_calls.is_empty() {
    // 已 append 的 assistant 原样保留(记录这次截断的 turn)
    let results = tool_calls.iter().map(|(id, _, _)| MessageItem::ToolResult {
        call_id: id.clone(),
        output: "tool call truncated: the response hit max_tokens before the \
                 arguments finished; not executed. Retry with a shorter response \
                 or split the work.".into(),
        is_error: true,
    }).collect();
    // 每个都发 ToolFinished{is_error} 以便事件流可观测(与 headless denial 一致)
    self.append(Role::Tool, results);
    continue;   // 不 dispatch;回喂错误,让模型重试
}
```

关键点:**整批**失败(不是只失败最后一个)——截断只保证最后一个 tool_call 不完整,但
provider 可能已并入多个,统一失败最安全,与 pi 一致。`continue` 而非 `break`,使模型看到
错误后自行重试(缩短输出或拆分)。

## 边界

- **无 tool_call 的纯文本截断**:`tool_calls.is_empty()` 时维持现状(当前会 `break`,turn
  以部分文本收尾)。本 spec **不处理**该情况,列为范围外(可后续加一条 Notice 提示用户
  调大 `CODECODER_MAX_TOKENS`)。
- codecoder 是**非流式**(`complete` 返回整条 Message),不存在 pi 那种流式 salvage-parse
  的额外复杂度——本修复因此比 pi 的更简单。

## 测试

- `openai.rs` 单测:`from_wire_response` 对 `finish_reason:"length"` 映射为
  `StopReason::Length`;对 `"tool_calls"`、`"stop"` 分别映射。
- `agent.rs` 单测:新增 `StubClient` 变体,第一次调用返回一个带 `StopReason::Length` 的
  tool_call。断言:(a) 该工具**未被 dispatch**(用一个会 panic/置标志的假工具证明未执行);
  (b) 会话里 append 了 `is_error:true` 的 ToolResult;(c) loop `continue` 后第二次调用可
  正常收尾。
- 全量 `cargo test` 回归(现有 112 测试不受 `Completion` 改型影响,因调用点仅 2 处)。

## 范围外

流式截断、纯文本截断的用户提示、`max_tokens` 自适应调整。
