# Unified message model

Every entry in a conversation is one `Message { id: MessageId, role, items: Vec<MessageItem> }` — a single model covering user prose, assistant answers, chain-of-thought, and tool traffic, rather than separate types per kind. `MessageItem` variants are `Text`, `Reasoning`, `ToolCall { id, name, args }`, and `ToolResult { call_id, .. }` (see [[0017-provider-neutral-message-model]]).

## Two distinct identities

- **`MessageId`** — a per-session monotonic `u64` assigned when a `Message` is appended and persisted with it. It anchors UI display state in `TuiApp`'s `HashMap<MessageId, DisplayState>`, so eviction / `/clear` / compaction don't mis-anchor collapse/expand state. It is not a UUID.
- **`ToolCall.id`** — a provider-neutral correlation id linking a `ToolCall` item to its `ToolResult` item, mapped at the API boundary to the wire format (OpenAI: `tool_calls[].id` / `tool_call_id`).

## Why unify

A single message type lets persistence, streaming render, and compaction operate over one uniform sequence. Keeping `MessageId` (whole-message UI/persistence identity) strictly separate from `ToolCall.id` (provider correlation) prevents the two from being conflated — they have different scopes, lifetimes, and consumers.
