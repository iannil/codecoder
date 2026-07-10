# Background Agent: headless one-shot runner

A Background Agent is a full-LLM-loop agent that runs autonomously with **no user
present** (CONTEXT.md). v1 ships the minimal shape: a **headless one-shot runner**
triggered by `CODECODER_BG_TASK=<task>`, which drives exactly one turn and exits.
Scheduling is external (cron/CI).

## Permission model (the "no user present" problem)

Only the top-level interactive agent owns a user-facing channel (see
[[0016-channel-topology-and-event-model]]); a Background Agent has none, so a
permission prompt would have no one to answer it. Rather than queue prompts, the
headless gate resolves them at authorization time:

- An `Ask { key }` tool runs **only if `key` is already in the session or the
  persisted project allowlist** (`codecoder.json`, see [[0005-permission-scope-and-session-allowlist]]).
- Otherwise it is **auto-denied** — an error `ToolResult`, never a blocking prompt.
- `ask_user` / `confirm` / `plan` (which need a user) short-circuit to a denial.

Each denial also emits a `ToolFinished { is_error: true }` event so it is
observable in the event stream; the headless runner drains these into
`BgOutcome.denied`.

The user pre-authorizes by editing `codecoder.json` before launch. This turns
"who answers the prompt?" into "what was authorized up front?", eliminating the
runtime responder.

## Not a sub-agent

Unlike a Sub-agent ([[0019-sub-agent-capability-boundary]], read-only, user
present, synchronously awaited), a Background Agent has the **full builtin
toolbox** and may write/run — bounded only by the pre-authorized allowlist. It is
`headless`, a boolean on `AgentLoop` that only alters the unauthorized-Ask branch
and the interactive-tool intercepts; interactive behavior is unchanged.

## Deferred (named hard problems, not in v1)

SIGINT/cancel wiring, a built-in scheduler, and multi-runner resource limits are
out of scope; the external scheduler bounds concurrency.
