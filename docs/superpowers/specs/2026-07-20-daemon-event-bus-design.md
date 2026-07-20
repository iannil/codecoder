# Daemon Event Bus — Design Spec

**Date:** 2026-07-20
**Branch:** `feat/daemon-event-bus`
**Status:** Approved (brainstormed 2026-07-20)
**Related:** ADR 0016 (channel topology), ADR 0032 (client-server architecture), client-server migration plan Task 8 (cross-session sharing — the event-bus stretch item)

## Goal

A daemon-level **event bus** that broadcasts lightweight `Notice` events (with a `source` tag) to all connected `cc` clients in **real time** — even clients that are idle in the REPL between turns. Primary use case: the daemon's background workgraph tick advances a milestone (or the supervisor restarts a crashed capability) and every connected client sees a notice immediately, without waiting for the next turn.

This also **fixes a latent bug**: `handle_connection` is currently one-request-per-connection, but `cc`'s REPL holds one persistent connection and sends multiple `SendMessage`s — so the second turn of any REPL session hangs forever. The persistent-connection refactor required for real-time push fixes this.

## Background / Current State

- `handle_connection` (`src/daemon/socket.rs:47`) reads ONE `ClientRequest`, handles it, returns. The daemon spawns a thread per accepted connection.
- `cc`'s `repl` (`src/bin/cc.rs:49`) calls `Connection::connect` ONCE, then loops `stdin → send(SendMessage) → drain events`. Mismatch → the 2nd turn hangs (no daemon thread reads the socket).
- The daemon already runs background threads that produce noteworthy events: the workgraph tick (`advance_one_milestone`) and the supervisor (`Supervisor::supervise`). Today these produce `BgOutcome`/internal state only — nothing reaches a connected client.
- `AgentEvent::Notice(text)` exists but flows only within a single session's agent→client stream; there is no cross-session/daemon-wide notification channel.

## Non-goals

- Typed/structured bus events (workgraph_milestone_done, capability_crashed, …). The bus carries `Notice` text + a `source` tag only. Typed events can be added later if rendering/dispatch needs it. (YAGNI.)
- Topic/filter subscriptions. Every connected client receives every broadcast. Filtering is a follow-up if a real need appears.
- Persistence of bus events. A client that connects after a broadcast does not receive it (no replay buffer).
- Bus events delivered to one-shot `cc "query"` invocations (those connect, do one turn, disconnect) — they receive broadcasts only during their brief connection.

## Design — Approach A: combined-channel + dedicated writer

### Persistent connection (fixes REPL + enables push)

`handle_connection` becomes a **loop** over `ClientRequest`s on a single connection. Per connection it sets up:

- `combined: (mpsc::Sender<ServerEvent>, mpsc::Receiver<ServerEvent>)` — the single stream the writer drains.
- A **writer thread** that owns the socket write-half: `for ev in combined_rx { write_event(socket, ev) }`. Single writer → writes are serialized. Exits when `combined_rx` closes.
- The connection's `combined_tx` is **registered with the `EventBus`** so broadcasts land directly in `combined_rx`.

The connection's main thread is the **request-reader**: loops `read_request`, and per request:
- `SendMessage`/`Resume` → dispatch via the session manager (returns a raw `Receiver<AgentEvent>`), spawn a **turn-feeder** thread (see below), continue reading (so `PromptReply` and the next request can arrive during the turn).
- `PromptReply { id, answer }` → send to the turn's `reply_tx` (per-connection; only one prompt is pending at a time).
- `NewSession`/`ListSessions`/`Status`/`Shutdown` → handle, push the result `ServerEvent` to `combined_tx`.

On EOF (client closed): drop `combined_tx` (writer exits), return. The bus reaps the dead `combined_tx` on its next `broadcast` (see Error handling).

### Turn-feeder (per turn)

A thread spawned per `SendMessage`/`Resume`:
- acquires the **turn_token** (the existing `Arc<Mutex<()>>` — keeps user turns and the workgraph tick mutually exclusive on the workgraph);
- drains the turn's `Receiver<AgentEvent>` via `recv_timeout(120s)`, translating each to `ServerEvent` and sending to `combined_tx`;
- on a prompt `AgentEvent` (`PermissionRequest`/`AskUser`/`Confirm`/`PlanApproval`/`TrustPrompt`): writes `ServerEvent::Prompt` to `combined_tx`, then blocks reading ONE `PromptReply` from the connection's `reply_rx`, fulfills the agent's oneshot, continues;
- on `TurnComplete` (or timeout/disconnect → `Error`): releases the turn_token, exits.

