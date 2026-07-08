# Provider-neutral message model, OpenAI as the canonical protocol

The internal conversation is a provider-neutral `Message { id: MessageId, role, items: Vec<MessageItem> }`, where `MessageItem` is `Text | Reasoning | ToolCall { id, name, args } | ToolResult { call_id, .. }`. A `Provider` trait translates this model to/from a concrete wire protocol at the API boundary; **OpenAI chat-completions is the canonical protocol** (matching `CODECODER_API_BASE`, `gpt-4o`, and the largest OpenAI-compatible ecosystem), with `StubClient` as the keyless deterministic fake.

## Why neutral

The reference implementation (claude-code) speaks Anthropic Messages, whose `tool_use`/`tool_result` content-block shape differs structurally from OpenAI's `tool_calls[]` + `role:"tool"` turns. Session persistence, streaming render, and compaction all depend on one stable message shape, so the wire format must not leak into it.

## Consequences

- `ToolCall.id` is a **provider-neutral** correlation id; at the OpenAI boundary it maps to `tool_calls[].id` (assistant turn) and `tool_call_id` (tool turn).
- Adding Anthropic (or any provider) later is a new `Provider` impl, not a change to the message model or to sessions on disk.
- Streaming deltas are normalized by the `Provider` into `AgentEvent`s before reaching the TUI, so the render path is provider-agnostic.
