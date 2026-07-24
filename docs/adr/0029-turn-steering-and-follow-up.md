# Turn steering and follow-up

Implements roadmap Wave 1 #6 ([[0027-pi-comparison-and-borrowing-roadmap]]),
borrowed from pi's steering/follow-up queues. Lets the user redirect a **running**
turn without aborting it, and lets input submitted as the agent is about to stop
restart the turn instead of waiting for a fresh one.

## The constraint (why not `cmd_tx`)

The agent thread is blocked inside `process_turn` for the whole turn; `run()` does
not service `cmd_rx` until the turn returns ([[0016-channel-topology-and-event-model]]).
So mid-turn user input cannot arrive as an `AgentCommand` — exactly the reason
cancel flips a shared `CancelToken` directly rather than sending `AgentCommand::Cancel`.
Steering uses the same shape.

## SteerQueue

`SteerQueue(Arc<Mutex<Vec<String>>>)` — a shared handle (like `CancelToken`):

- The TUI **pushes** to it directly when the user submits non-slash text while a
  turn is in flight (`activity.is_some()`), instead of sending `ProcessMessage`.
- `process_turn` **drains** it and appends each entry as a `Role::User` message so
  the next provider call sees it.

`process_turn` drains at two points:

1. **Iteration top** (after the cancel check): pending steering is injected as
   `User` before the working set is built — the next provider call sees it. This
   is *steering* (redirect mid-tool-loop).
2. **Natural-stop point** (the assistant returned no tool calls, so the turn would
   end): if the drain yields anything, `continue` instead of `break`. This is
   *follow-up* — late input restarts the turn rather than being lost.

Input submitted after the turn has fully ended still flows the old way
(`AgentCommand::ProcessMessage` queued on `cmd_rx`, run as the next turn).

## Semantics

- Steering messages are ordinary `User` turns in the session and transcript — they
  persist ([[0004-session-persistence-and-migration]]) and the TUI echoes them.
- Order is preserved (FIFO drain).
- Headless Background Agents ([[0026-background-agent-headless-runner]]) have no
  TUI and never push; the queue stays empty, behavior unchanged.
- Cancel still wins: the cancel check precedes the iteration-top drain.

## Deferred

pi also has a **next-turn** queue (prepend to the next user prompt) distinct from
steering/follow-up. One unified queue covers both behaviors here; a separate
next-turn tier is deferred until a concrete need appears.

## 修订（2026-07-24，迭代 4：no-op 探索兜底）

steering 除用户注入外,新增 **agent 自发 nudge**:turn 内连续
`CODECODER_NOOP_NUDGE_THRESHOLD`(默认 3,0=禁用)个「纯探索」迭代(该迭代的
tool_calls 非空且全部 ∈ `EXPLORATION_TOOLS` = read_file/glob/grep/diff)后,内核追加
一条 `User` steering 消息推动 agent 动手修改或明确声明阻塞原因,并发 `Notice` 事件。
任一迭代出现非探索工具即重置计数;每 turn 至多注入一次。与迭代 1 的 needs_fix
自恢复叠加:turn 内先 nudge,若整轮仍无产出则由客观 gate→自恢复循环接手。
