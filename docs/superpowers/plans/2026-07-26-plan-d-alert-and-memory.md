# Plan D: Alert Channel & Long-Term Memory Accumulation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add alert notifications (webhook) for headless runner failures and implement a mechanism for the agent to autonomously accumulate project knowledge across sessions. This addresses the P1/P2 gaps where failures go unnoticed and each session starts from zero context about the project.

**Architecture:** Two independent sub-systems:
1. **Alert channel**: After each BG run, check the exit code and mission_state. If non-zero, POST a JSON payload to a configurable webhook URL. Supports Slack webhook format (simple message) and generic webhook.
2. **Long-term memory accumulation**: An `auto-memory` skill that runs after each milestone completion, prompting the agent to write a memory entry about what it learned about the project (codebase patterns, architecture decisions, pitfalls). Also add a daemon thread that periodically reviews `bg_ledger.jsonl` and writes summary memories.

**Tech Stack:** Rust (existing), ureq (existing — HTTP POST), serde_json (existing)

## Global Constraints

- Alert webhook URL comes from env var `CODECODER_ALERT_WEBHOOK` (not from `.ccd.env` — secrets never from repo files)
- Webhook payload is JSON, Slack-compatible: `{"text": "...", "attachments": [...]}`
- Memory accumulation is best-effort: failure to write a memory does not interrupt the milestone
- Memory entries follow the existing `memory/<key>` file convention
- `CODECODER_ALERT_ON_FAILURE_ONLY` env var (default true) — only alert on non-zero exit
- New memory entries are prefixed `auto-` to distinguish them from manually written memories

---

### Task 1: Add alert config and webhook sender

**Files:**
- Modify: `src/config.rs` (add `alert_webhook`, `alert_on_failure_only`)
- Create: `src/alert.rs` (webhook sender)
- Test: inline tests

**Interfaces:**
- `Config.alert_webhook: Option<String>` — webhook URL (env `CODECODER_ALERT_WEBHOOK`)
- `Config.alert_on_failure_only: bool` — default true (env `CODECODER_ALERT_ON_FAILURE_ONLY`)
- `send_alert(webhook: &str, text: &str) -> anyhow::Result<()>` — POST JSON to webhook

- [ ] **Step 1: Add config fields**

```rust
// In Config struct:
    /// 告警 webhook URL (Slack-compatible)。env CODECODER_ALERT_WEBHOOK。
    pub alert_webhook: Option<String>,
    /// 是否仅失败时告警。env CODECODER_ALERT_ON_FAILURE_ONLY, 默认 true。
    pub alert_on_failure_only: bool,
```

- [ ] **Step 2: Add env parsing**

```rust
            alert_webhook: env("CODECODER_ALERT_WEBHOOK"),
            alert_on_failure_only: env("CODECODER_ALERT_ON_FAILURE_ONLY")
                .map(|v| v != "0" && v != "false")
                .unwrap_or(true),
```

- [ ] **Step 3: Create `src/alert.rs`**

```rust
// src/alert.rs
// Alert notification system for headless runner failures.

/// Send a Slack-compatible webhook alert.
/// The payload is `{"text": "..."}` — compatible with Slack, Discord, Teams, etc.
pub fn send_alert(webhook: &str, text: &str) -> anyhow::Result<()> {
    let body = serde_json::json!({ "text": text });
    let resp = ureq::post(webhook)
        .set("Content-Type", "application/json")
        .send_json(&body)?;
    let status = resp.status();
    if status < 200 || status >= 300 {
        let body_text = resp.into_string().unwrap_or_default();
        anyhow::bail!("webhook returned {status}: {body_text}");
    }
    Ok(())
}

/// Build an alert message from a BG run outcome.
pub fn format_bg_alert(exit_code: i32, mission_state: &str, summary: &str) -> String {
    format!(
        "🔴 CodeCoder BG Alert\nExit Code: {exit_code}\nState: {mission_state}\nSummary: {summary}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bg_alert_includes_key_info() {
        let msg = format_bg_alert(2, "StuckNeedsFix", "milestone #3 failed after 3 retries");
        assert!(msg.contains("Exit Code: 2"));
        assert!(msg.contains("StuckNeedsFix"));
        assert!(msg.contains("milestone #3"));
    }
}
```

