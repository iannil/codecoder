# Kernel concurrency: OS threads, bidirectional channels, oneshot replies

The kernel runs on **OS threads + channels** (not tokio, not lunatic): a TUI thread renders while an agent thread runs the query loop, and sub-agents are spawned threads. Blocking LLM/tool I/O is fine because each lives on its own thread, and this keeps `async` from coloring all 21 tools. lunatic is reserved strictly for the `Wasm` capability environment (see [[0021-capability-environments-and-lifecycle]]), never as the host kernel.

## Channel topology

- **TUI → agent**: `cmd_tx` carrying `AgentCommand` — only **user-initiated** intents (`ProcessMessage`, `Shutdown`, `Cancel`).
- **agent → TUI**: `event_rx` carrying `AgentEvent` — one-way traffic at two rhythms (high-frequency LLM stream deltas; low-frequency structured state: tool start/end, reasoning, sub-agent milestones).
- **Blocking round-trips** (permission, `ask_user`): the `AgentEvent` embeds a `reply_tx` oneshot; the agent thread blocks on it until the TUI answers. Replies **never** travel over `cmd_tx`.

## Considered options

- **tokio** — better for many parallel outbound HTTP calls, but viral `async` across every tool, sandbox, and the TUI bridge. Not on the hot path today.
- **lunatic processes** — philosophically elegant for sandboxing, but would make CodeCoder itself a WASM guest, crippling the native TUI, FS, and HTTP client.

## Consequences

- `PermissionResponse` is **not** an `AgentCommand` — routing a pending request's answer over `cmd_tx` would let it be reordered behind a new `ProcessMessage`. The oneshot makes reordering structurally impossible.
- Within a turn, tool calls execute **serially** in the order the model returned them (one Dialog is active at a time; side effects stay ordered). Parallel execution is a future optimization limited to `Permission::None` tools.
- **Cancellation is cooperative**, not a thread kill (Rust cannot safely kill a thread): `Cancel` sets a shared cancellation token checked between stream deltas and before each tool; long-running `run_command` children are killed via a stored process handle; a thread blocked on `reply_tx` is released by the TUI answering with a `Cancelled` variant.
