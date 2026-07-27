# Plan C: Tier-2 Compaction (LLM Summarization)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement tier-2 compaction — when tier-1 (dropping Reasoning + ToolResult bodies) is insufficient to keep the Context Working Set within the model's context window, use a one-shot LLM call to summarize the oldest conversation span into a synthetic System message. This addresses the P1 gap where long-running sessions could overflow the context window.

**Architecture:** The existing `compaction.rs` module already has the tier-1 infrastructure. Add `tier2_summarize` and `tier2_should_run` functions. When tier-1 is done and the working set still exceeds the threshold, the agent's `process_turn` method calls tier-2: it picks the oldest span of messages (before the anchor goal, before the recent tail), calls the provider with a summarization prompt, and replaces that span with a single synthetic System message. The tier-2 result is non-persisted (derived from the full messages, like tier-1). Follow the existing pattern in `compaction.rs` — the `ContextWorkingSet` builder.

**Tech Stack:** Rust (existing), same provider used for chat (reuse `Provider::complete`), existing tokenizer for length estimation.

## Global Constraints

- Tier-2 must NEVER destroy the persisted session record (same invariant as tier-1)
- The first user goal message must remain as an anchor (never summarized away)
- The recent tail (last N messages) must be preserved in full for ongoing conversation
- Tier-2 summary is a synthetic System message, not a replacement for the original messages
- Only invoke tier-2 when tier-1 is done and the working set is still > 75% of the window
- On tier-2 failure (LLM error, empty response), degrade gracefully back to tier-1 only
- Add a `CODECODER_COMPACTION_TIER2` env var (default `true`) to allow disabling

---

### Task 1: Add tier-2 compaction config

**Files:**
- Modify: `src/config.rs` (add `compaction_tier2: bool` field)
- Test: inline tests

**Interfaces:**
- `Config.compaction_tier2: bool` (default true, env `CODECODER_COMPACTION_TIER2`)

- [ ] **Step 1: Add field to Config**

```rust
// After pub command_timeout_secs:
    /// 是否启用 tier-2 compaction (LLM 摘要)。env CODECODER_COMPACTION_TIER2, 默认 true。
    pub compaction_tier2: bool,
```

- [ ] **Step 2: Add env parsing**

```rust
            compaction_tier2: env("CODECODER_COMPACTION_TIER2")
                .map(|v| v != "0" && v != "false" && v != "no")
                .unwrap_or(true),
```

- [ ] **Step 3: Add to DOTENV_ALLOWED_KEYS**

```rust
    "CODECODER_COMPACTION_TIER2",
```

- [ ] **Step 4: Write test**

```rust
#[test]
fn compaction_tier2_default_true() {
    unsafe { std::env::remove_var("CODECODER_COMPACTION_TIER2"); }
    assert!(Config::from_env().compaction_tier2);
}

#[test]
fn compaction_tier2_can_be_disabled() {
    unsafe { std::env::set_var("CODECODER_COMPACTION_TIER2", "false"); }
    assert!(!Config::from_env().compaction_tier2);
    unsafe { std::env::remove_var("CODECODER_COMPACTION_TIER2"); }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add compaction_tier2 toggle"
```

---

### Task 2: Implement tier-2 summarization logic

**Files:**
- Modify: `src/compaction.rs` (add `tier2_should_run`, `tier2_summarize` functions)
- Test: inline tests in `src/compaction.rs`

