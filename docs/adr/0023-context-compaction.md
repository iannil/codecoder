# Context compaction

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
