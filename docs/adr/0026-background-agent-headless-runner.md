# Background Agent: headless one-shot runner

A Background Agent is a full-LLM-loop agent that runs autonomously with **no user
present** (CONTEXT.md). v1 ships the minimal shape: a **headless one-shot runner**
triggered by `CODECODER_BG_TASK=<task>`, which drives exactly one turn and exits.
Scheduling is external (cron/CI/systemd timer / launchd). See `docs/background-agent-scheduling.md` for example configurations.

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

## Graceful SIGINT cancel

A headless run wires SIGINT (Ctrl+C / `kill -INT`) to the agent's `CancelToken`
via `signal-hook` (`CancelToken::cancel_on_sigint`), registered for the initial
turn's agent and each auto-advanced milestone's agent. On SIGINT the token flips,
the turn loop's cancel check fires, and `run_command` kills its subprocess — so a
runaway task stops gracefully instead of requiring `kill -9`. The runner then
returns a partial `BgOutcome`. (Cancellation is cooperative: a turn blocked in a
single long provider call still completes that call before the next iteration
checks cancel.)

## Deferred (named hard problems, not in v1)

A built-in scheduler and multi-runner resource limits are out of scope; the
external scheduler bounds concurrency. (SIGINT/cancel, originally listed here,
is now implemented above.)

## 修订(2026-07-24,迭代 1):needs_fix 自恢复循环

此前 headless workgraph runner 只推进 `pending` 就绪里程碑:验收 `needs_fix` 后 runner 无法自恢复,需人手动重置 `pending` 才重试(dogfooding 评估的 P0 缺口,见 `docs/superpowers/audits/2026-07-23-coedit-dogfooding-evaluation.md`)。本迭代补上**有界自恢复**:

- **重试状态持久化**:`Milestone` 新增 `fix_attempts: usize` 与 `last_failure: Option<String>` 字段,随 `workgraph.json` 持久化——跨进程尊重预算。验收失败时把 gate_reason 写入 `last_failure`;pass 时清空(避免陈旧原因污染未来重试)。
- **选取**:`WorkGraph::next_retryable(max_attempts)` 选状态 `NeedsFix`、deps 全 Done、`fix_attempts < max_attempts` 的最低 id 节点;`max_attempts == 0` 恒返回 `None`(禁用自恢复)。
- **重试执行**:`retry_one_milestone` **先**递增 `fix_attempts`(即便 turn 崩溃预算也被尊重),再把 `last_failure` 注入 `build_repair_prompt` 重跑该里程碑并过同一套客观验收门。runner 主循环仅在无 `pending` 就绪里程碑时走重试分支。
- **预算**:`CODECODER_BG_MAX_FIX_ATTEMPTS`(默认 3)。**重试不计入 `max_auto`**,也不累加 `consecutive_fail`、不走 `next_action`;`StuckNeedsFix`(退出码 2)**仅在**既无就绪、又无预算内可重试节点时才落。
- **已知取舍**:启用自恢复(`max_fix_attempts > 0`)时,跨里程碑的 `consecutive_fail` 熔断(CircuitBreaker)实际被 per-node 重试预算 + `max_auto` 取代(重试路径不累加 `consecutive_fail`)。这符合本迭代"用有界重试替代过早熔断"的目标;未来可让耗尽预算的硬失败里程碑累加一个独立计数以恢复跨里程碑熔断。
- **作用域**:自恢复仅限 `CODECODER_BG_WORKGRAPH` headless runner(`run_background_cfg`);交互式的 `drive_workgraph` 路径(`src/agent.rs`)与 daemon 空闲推进线程(`src/daemon/mod.rs`,裸调 `advance_one_milestone`)仍只标记 `needs_fix`,需要人工重置为 `pending` 才会重试。