**Interfaces:**
- `tier2_should_run(working_set: &[&Message], threshold_pct: f64, window_size: usize) -> Option<usize>` — returns the index of the first message to summarize (the oldest span that isn't the anchor or the tail), or None if tier-2 is not needed
- `tier2_summarize(provider: &dyn Provider, span: &[&Message], model: &str) -> Result<String, String>` — calls the LLM to summarize a message span, returns the summary text
- `build_summary_prompt(span: &[&Message]) -> String` — builds the summarization prompt
- `Tier2Summary { text: String, token_count: usize }`

- [ ] **Step 1: Read the existing compaction.rs**

```bash
cat src/compaction.rs | head -80
```

- [ ] **Step 2: Add `tier2_should_run` function**

```rust
/// Determine if tier-2 compaction should run after tier-1.
/// Returns the index of the first message that can be summarized (the oldest span
/// that is not the anchor goal and not the recent tail).
/// 
/// The anchor is the first user message (the original goal, never evicted).
/// The tail is the last ~20 messages (preserved for ongoing conversation context).
/// Everything between them is eligible for summarization.
pub fn tier2_should_run(working_set: &[&Message], threshold_pct: f64, window_size: usize) -> Option<usize> {
    // ... implementation
}
```

- [ ] **Step 3: Add `build_summary_prompt`**

```rust
/// Build a summarization prompt for an LLM.
/// Instructs the LLM to extract: what was accomplished, what decisions were made,
/// what remaining work exists, and any important context.
pub fn build_summary_prompt(span: &[&Message]) -> String {
    let mut text = String::new();
    for m in span {
        for item in &m.items {
            match item {
                MessageItem::Text(t) => text.push_str(t),
                MessageItem::ToolCall { name, args, .. } => {
                    text.push_str(&format!("[tool: {name}({args})]\n"));
                }
                MessageItem::ToolResult { content, .. } => {
                    text.push_str(&format!("[result: {}]\n", truncate(content, 200)));
                }
                _ => {}
            }
        }
    }
    format!(
        "You are a conversation summarizer. The following is a conversation span \
         between a user and an AI agent. Summarize the key information: what was \
         asked, what was done, what decisions were made, what files were changed, \
         and what remaining work exists. Be concise but comprehensive.\n\n\
         CONVERSATION:\n{text}\n\n\
         SUMMARY:"
    )
}
```

- [ ] **Step 4: Add `tier2_summarize` function**

```rust
/// Perform tier-2 summarization: call the LLM to summarize the oldest conversation span.
/// Returns the summary text on success, or an error string.
pub fn tier2_summarize(
    provider: &dyn Provider,
    span: &[&Message],
    model: &str,
) -> Result<String, String> {
    let prompt = build_summary_prompt(span);
    let req = crate::provider::CompletionRequest {
        model: model.to_string(),
        messages: vec![crate::message::Message {
            id: 0,
            role: crate::message::Role::User,
            items: vec![crate::message::MessageItem::Text(prompt)],
            created_at: 0,
        }],
        max_tokens: 1024,
        temperature: 0.3,
        tool_schemas: None,
    };
    match provider.complete(&req) {
        Ok(completion) => {
            let text = completion.content.trim().to_string();
            if text.is_empty() {
                return Err("tier-2 summary returned empty text".into());
            }
            Ok(text)
        }
        Err(e) => Err(format!("tier-2 LLM call failed: {e}")),
    }
}
```

- [ ] **Step 5: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, MessageItem, Role};

    fn make_message(id: u64, role: Role, text: &str) -> Message {
        Message {
            id,
            role,
            items: vec![MessageItem::Text(text.to_string())],
            created_at: id as u64,
        }
    }

    #[test]
    fn tier2_should_run_returns_none_when_below_threshold() {
        // ... implementation
    }

    #[test]
    fn tier2_should_run_returns_index_when_above_threshold() {
        // ... implementation
    }

    #[test]
    fn build_summary_prompt_contains_messages() {
        let msgs = vec![make_message(1, Role::User, "hello")];
        let prompt = build_summary_prompt(&msgs.iter().collect::<Vec<_>>());
        assert!(prompt.contains("hello"));
    }
}
```

- [ ] **Step 6: Run tests**

```bash
cargo test compaction::tests
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/compaction.rs
git commit -m "feat(compaction): add tier-2 LLM summarization logic"
```

---

### Task 3: Wire tier-2 into ContextWorkingSet builder

**Files:**
- Modify: `src/compaction.rs` (integrate tier-2 into the `ContextWorkingSet` builder)
- Modify: `src/agent.rs` (pass `compaction_tier2` config through to the compaction call)
- Test: existing compaction tests + new integration test

**Interfaces:**
- `ContextWorkingSet::from_session` now accepts `compaction_tier2: bool` parameter
- After tier-1 drops Reasoning + ToolResult bodies, if still over threshold and tier-2 enabled, call `tier2_summarize` and replace the summarizable span with a synthetic System message

- [ ] **Step 1: Modify `ContextWorkingSet` builder**

Follow the existing pattern: after tier-1 processing, check if `tier2_should_run` returns Some. If so, call `tier2_summarize`, build a synthetic System message with the summary, and replace the span.

- [ ] **Step 2: Add integration test that exercises tier-2 path**

Use `ScriptedProvider` to inject a controlled summary response.

- [ ] **Step 3: Run tests**

```bash
cargo test
```
Expected: All existing tests still pass.

- [ ] **Step 4: Commit**

```bash
git add src/compaction.rs src/agent.rs
git commit -m "feat(compaction): wire tier-2 summarization into ContextWorkingSet"
```

---

### Task 4: Update docs

**Files:**
- Modify: `README.md` (add `CODECODER_COMPACTION_TIER2` env var)
- Modify: `ARCHITECTURE.md` (update the compaction entry to note tier-2 is implemented)

**Interfaces:**
- Documentation only.

- [ ] **Step 1: Update README.md env table**

Add `CODECODER_COMPACTION_TIER2` entry.

- [ ] **Step 2: Update ARCHITECTURE.md**

Change the compaction entry in the module map from "tier-1 live + tier-2 deferred" to "tier-1 + tier-2 (LLM summarization)".

- [ ] **Step 3: Commit**

```bash
git add README.md ARCHITECTURE.md
git commit -m "docs: document tier-2 compaction feature"
```