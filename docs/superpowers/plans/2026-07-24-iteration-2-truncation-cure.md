# 迭代 2：截断根治（自适应 max_tokens）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** headless/交互 turn 命中 `StopReason::Length`（max_tokens 截断）时，自动上调该 turn 的有效 max_tokens（翻倍、封顶）重试，复用现有 guard，配合提高的默认值与一行小步写引导，消灭「大文件写截断」这一头号杀手。

**Architecture:** 在 `AgentLoop` 的 turn 工具迭代循环里用局部 `effective_max_tokens`（每 turn 从 `self.max_tokens` 重置）替代固定 `self.max_tokens` 构造 `CompletionRequest`；命中 `Length` 时翻倍到 `self.max_tokens_ceiling` 封顶再 `continue` 重试，并把 `Length` 判定提到 `tool_calls.is_empty()` 收尾之前（修静默收尾缺口）。`max_tokens` 默认 4096→8192，新增 `CODECODER_MAX_TOKENS_CEILING`（默认 32768），ceiling 在 `build` 内由 `Config::from_env()` 注入以覆盖所有构造点。

**Tech Stack:** Rust（无新依赖）；现有 `Provider`/`CompletionRequest`/`StopReason` 不变；hermetic 测试用记录 `req.max_tokens` 的有状态测试 Provider。

## Global Constraints

- 不新增任何 crate 依赖。
- TDD：先写失败测试再实现。所有测试 hermetic（`tempdir` + 测试 Provider，无网络/真实 LLM）。
- 不改 `Provider` trait、不改 `CompletionRequest`/`Completion`/`StopReason` 定义。
- 不拼接半写文件；`Length` + tool_calls 时**保留现有 guard**（追加 is_error 结果、绝不执行半序列化 tool call）。
- 数值：`max_tokens` 默认 **8192**；`CODECODER_MAX_TOKENS_CEILING` 默认 **32768**；命中 `Length` 时 `effective = effective.saturating_mul(2).min(ceiling)`。
- `effective_max_tokens` **每 turn 重置**为 `self.max_tokens`，不跨 turn 累积。
- 术语精确（CONTEXT.md）：turn / tool call / max_tokens / StopReason。

---

## 关键现状锚点

- `src/config.rs:35` `max_tokens` 默认 `unwrap_or(4096)`；数值字段解析模式 `.and_then(|v| v.parse().ok()).unwrap_or(N)`。
- `src/agent.rs:203` 结构体字段区；`build()`（283–358）是唯一初始化点，已在其中读 env（`trust::default_trust`）。`set_tool_cap`（390）是「Config 派生的 per-run 可调项用 setter」的既有范式。
- `src/agent.rs:798` `for _ in 0..self.tool_cap {`（turn 工具迭代循环，位于 `process_turn`）。
- `src/agent.rs:833-839` 构造 `CompletionRequest`（`max_tokens: self.max_tokens`）。
- `src/agent.rs:877-885` `tool_calls.is_empty()` 收尾 break（含 steering）。
- `src/agent.rs:887-913` 现有 `Length` guard（neutralize 半成品 + continue），**位于收尾 break 之后**——需提前。
- `src/agent.rs:1400` `build_system_prompt_with_catalog`（`parts` 拼装）；`1422` `build_system_prompt`。
- 现有截断测试 `truncated_tool_call_is_not_executed_and_loop_recovers`（agent.rs ~1846）+ 测试 Provider `TruncatedToolCall`（~1818）。

---

## Task 1: config — 默认 8192 + max_tokens_ceiling

**Files:**
- Modify: `src/config.rs`（`Config` 结构体、`from_env`、`max_tokens` 默认、测试）

**Interfaces:**
- Produces: `Config.max_tokens` 默认 8192；`Config.max_tokens_ceiling: u32`（env `CODECODER_MAX_TOKENS_CEILING`，默认 32768）。

- [ ] **Step 1: 写失败测试**（加到 `src/config.rs` `mod tests`）

```rust
#[test]
fn max_tokens_default_is_8192() {
    unsafe { std::env::remove_var("CODECODER_MAX_TOKENS"); }
    assert_eq!(Config::from_env().max_tokens, 8192);
}

#[test]
fn max_tokens_ceiling_default_and_override() {
    unsafe { std::env::remove_var("CODECODER_MAX_TOKENS_CEILING"); }
    assert_eq!(Config::from_env().max_tokens_ceiling, 32768);
    unsafe { std::env::set_var("CODECODER_MAX_TOKENS_CEILING", "16384"); }
    assert_eq!(Config::from_env().max_tokens_ceiling, 16384);
    unsafe { std::env::remove_var("CODECODER_MAX_TOKENS_CEILING"); }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib config::tests::max_tokens_ceiling_default_and_override config::tests::max_tokens_default_is_8192`