This preserves the synchronous-prompt model from Task 9a and the turn-token mutual exclusion, now across a persistent connection.

### EventBus

New module `src/daemon/bus.rs`:

```rust
pub struct EventBus {
    subscribers: std::sync::Mutex<Vec<std::sync::mpsc::Sender<crate::daemon::proto::ServerEvent>>>,
}

impl EventBus {
    pub fn new() -> Self { Self { subscribers: Default::default() } }
    /// Register a connection's combined_tx. Returns nothing; the bus owns the Sender.
    pub fn register(&self, tx: std::sync::mpsc::Sender<crate::daemon::proto::ServerEvent>) {
        self.subscribers.lock().unwrap().push(tx);
    }
    /// Broadcast a BusNotice to every registered connection. Dead/full subscribers
    /// are reaped (mpsc::Sender::send fails when the receiver is dropped or the
    /// channel is full — we treat both as "this subscriber is gone/stuck").
    pub fn broadcast(&self, source: &str, text: &str) {
        let ev = crate::daemon::proto::ServerEvent::BusNotice {
            source: source.into(), text: text.into(),
        };
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(ev.clone()).is_ok());
    }
}
```

Owned by `Daemon` as `Arc<EventBus>`, cloned into `handle_connection` (to register) and into the workgraph + supervisor threads (to broadcast). No async, no extra threads — broadcasts happen inline at the publisher.

### Protocol addition (`src/daemon/proto.rs`)

```rust
// in ServerEvent:
BusNotice { source: String, text: String },
```

Tagged `#[serde(tag = "type", rename_all = "snake_case")]` → `{"type":"bus_notice","source":"workgraph","text":"..."}`. The `cc` client renders it distinctly from per-turn `Notice` (e.g. prefix with the source).

### Publishers (two, both cheap)

1. **Workgraph tick** (`src/daemon/mod.rs` workgraph thread): after `advance_one_milestone(...)` returns `Some(outcome)`, broadcast a notice summarizing it, e.g. `bus.broadcast("workgraph", &format!("milestone advanced: {}", summary))`. (The `BgOutcome.events` already carries human-readable lines — use one.)
2. **Supervisor** (`src/daemon/mod.rs` supervisor thread, or inside `Supervisor::supervise`): on a restart or `gave_up`, broadcast `bus.broadcast("supervisor", &format!("capability '{name}' {restarted|gave up}"))`.

Both are single `broadcast` calls at points that already hold the information.

### `cc` client concurrent reader

`cc`'s REPL gains a **reader thread**:
- continuously `conn.next_event()` and prints each `ServerEvent` (bus notices, turn deltas, tool events, …);
- on `ServerEvent::Prompt { id, body }`: render it, read ONE line from stdin, send `ClientRequest::PromptReply { id, answer }` (safe — the main thread is blocked waiting for turn-complete during a turn, so stdin is free for the reader to consume);
- on `ServerEvent::TurnComplete` (or `Error`): signal the main thread (via a oneshot/channel) that the turn is done.

The main thread: read stdin → `conn.send(SendMessage)` → wait for the reader's turn-done signal → loop. Between turns the reader keeps printing bus notices as they arrive (real-time push while idle).

`send_one` (one-shot `cc "query"`) gets the same reader loop (without the stdin REPL); it also benefits from receiving bus notices during its brief connection, though that's incidental.

### Data flow

- **Bus event:** publisher → `bus.broadcast` → each registered `combined_tx` → `combined_rx` → writer → socket → `cc` reader thread → printed immediately (even while the client is idle in the REPL).
- **Turn event:** `SendMessage` → request-reader dispatches → turn-feeder drains → `combined_tx` → writer → socket → reader prints; `TurnComplete` → feeder exits + reader signals main.
- **Prompt:** turn-feeder writes `Prompt` → writer → socket → reader renders + reads stdin + sends `PromptReply` → request-reader routes to `reply_tx` → feeder unblocks.

## Error handling

