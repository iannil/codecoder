# Context compaction

**Status**: Accepted; **tier 1 and tier 2 implemented**. `compaction.rs::working_set` applies tier 1 (drop `Reasoning`, elide old `ToolResult` bodies); when the tier-1 result is still over threshold, `AgentLoop::context_working_set` applies tier 2 — one cached LLM call summarizes the oldest span (`[anchor+1 .. last_user]`) into a synthetic `System` message. The persisted Session stays full-fidelity.

A long-running self-evolving agent will overflow any model window, so the context sent to the provider is compacted — but **compaction only shapes a derived working set; it never destroys the persisted Session**. The on-disk `messages` stay full-fidelity; the `Context Working Set` is recomputed each run from those messages against the current model's window.

## Trigger and strategy

- **Trigger**: `token_count` reaching ~75% of the *model's* context window (a ratio, not a fixed token count — windows vary widely across models).
- **Strategy — tiered hybrid**:
  1. Drop the cheapest, bulkiest items first: old `ToolResult` bodies and `Reasoning` (which are not replayed to the provider anyway — see [[0004-session-persistence-and-migration]]). This tier alone is often enough.
  2. If still over, summarize the oldest remaining dialogue span into a synthetic `System` summary message (costs one LLM call).
  - The **first user goal** is an anchor that is never evicted.

## Why derive, not mutate

If compaction rewrote history in place, the autosaved Session (see [[0004-session-persistence-and-migration]]) would permanently lose original messages — contradicting "the Session is the durable record." Instead the full history is persisted and the working set is derived, so:

- `/resume` restores full fidelity;
- a summary is stored as a `compaction` side-field/overlay, not a replacement for `messages`;
- switching to a larger-window model automatically "decompresses" an old conversation.

This mirrors the derived-`Mode` principle ([[0001-tui-keybinding-and-mode-semantics]]): anything derivable is derived, never stored as state that can drift.

## Implementation notes (tier 1)

- **The threshold is evaluated against the full history size, never the compacted size.** `working_set` recomputes `count_tokens` on the untouched `messages` each turn. Feeding the post-compaction count back into `should_compact` would lower it, un-trip the threshold, and oscillate (compact → shrink → grow → overflow).
- **`ToolResult` bodies are elided, not removed.** The item is kept with a `[elided …]` placeholder so the `tool_call` ↔ `tool_call_id` pairing survives — OpenAI rejects a `tool_call` with no matching tool response.
- **Evicting `Reasoning` does not shrink the provider request** — the wire layer already skips it ([[0004-session-persistence-and-migration]]). Its removal instead realigns `count_tokens` (which *does* count it) with what is actually sent, so `ctx%` stops overstating the payload. Only `ToolResult` elision reduces the real request.
- `should_compact` is now live (called inside `working_set`); it is no longer a wired-to-nothing detector.