（若一次只接受一个过滤名，分两次跑。）
Expected: FAIL — `no field max_tokens_ceiling` / 默认值为 4096。

- [ ] **Step 3: 改默认值**（`src/config.rs:35` 区域）

把
```rust
            max_tokens: env("CODECODER_MAX_TOKENS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4096),
```
改为 `.unwrap_or(8192),`。

- [ ] **Step 4: 加 ceiling 字段**（`Config` 结构体，`max_tokens: u32,` 之后）

```rust
    /// 自适应截断根治:命中 StopReason::Length 时,单 turn 有效 max_tokens 翻倍上调的封顶值
    /// (迭代 2)。env CODECODER_MAX_TOKENS_CEILING,默认 32768。
    pub max_tokens_ceiling: u32,
```

- [ ] **Step 5: 加 ceiling 解析**（`from_env`，`max_tokens: …` 块之后）

```rust
            max_tokens_ceiling: env("CODECODER_MAX_TOKENS_CEILING")
                .and_then(|v| v.parse().ok())
                .unwrap_or(32768),
```

- [ ] **Step 6: 修补其它 `Config { … }` 字面量**

Run: `cargo build --tests`
Expected: 若报 `missing field max_tokens_ceiling`（如 `src/daemon/mod.rs` 的测试 Config 字面量），在报错处补 `max_tokens_ceiling: 32768,`。修到通过。

- [ ] **Step 7: 跑测试确认通过**

Run: `cargo test --lib config::tests`
Expected: PASS（含既有 config 测试）。

- [ ] **Step 8: 提交**

```bash
git add src/config.rs src/daemon/mod.rs
git commit -m "feat(config): max_tokens default 8192 + CODECODER_MAX_TOKENS_CEILING (32768)"
```

---

## Task 2: AgentLoop ceiling 字段 + setter（build 内由 env 注入）

**Files:**
- Modify: `src/agent.rs`（`AgentLoop` 字段、`build` 初始化、`set_max_tokens_ceiling` setter、测试）

**Interfaces:**
- Consumes: `Config.max_tokens_ceiling`（Task 1）。
- Produces: `AgentLoop.max_tokens_ceiling: u32`（build 内初始化为 `Config::from_env().max_tokens_ceiling`）；`pub fn set_max_tokens_ceiling(&mut self, n: u32)`。

- [ ] **Step 1: 写失败测试**（加到 `src/agent.rs` `mod tests`）

```rust
#[test]
fn max_tokens_ceiling_defaults_and_setter_overrides() {
    let dir = tempdir().unwrap();
    let mut agent = AgentLoop::new(stub_provider(), "m", 256, 0.0, dir.path().to_path_buf());
    // 默认来自 Config::from_env()(env 未设 → 32768)。
    assert_eq!(agent.max_tokens_ceiling, 32768);
    agent.set_max_tokens_ceiling(1024);
    assert_eq!(agent.max_tokens_ceiling, 1024);
}
```

（`stub_provider()` / `tempdir()` 已在该测试模块使用——沿用现有 import；若 `stub_provider` 不可见，用 `Arc::new(StubClient) as Arc<dyn Provider>`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib agent::tests::max_tokens_ceiling_defaults_and_setter_overrides`
Expected: FAIL — `no field max_tokens_ceiling`。

- [ ] **Step 3: 加字段**（`src/agent.rs` 结构体，`max_tokens: u32,`（203）之后）

```rust
    /// 自适应截断根治(迭代 2):命中 StopReason::Length 时,单 turn 有效 max_tokens
    /// 翻倍上调的封顶值。build 内由 Config::from_env() 注入,故所有构造点(交互/BG/
    /// daemon/sub-agent/verify)统一遵守 CODECODER_MAX_TOKENS_CEILING。
    max_tokens_ceiling: u32,
```

- [ ] **Step 4: 在 build 初始化字段**（`src/agent.rs` `build`，`Self { … }` 字面量里，`max_tokens,` 之后）

```rust
            max_tokens_ceiling: crate::config::Config::from_env().max_tokens_ceiling,
