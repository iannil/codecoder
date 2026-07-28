# P0-2: headless plan 拒绝修复 — 设计文档

> 7×24 高度自主开发差距 P0-2：headless 模式下 `plan` 工具因需要用户确认被拒绝。修复方式：headless 模式下自动批准 plan。

---

## 现状

`dispatch_tool()` 中两处拦截：

1. **第 1105 行** — `self.headless && name == "plan"` → 直接 denied（返回 error）
2. **第 1123 行** — 即使绕过第 1 步，进入 `self.plan()` → 发送 `PlanApproval` 事件 → 无用户确认 → 卡住

## 设计

### 方案

在 `self.plan()` 中增加 headless 分支：当 `self.headless == true` 时，自动批准 plan 并写入 PLAN.md，不发送 `PlanApproval` 事件。

同时去掉 `dispatch_tool()` 中对 plan 的 headless 拦截（第 1105 行），让 plan 正常走 `self.plan()` 方法。

### 修改点

| 文件 | 变更 | 类型 |
|------|------|------|
| `src/agent.rs` | `dispatch_tool()` 第 1105 行：从 headless 拦截列表中移除 `"plan"` | 修改 |
| `src/agent.rs` | `plan()` 方法增加 headless 自动批准分支 | 修改 |

### 具体代码变更

**dispatch_tool() 第 1105 行：**
```rust
// 改前:
if self.headless && (name == "ask_user" || name == "confirm" || name == "plan") {
// 改后:
if self.headless && (name == "ask_user" || name == "confirm") {
```

**plan() 方法增加 headless 分支：**
```rust
fn plan(&mut self, call_id: &str, args: &serde_json::Value, event_tx: &Sender<AgentEvent>) -> ToolOutcome {
    let text = ...; // 现有 plan 文本提取逻辑不变
    
    if text.is_empty() {
        return ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output: "provide `steps` or `plan`".into(),
            is_error: true,
        });
    }

    // headless 模式：自动批准，不发送 PlanApproval
    if self.headless {
        let _ = std::fs::write(self.root.join("PLAN.md"), format!("# Plan\n\n{text}\n"));
        return ToolOutcome::Result(MessageItem::ToolResult {
            call_id: call_id.to_string(),
            output: "plan approved (headless) and recorded to PLAN.md".to_string(),
            is_error: false,
        });
    }

    // 交互模式：发送 PlanApproval 等待用户确认（现有逻辑不变）
    let (reply_tx, reply_rx) = channel();
    let _ = event_tx.send(AgentEvent::PlanApproval { plan: text.clone(), reply_tx });
    let approved = reply_rx.recv().unwrap_or(false);
    ...
}
```

### 验收标准

1. headless 模式下 `plan` 工具不再被 denied
2. headless 模式下 `plan` 自动批准并写入 PLAN.md
3. 交互模式下 `plan` 行为不变（仍发送 PlanApproval 等待用户确认）
4. ask_user 和 confirm 在 headless 模式下仍被 denied（行为不变）