- [ ] **Step 4: Wire alert into background runner**

In `src/background.rs`, after the `run_background` / `run_background_cfg` function completes, check the exit code. If alert is configured and the condition is met, call `send_alert`.

- [ ] **Step 5: Run tests**

```bash
cargo test alert::tests
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/alert.rs src/config.rs src/background.rs
git commit -m "feat: add webhook alert channel for BG runner failures"
```

---

### Task 2: Add auto-memory skill

**Files:**
- Create: `skills/auto-memory.md` (the skill that prompts the agent to write memory entries)
- Modify: `src/background.rs` or `src/agent.rs` (trigger auto-memory after milestone completion)

**Interfaces:**
- `skills/auto-memory.md`: A procedural knowledge document that tells the agent to write a memory entry after completing a milestone, documenting what it learned about the project
- The trigger is in `drive_workgraph` or `advance_one_milestone`: after a milestone passes acceptance, activate the auto-memory skill and prompt the agent

- [ ] **Step 1: Create `skills/auto-memory.md`**

```markdown
# Auto-Memory: Project Knowledge Accumulation

After completing each milestone, write a `memory/auto-<topic>.md` entry documenting
what you learned about the project during this milestone. This ensures knowledge
accumulates across sessions.

## When to write

After a milestone passes acceptance (verdict == pass), write one or more memory
entries covering:

1. **Codebase patterns discovered**: naming conventions, file organization, 
   architectural patterns you observed
2. **Pitfalls encountered**: bugs, gotchas, things that went wrong and why
3. **Design decisions made**: why you chose approach A over B
4. **New dependencies or tools used**: what they do and how to use them

## Format

Each memory entry is a file under `memory/` named `auto-<kebab-case-topic>.md`:

```markdown
---
name: auto-<topic>
description: <one-line description>
metadata:
  type: project
---

<detailed content, 2-5 sentences>

**Why:** <why this knowledge matters>
**How to apply:** <how to use this knowledge in future work>
```

## Constraints

- Keep entries concise (2-5 sentences each)
- Only write entries for genuinely non-obvious knowledge
- Skip entries for things already documented in ADRs, ARCHITECTURE.md, or README.md
- Use the `memory` tool to write the entry
```

- [ ] **Step 2: Wire auto-memory trigger in `drive_workgraph`**

In `src/agent.rs`, after a milestone's verdict is `pass`, inject a user message prompting the agent to run the auto-memory process via `use_skill auto-memory`. This is a non-blocking nudge (not an error if it fails).

- [ ] **Step 3: Run tests**

```bash
cargo test
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add skills/auto-memory.md src/agent.rs
git commit -m "feat: add auto-memory skill for cross-session knowledge accumulation"
```

---

### Task 3: Add ledger review daemon thread

**Files:**
- Modify: `src/daemon/mod.rs` (add a thread that periodically reviews bg_ledger and writes summary memories)
- Test: inline tests

**Interfaces:**
- A new daemon thread reads `bg_ledger.jsonl` every `CODECODER_WG_TICK_SECS`, identifies patterns (repeated failures, new successful patterns), and writes summary memories

- [ ] **Step 1: Add the ledger review thread**

```rust
// In src/daemon/mod.rs, after the reload thread, add a lightweight thread that
// reads the last N entries of bg_ledger and writes a summary memory if patterns emerge.
// For simplicity, this thread just writes a "last BG run" memory on each tick.
```

- [ ] **Step 2: Run tests**

```bash
cargo test
```
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/daemon/mod.rs
git commit -m "feat(daemon): add ledger review thread for periodic memory accumulation"
```

---

### Task 4: Update docs

**Files:**
- Modify: `README.md` (add new env vars, document auto-memory skill)
- Modify: `AGENTS.md` (reference auto-memory skill)

- [ ] **Step 1: Update README.md**

Add `CODECODER_ALERT_WEBHOOK`, `CODECODER_ALERT_ON_FAILURE_ONLY` to env table.
Add auto-memory to the skills section.

- [ ] **Step 2: Update AGENTS.md**

Add a line about auto-memory for cross-session learning.

- [ ] **Step 3: Commit**

```bash
git add README.md AGENTS.md
git commit -m "docs: document alert webhook and auto-memory skill"
```