```

- [ ] **Step 5: 加 setter**（紧邻 `set_tool_cap`，agent.rs:390 附近）

```rust
    /// 覆盖自适应截断的 max_tokens 封顶(测试/特殊场景)。默认由 build 从 env 注入。
    pub fn set_max_tokens_ceiling(&mut self, n: u32) {
        self.max_tokens_ceiling = n;
    }
```

- [ ] **Step 6: 跑测试确认通过 + 无回归**

Run: `cargo test --lib agent::tests::max_tokens_ceiling_defaults_and_setter_overrides && cargo test --lib agent::tests`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/agent.rs
git commit -m "feat(agent): AgentLoop max_tokens_ceiling field + setter (env-injected in build)"
```

---

## Task 3: 自适应 bump 核心（turn 循环）+ Length 提前

**Files:**
- Modify: `src/agent.rs`（`process_turn` turn 循环：`effective_max_tokens`、请求、Length 处理重排）
- Test: `src/agent.rs`（`#[cfg(test)]`）

**Interfaces:**
- Consumes: `self.max_tokens`、`self.max_tokens_ceiling`（Task 2）、`StopReason::Length`。
- Produces（测试专用）: `RecordingLengthProvider { fail_times: usize, calls: Mutex<usize>, seen_max_tokens: Mutex<Vec<u32>> }`——前 `fail_times` 次返回 `StopReason::Length`（空 tool_calls），其后返回普通 `Stop`；每次 `complete` 把 `req.max_tokens` 记入 `seen_max_tokens`。

- [ ] **Step 1: 写失败测试**（加到 `src/agent.rs` `mod tests`）

```rust
use std::sync::Mutex as StdMutex;

/// 记录每次请求的 max_tokens;前 fail_times 次以 Length 截断(空 tool_calls),其后正常 Stop。
struct RecordingLengthProvider {
    fail_times: usize,
    calls: StdMutex<usize>,
    seen_max_tokens: StdMutex<Vec<u32>>,
}
impl Provider for RecordingLengthProvider {
    fn name(&self) -> &str { "recording-length" }
    fn complete(&self, req: &CompletionRequest) -> anyhow::Result<Completion> {
        self.seen_max_tokens.lock().unwrap().push(req.max_tokens);
        let mut c = self.calls.lock().unwrap();
        let i = *c; *c += 1;
        let msg = Message {
            id: 0, role: Role::Assistant,
            items: vec![MessageItem::Text { text: if i < self.fail_times { "partial".into() } else { "done".into() } }],
        };
        let stop = if i < self.fail_times { StopReason::Length } else { StopReason::Stop };
        Ok(Completion { message: msg, stop_reason: stop })
    }
}

#[test]
fn length_stop_bumps_effective_max_tokens_on_retry() {
    let dir = tempdir().unwrap();
    let p = Arc::new(RecordingLengthProvider {
        fail_times: 2, calls: StdMutex::new(0), seen_max_tokens: StdMutex::new(vec![]),
    });
    let mut agent = AgentLoop::new(p.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
    agent.set_max_tokens_ceiling(4096);
    let (tx, _rx) = std::sync::mpsc::channel();
    agent.run_one_turn("go".into(), &tx);
    let seen = p.seen_max_tokens.lock().unwrap().clone();
    // 256 → 截断 → 512 → 截断 → 1024 → Stop。翻倍链可见。
    assert_eq!(seen, vec![256, 512, 1024], "seen={seen:?}");
}

#[test]
fn effective_max_tokens_caps_at_ceiling() {
    let dir = tempdir().unwrap();
    // 恒截断(fail_times 极大),空 tool_calls → 达 ceiling 后收尾。
    let p = Arc::new(RecordingLengthProvider {
        fail_times: 99, calls: StdMutex::new(0), seen_max_tokens: StdMutex::new(vec![]),
    });
    let mut agent = AgentLoop::new(p.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
    agent.set_max_tokens_ceiling(1024);
    let (tx, _rx) = std::sync::mpsc::channel();
    agent.run_one_turn("go".into(), &tx);
    let seen = p.seen_max_tokens.lock().unwrap().clone();
    // 256 → 512 → 1024(封顶,空 tool_calls → 不再翻倍,收尾)。绝不超过 1024。
    assert_eq!(seen, vec![256, 512, 1024], "seen={seen:?}");
    assert!(seen.iter().all(|&m| m <= 1024));
}

#[test]
fn bump_resets_per_turn() {
    let dir = tempdir().unwrap();
    let p = Arc::new(RecordingLengthProvider {
        fail_times: 1, calls: StdMutex::new(0), seen_max_tokens: StdMutex::new(vec![]),
    });
    let mut agent = AgentLoop::new(p.clone() as Arc<dyn Provider>, "m", 256, 0.0, dir.path().to_path_buf());
    agent.set_max_tokens_ceiling(4096);
    let (tx, _rx) = std::sync::mpsc::channel();
    agent.run_one_turn("t1".into(), &tx); // 256 → 截断 → 512 → Stop
    // 下一 turn 让它立即 Stop:重置 calls 让 fail_times 已过。
    agent.run_one_turn("t2".into(), &tx); // 首个请求应回到 256(重置),非 512
    let seen = p.seen_max_tokens.lock().unwrap().clone();
    assert_eq!(seen[0], 256);           // turn1 起点
    assert_eq!(seen[1], 512);           // turn1 bump
    assert_eq!(seen[2], 256, "turn2 应从 self.max_tokens 重置, seen={seen:?}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib agent::tests::length_stop_bumps_effective_max_tokens_on_retry`