- **Dead/stuck subscriber:** `mpsc::Sender::send` is non-blocking and fails if the receiver is dropped OR the channel is full (bounded? — `mpsc::channel()` is unbounded, so "full" doesn't happen; `send` fails only on receiver-dropped). A dropped receiver (connection closed) → `retain` reaps it. So a closed connection is cleaned up lazily on the next broadcast. (Unbounded channel means a slow client's notices queue in memory — acceptable for a dev tool; if it ever matters, switch to `sync_channel` with `try_send`.)
- **Writer exit:** when the request-reader drops `combined_tx` on connection end, `combined_rx` closes once all clones of `combined_tx` are dropped — including the one the bus holds. The bus reaps its clone on the next broadcast (send fails). Until then the writer may stay alive briefly holding a closed-drained rx; it exits as soon as `combined_rx` returns `None`. To avoid a stranded writer, `handle_connection` on EOF should also drop its `combined_tx` and join the writer.
- **Turn timeout:** turn-feeder keeps the existing 120s `recv_timeout` → on Timeout/Disconnected sends `ServerEvent::Error` to `combined_tx` and exits.
- **Lock poisoning:** `EventBus.subscribers` Mutex + `mpsc` — `.lock().unwrap()` matches codebase convention.
- **Thread lifetime:** writer + request-reader joined/cleaned on connection end; turn-feeder exits on TurnComplete; bus subscriptions reaped lazily.

## Testing

- `bus::tests::register_then_broadcast_delivers` — register a subscriber tx, broadcast, assert the rx receives `BusNotice{source,text}`.
- `bus::tests::broadcast_reaps_dead_subscribers` — register two, drop one rx, broadcast, assert the live one still receives and the dead one is reaped (`subscribers.len()` shrinks).
- `socket` integration test **`multi_turn_on_one_connection`** (REPL-bug regression) — one connection sends `SendMessage("a")` → drain to TurnComplete; sends `SendMessage("b")` → drain to TurnComplete. Both complete (pre-fix the second hangs).
- `socket` integration test **`bus_notice_reaches_idle_client`** — client connects (subscribes), sends NO turn; daemon `bus.broadcast("test", "hi")`; assert the client receives `BusNotice` (proves real-time push to an idle client).
- `socket` integration test **`prompt_round_trips_across_persistent_connection`** — a scripted provider that triggers a `Confirm` mid-turn; client (on the same persistent connection) answers; turn completes. (Guards the prompt-routing refactor.)
- Publisher wiring: a focused test that `advance_one_milestone` returning `Some` triggers a `bus.broadcast` (inject an `EventBus`, assert a subscriber received it) — or assert via the workgraph-thread path.
- Existing suite stays green; `handle_connection` signature change (`+ bus: &Arc<EventBus>`) ripples to callers/tests.

## Rationale — why Approach A

| Approach | Threads/conn | Write serialization | Std-select needed | Verdict |
|---|---|---|---|---|
| **A: combined-channel + writer (chosen)** | 2 persistent + 1/turn | single writer (clean) | no | chosen |
| B: try_recv poll loop | 1 | inline (racy without care) | yes (nonblocking socket) | busy-wait, messy prompt routing |
| C: separate bus-rx + feeder | 3 + 1/turn | single writer | no | more threads, no gain over A |

A gives clean single-writer serialization, no busy-wait, no nonblocking-socket gymnastics, and the bus adds zero extra threads (broadcasts land directly in each connection's combined channel). The per-connection thread count (2 persistent + 1/turn) is fine for a dev tool with 1–2 clients.

## Interfaces (contract for the implementation plan)

- `proto::ServerEvent::BusNotice { source: String, text: String }`.
- `daemon::bus::EventBus::new() -> Self`; `register(tx: mpsc::Sender<ServerEvent>)`; `broadcast(source: &str, text: &str)`.
- `socket::handle_connection(stream, mgr, shutdown, turn_token, bus: &Arc<EventBus>)` — now loops; signature gains `bus`.
- per-connection: `combined: (mpsc::Sender<ServerEvent>, mpsc::Receiver<ServerEvent>)`; `reply: (mpsc::Sender<PromptAnswer>, mpsc::Receiver<PromptAnswer>)` for prompt routing.
- `Daemon::run` owns `Arc<EventBus>`; passes clones to `handle_connection` and to the workgraph + supervisor threads.
- `cc` REPL: reader thread + turn-done signaling (oneshot or channel).

## Files touched

- `src/daemon/proto.rs` — add `ServerEvent::BusNotice`.
- `src/daemon/bus.rs` — new: `EventBus`.
- `src/daemon/socket.rs` — refactor `handle_connection` to loop + writer thread + turn-feeder + reply channel; new `bus` param.
- `src/daemon/mod.rs` — own `Arc<EventBus>`; pass to `handle_connection`; workgraph + supervisor threads broadcast.
- `src/bin/cc.rs` (+ `src/client/mod.rs` if needed) — reader thread + turn-done signal; render `BusNotice`.
- `src/background.rs` — possibly expose a one-line summary helper for the workgraph broadcast (or inline in `daemon/mod.rs`).