Expected: FAIL — 当前用固定 `self.max_tokens`（256）且 Length 处理在收尾 break 之后 → `seen` 全 256 或 turn 静默结束，断言不符。

- [ ] **Step 3: 引入 effective_max_tokens**（`src/agent.rs`，`for _ in 0..self.tool_cap {`（798）之前插入）

```rust
        // 自适应截断根治(迭代 2):有效 max_tokens 每 turn 从配置值起,命中 Length 翻倍上调。
        let mut effective_max_tokens = self.max_tokens;
```

- [ ] **Step 4: 请求改用 effective_max_tokens**（`src/agent.rs:836`）

把 `max_tokens: self.max_tokens,` 改为 `max_tokens: effective_max_tokens,`。

- [ ] **Step 5: 把 Length 处理提到收尾 break 之前并加 bump**（`src/agent.rs`）

在 `self.append(Role::Assistant, reply.items);`（875）之后、`if tool_calls.is_empty() {`（877）**之前**，插入新的 Length 处理块；并**删除**原 887-913 的旧 Length guard 块（避免重复）。新块：

```rust
            // 截断根治(迭代 2 / ADR 0038):响应在 max_tokens 处被截断时,先 neutralize 任何
            // 半序列化的 tool call(绝不执行),再自适应上调本 turn 的有效预算重试;达封顶
            // 仍截断则收尾。此判定必须在 `tool_calls.is_empty()` 收尾之前——否则截断的纯
            // 文本响应会被当成 turn 正常结束而静默收尾。
            if stop_reason == StopReason::Length {
                if !tool_calls.is_empty() {
                    let results = tool_calls
                        .iter()
                        .map(|(call_id, name, _)| {
                            let output = "tool call truncated: the response hit max_tokens before the \
                                 arguments finished; not executed. Retry with a shorter response or \
                                 split the work."
                                .to_string();
                            let _ = event_tx.send(AgentEvent::ToolFinished {
                                name: name.clone(),
                                is_error: true,
                                output: output.clone(),
                            });
                            MessageItem::ToolResult {
                                call_id: call_id.clone(),
                                output,
                                is_error: true,
                            }
                        })
                        .collect();
                    self.append(Role::Tool, results);
                }
                if effective_max_tokens < self.max_tokens_ceiling {
                    effective_max_tokens = effective_max_tokens.saturating_mul(2).min(self.max_tokens_ceiling);
                    let _ = event_tx.send(AgentEvent::Notice(format!(
                        "response truncated at max_tokens; raising to {effective_max_tokens} and retrying"
                    )));
                    continue; // 带更大预算重试
                }
                // 已达封顶:tool_calls 情形已追加 is_error(交模型重试);空 tool_calls 情形收尾。
                if tool_calls.is_empty() {
                    hit_tool_cap = false;
                    break;
                }
                continue;
            }

```

（注意:删除旧的 887-913 块后,`if tool_calls.is_empty() { … }` 收尾分支保持原样紧随其后。）

- [ ] **Step 6: 跑新测试 + 既有截断测试确认通过**

Run: `cargo test --lib agent::tests::length_stop_bumps_effective_max_tokens_on_retry agent::tests::effective_max_tokens_caps_at_ceiling agent::tests::bump_resets_per_turn agent::tests::truncated_tool_call_is_not_executed_and_loop_recovers`
（如过滤器只接受单名，逐个跑。）
Expected: PASS — 三个新测试绿；`truncated_tool_call_is_not_executed_and_loop_recovers` 仍绿（guard 不回退:半成品 tool call 不执行、is_error 追加）。

- [ ] **Step 7: 全 agent 模块 + 全仓测试**

Run: `cargo test --lib agent::tests && cargo test`
Expected: PASS（无回归）。

- [ ] **Step 8: 提交**

```bash
git add src/agent.rs
git commit -m "feat(agent): adaptive max_tokens bump on StopReason::Length (truncation cure)"
```

---

## Task 4: 小步写 system-prompt 引导

**Files:**
- Modify: `src/agent.rs`（`build_system_prompt_with_catalog` 加常量 push + 测试）

**Interfaces:**
- Consumes: `build_system_prompt`（1422）/`build_system_prompt_with_catalog`（1400）。

- [ ] **Step 1: 写失败测试**（加到 `src/agent.rs` `mod tests`）

```rust
#[test]
fn system_prompt_includes_small_step_write_guidance() {
    let dir = tempdir().unwrap();
    let p = build_system_prompt(dir.path());
    assert!(p.contains("append"), "应含小步写引导, prompt={p}");
    assert!(p.to_lowercase().contains("max_tokens"), "应解释原因, prompt={p}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib agent::tests::system_prompt_includes_small_step_write_guidance`
Expected: FAIL — 引导缺失。

- [ ] **Step 3: 加常量 + push**（`src/agent.rs`，`build_system_prompt_with_catalog`（1400）内）

在文件适当位置（`build_system_prompt_with_catalog` 之上）加常量：

```rust
/// 小步写纪律(迭代 2):始终注入,减少单次巨量 write_file 被 max_tokens 截断。
const SMALL_STEP_WRITE_GUIDANCE: &str =
    "When writing a large file, prefer building it up in smaller chunks \
     (multiple append-style edit_file / write_file calls) rather than one \
     giant write_file — a single oversized tool call can be cut off at \
     max_tokens and fail.";
```

在 `build_system_prompt_with_catalog` 的 `let mut parts = Vec::new();` 之后、AGENTS.md 处理之前（或紧随其后），加：

```rust
    parts.push(SMALL_STEP_WRITE_GUIDANCE.to_string());
```

- [ ] **Step 4: 跑测试确认通过 + 无回归**

Run: `cargo test --lib agent::tests::system_prompt_includes_small_step_write_guidance && cargo test --lib agent::tests`
Expected: PASS。注意 `build_system_prompt_uses_provided_registry`（2275）若断言了完整 prompt 结构，确认其仍绿；如它断言 parts 顺序/数量，按新增一段调整该断言。

- [ ] **Step 5: 提交**

```bash
git add src/agent.rs
git commit -m "feat(agent): inject small-step file-write guidance into system prompt"
```

---

## Task 5: 文档 + ADR 0038

**Files:**
- Modify: `README.md`（env 表：`CODECODER_MAX_TOKENS` 默认 8192；新增 `CODECODER_MAX_TOKENS_CEILING`）
- Modify: `ARCHITECTURE.md`（agent 循环段补自适应预算）
- Modify: `CLAUDE.md`（若提及 max_tokens 默认 4096，更新为 8192 + 自适应；若提及「max_tokens 截断」footgun，标注已治）
- Create: `docs/adr/0038-adaptive-max-tokens-budget.md`
- Modify: `docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`（§6.5 max_tokens 截断 footgun 标 `（已修：迭代 2 自适应预算）`）

- [ ] **Step 1: 核对代码事实**

Run: `grep -n "max_tokens" src/config.rs && grep -n "max_tokens_ceiling\|effective_max_tokens" src/agent.rs | head`
Expected: 确认默认 8192、ceiling 32768、bump 逻辑就位（供文档准确引用）。

- [ ] **Step 2: README env 表**——更新/新增两行：

```markdown
| `CODECODER_MAX_TOKENS` | `8192` | 单次生成的 max_tokens。命中截断时按 CODECODER_MAX_TOKENS_CEILING 自适应上调。 |
| `CODECODER_MAX_TOKENS_CEILING` | `32768` | 截断自适应上调的封顶值（迭代 2）。命中 StopReason::Length 时该 turn 有效 max_tokens 翻倍直至此上限。 |
```

- [ ] **Step 3: ARCHITECTURE.md**——在 agent 循环/turn 描述处补：

```markdown
turn 内命中 max_tokens 截断（`StopReason::Length`）时，先 neutralize 任何半序列化的 tool call（绝不执行），再把该 turn 的有效 max_tokens 翻倍上调（封顶 `CODECODER_MAX_TOKENS_CEILING`，默认 32768）重试；达封顶仍截断则收尾并交由里程碑客观门 → 迭代 1 自恢复循环接手。
```

- [ ] **Step 4: CLAUDE.md**——若有 max_tokens 默认 4096 的描述改为 8192 + 自适应；截断相关 footgun 措辞标注已治（读现有措辞后按实调整）。

- [ ] **Step 5: 新建 ADR 0038**

```markdown
# ADR 0038 — 自适应 max_tokens 预算（截断根治）

- **状态**: Accepted
- **日期**: 2026-07-24
- **关联**: ADR 0027（截断 guard / StopReason::Length）、迭代 2 spec

## 背景
max_tokens 默认 4096 偏低,大文件写常触发 StopReason::Length。既有 guard 只 neutralize 半成品 tool call 并提示模型「拆分」,不提高预算 → 弱模型反复截断至撞 tool cap;且 Length + 空 tool_calls 会先命中收尾 break 被静默当完成。

## 决策
1. max_tokens 默认 4096→8192。
2. turn 内局部 `effective_max_tokens`(每 turn 从 self.max_tokens 重置);命中 Length 且未达封顶 → `saturating_mul(2).min(ceiling)` 后 continue 重试;发 Notice 可观测。
3. 封顶 `CODECODER_MAX_TOKENS_CEILING`(默认 32768),在 AgentLoop::build 内由 Config::from_env() 注入 → 所有构造点统一遵守。
4. Length 判定提到 `tool_calls.is_empty()` 收尾之前,修静默收尾;保留既有 guard(半成品 tool call 绝不执行)。
5. 不拼接半写文件(脆弱);达封顶仍失败交里程碑门 → 迭代 1 自恢复。

## 后果
- 正面:大文件一次写成概率大增;弱模型无需自觉拆分;截断纯文本不再静默。
- 代价:单 turn 最坏多几次翻倍重试;部分 provider 对 max_tokens 有硬上限,超限请求走 complete_retrying 错误路径(不崩)。
- 补充(非本迭代):按 token 预估文件大小预设 max_tokens;per-tool 预算。
```

- [ ] **Step 6: 评估报告 §6.5**——`max_tokens` 截断 footgun 标 `（已修：迭代 2 自适应预算）`。

- [ ] **Step 7: 全仓测试 + 数字核对**

Run: `cargo test 2>&1 | tail -3`
Expected: PASS。README/ARCHITECTURE/CLAUDE 中若引用具体计数，按实更新。

- [ ] **Step 8: 提交**

```bash
git add README.md ARCHITECTURE.md CLAUDE.md docs/adr/0038-adaptive-max-tokens-budget.md docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md
git commit -m "docs: adaptive max_tokens budget (ADR 0038, README, ARCHITECTURE, CLAUDE)"
```

---

## Self-Review

- **Spec coverage**：改动点 1（config 默认+ceiling）=Task 1；ceiling 传导=Task 2；自适应 bump 核心 + Length 提前（缺口 3）=Task 3；小步写引导=Task 4；文档/ADR=Task 5。测试覆盖 spec §4 全部四项：bump-on-length、cap-at-ceiling、empty-tool-calls-not-silent（由 `effective_max_tokens_caps_at_ceiling` 的空 tool_calls 路径 + Length 提前覆盖）、reset-per-turn；并保留 guard 回归测试。
- **Placeholder scan**：无 TBD；每个改代码步骤给出完整代码。Task 5 CLAUDE.md 步骤依现有措辞「读后按实调整」——属既有文档措辞不确定，非需求占位。
- **Type consistency**：`max_tokens_ceiling: u32`（Config 与 AgentLoop 同名）、`set_max_tokens_ceiling(&mut self, u32)`、`effective_max_tokens`（局部）、`RecordingLengthProvider` 字段签名在 Task 3 内自洽；`saturating_mul(2).min(self.max_tokens_ceiling)` 与 spec §2/§3 数值一致。
- **已知取舍**：ceiling 在 build 内读 `Config::from_env()`——与「max_tokens 由调用方传入」略不对称，但换取所有构造点零改动地统一遵守 env，且测试可用 setter 确定性覆盖。
